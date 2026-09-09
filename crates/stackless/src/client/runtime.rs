//! Private Stripe working directories and shared project recovery.

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use stackless_core::def::StackDef;
use stackless_core::lockfile::FileLock;
use stackless_core::paths::Paths;
use stackless_core::state::{
    InstanceRecord, Ownership, ResourceIntent, ResourcePhase, StateError, Store,
};
use stackless_stripe_projects::{CommandRunner, StripeProjects, project};

use crate::error::Error;

pub(super) const PROJECT_KEY: &str = "stripe:project";
pub(super) const ENVIRONMENT_KEY: &str = "stripe:environment";
pub(super) const ENVIRONMENT_KIND: &str = "stripe-environment";

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct EnvironmentPayload {
    pub project_id: String,
    pub environment: String,
    #[serde(default)]
    pub creation_submitted: bool,
}

pub(crate) struct RuntimeContext {
    pub dir: PathBuf,
    pub project_id: Option<String>,
    _guard: FileLock,
}

fn invalid(detail: impl Into<String>) -> Error {
    StateError::ResourceInvariant {
        detail: detail.into(),
    }
    .into()
}

fn private_dir(path: &Path) -> Result<(), Error> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(Error::Runtime)?;
    if !std::fs::symlink_metadata(path)
        .map_err(Error::Runtime)?
        .file_type()
        .is_dir()
    {
        return Err(invalid(
            "runtime directory must be a directory, not a symlink",
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(Error::Runtime)
}

fn write_snapshot(dir: &Path, text: &str) -> Result<(), Error> {
    write_snapshot_with_lock(dir, text, &FileLock::stripe_lock_path(dir))
}

fn write_snapshot_with_lock(dir: &Path, text: &str, lock_path: &Path) -> Result<(), Error> {
    // A helper acknowledges this lock before its caller releases user code.
    // After controller death, wait for that command before replacing its input.
    let _command =
        FileLock::acquire_existing(lock_path, Duration::from_secs(30)).map_err(|error| {
            invalid(format!(
                "Stripe command is still using this runtime: {error}"
            ))
        })?;
    let temporary = dir.join(format!(
        "definition-{}.tmp",
        stackless_core::state::new_operation_id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(Error::Runtime)?;
    file.write_all(text.as_bytes()).map_err(Error::Runtime)?;
    file.sync_all().map_err(Error::Runtime)?;
    std::fs::rename(temporary, dir.join("stackless.toml")).map_err(Error::Runtime)
}

fn stripe_error(error: stackless_stripe_projects::ProjectsError) -> Error {
    Error::substrate(
        stackless_core::substrate::SubstrateFault::from_fault(&error),
        None,
    )
}

fn ready(store: &Store, owner: &str, key: &str, id: &str, payload: &str) -> Result<(), Error> {
    let record = store
        .resource(owner, key)?
        .ok_or_else(|| invalid("missing Stripe resource intent"))?;
    if record.phase == ResourcePhase::Ready {
        if record.resource_id != id {
            return Err(invalid(
                "Stripe resource handle changed within one identity",
            ));
        }
        return Ok(());
    }
    store.resource_created(owner, key, id, payload)?;
    store.resource_ready(owner, key)?;
    Ok(())
}

fn intent(
    store: &Store,
    record: &InstanceRecord,
    key: &str,
    kind: &str,
    ownership: Ownership,
    id: &str,
    payload: &str,
) -> Result<(), Error> {
    let resource = store.resource_intent(ResourceIntent {
        owner_id: &record.instance_id,
        key,
        step_id: stackless_core::state::INSTANCE_RESOURCE_STEP,
        provider: record.substrate.as_str(),
        ownership,
        resource_kind: kind,
        resource_id: id,
        payload,
        dependencies: if key == ENVIRONMENT_KEY {
            &[PROJECT_KEY]
        } else {
            &[]
        },
    })?;
    if resource.phase == ResourcePhase::Absent {
        store.resource_rearm(&record.instance_id, key, id, payload)?;
    }
    Ok(())
}

/// The session guard covers selection, provisioning, env pull, and API token
/// resolution. A per-command lock alone would allow context switches between them.
pub(crate) async fn prepare<R: CommandRunner>(
    paths: &Paths,
    store: &Store,
    record: &InstanceRecord,
    def: &StackDef,
    definition_text: &str,
    create: bool,
    runner: &R,
) -> Result<RuntimeContext, Error> {
    if record.instance_id.len() != 32 || !record.instance_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("invalid immutable instance ID"));
    }
    let root = paths.state_dir().join("runtime");
    private_dir(&root)?;
    let dir = root.join(&record.instance_id);
    private_dir(&dir)?;
    let guard =
        FileLock::acquire_with_wait(&dir.join("stripe-session.lock"), Duration::from_secs(30))
            .map_err(|err| invalid(format!("Stripe session is busy: {err}")))?;
    write_snapshot(&dir, definition_text)?;
    let mut context = RuntimeContext {
        dir,
        project_id: None,
        _guard: guard,
    };
    let existing = store.instance_stripe_project(&record.instance_id)?;
    let requested = project::recorded_project_id(def);
    let needs_stripe = record.substrate.as_str() != "local"
        || def
            .services
            .values()
            .any(|workload| workload.on.as_deref().is_some_and(|on| on != "local"))
        || !def.integrations.is_empty()
        || requested.is_some()
        || existing.is_some();
    if !needs_stripe {
        return Ok(context);
    }

    let scope = match (&existing, &requested) {
        (Some(bound), want)
            if want
                .as_ref()
                .is_none_or(|id| bound.project_id.as_ref() == Some(id)) =>
        {
            bound.scope.clone()
        }
        (_, Some(id)) => format!("project:{id}"),
        _ => format!("definition:{}:{}", record.definition_dir, def.stack.name),
    };
    if !create && existing.is_none() && requested.is_none() {
        return Err(invalid(
            "legacy Stripe project identity is not recorded; record its project ID before teardown or credential access",
        ));
    }
    let bound = store.bind_stripe_project(&record.instance_id, &scope, requested.as_deref())?;
    if create {
        intent(
            store,
            record,
            PROJECT_KEY,
            "stripe-project",
            Ownership::Shared,
            &bound.context_id,
            &serde_json::json!({"scope": scope}).to_string(),
        )?;
    }
    let project_root = paths.state_dir().join("stripe-projects");
    private_dir(&project_root)?;
    let project_dir = project_root.join(&bound.context_id);
    private_dir(&project_dir)?;
    let project_guard =
        FileLock::acquire_with_wait(&project_dir.join("setup.lock"), Duration::from_secs(30))
            .map_err(|err| invalid(format!("shared Stripe project setup is busy: {err}")))?;
    let bound = store
        .stripe_project(&scope)?
        .ok_or_else(|| invalid("Stripe project intent disappeared"))?;
    let project_id = match bound.project_id {
        Some(id) => id,
        None => {
            let stripe = StripeProjects::new(runner, &project_dir);
            let found = project::project_named(&stripe, &bound.resource_name)
                .await
                .map_err(stripe_error)?;
            let id = match found {
                Some(id) => id,
                None if create && !bound.creation_started => {
                    project::run_init_preflight(&stripe, &bound.resource_name)
                        .await
                        .map_err(stripe_error)?;
                    if !store.start_stripe_project_creation(&scope)? {
                        return Err(invalid("Stripe project creation is already in progress"));
                    }
                    project::initialize_named_project(&stripe, &bound.resource_name)
                        .await
                        .map_err(stripe_error)?;
                    project::project_named(&stripe, &bound.resource_name)
                        .await
                        .map_err(stripe_error)?
                        .ok_or_else(|| {
                            invalid(
                                "Stripe project creation returned without a recoverable project ID",
                            )
                        })?
                }
                None => {
                    return Err(invalid(
                        "Stripe project creation outcome is unknown; exact-name lookup found no project, so a second create is refused",
                    ));
                }
            };
            store.stripe_project_created(&scope, &id)?;
            id
        }
    };
    drop(project_guard);
    if create {
        ready(
            store,
            &record.instance_id,
            PROJECT_KEY,
            &project_id,
            &serde_json::json!({"scope": scope, "project_id": project_id}).to_string(),
        )?;
    }
    let stripe = StripeProjects::new(runner, &context.dir);
    project::pull_project(&stripe, &project_id)
        .await
        .map_err(stripe_error)?;
    context.project_id = Some(project_id.clone());
    if create {
        let mut environment = EnvironmentPayload {
            project_id: project_id.clone(),
            environment: record.resource_namespace.clone(),
            creation_submitted: false,
        };
        let payload =
            serde_json::to_string(&environment).map_err(|error| invalid(error.to_string()))?;
        intent(
            store,
            record,
            ENVIRONMENT_KEY,
            ENVIRONMENT_KIND,
            Ownership::Owned,
            &record.resource_namespace,
            &payload,
        )?;
        let exists = project::environment_registered(&stripe, &record.resource_namespace)
            .await
            .map_err(stripe_error)?;
        let tracked = store
            .resource(&record.instance_id, ENVIRONMENT_KEY)?
            .ok_or_else(|| invalid("environment intent disappeared"))?;
        let prior: EnvironmentPayload =
            serde_json::from_str(&tracked.payload).map_err(|error| invalid(error.to_string()))?;
        if exists {
            if tracked.phase == ResourcePhase::Intent && !prior.creation_submitted {
                store.resource_absent(&record.instance_id, ENVIRONMENT_KEY)?;
                return Err(invalid(
                    "Stripe environment existed before this instance submitted creation",
                ));
            }
            project::select_environment(&stripe, &record.resource_namespace)
                .await
                .map_err(stripe_error)?;
        } else {
            if tracked.phase == ResourcePhase::Intent && prior.creation_submitted {
                return Err(stripe_error(
                    stackless_stripe_projects::ProjectsError::CreationUnknown {
                        resource: record.resource_namespace.clone(),
                    },
                ));
            }
            if tracked.phase != ResourcePhase::Intent {
                store.resource_absent(&record.instance_id, ENVIRONMENT_KEY)?;
                store.resource_rearm(
                    &record.instance_id,
                    ENVIRONMENT_KEY,
                    &record.resource_namespace,
                    &payload,
                )?;
            }
            environment.creation_submitted = true;
            let submitted =
                serde_json::to_string(&environment).map_err(|error| invalid(error.to_string()))?;
            store.resource_intent_payload(&record.instance_id, ENVIRONMENT_KEY, &submitted)?;
            project::create_environment(&stripe, &record.resource_namespace)
                .await
                .map_err(stripe_error)?;
        }
        environment.creation_submitted = true;
        let payload =
            serde_json::to_string(&environment).map_err(|error| invalid(error.to_string()))?;
        ready(
            store,
            &record.instance_id,
            ENVIRONMENT_KEY,
            &record.resource_namespace,
            &payload,
        )?;
    }
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_stripe_projects::{CommandOutput, ProjectsError};
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    #[derive(Default)]
    struct State {
        projects: BTreeMap<String, String>,
        linked: BTreeMap<PathBuf, String>,
        environments: BTreeMap<String, BTreeSet<String>>,
        creates: usize,
        lose_create_response: bool,
        lose_environment_response: bool,
        hide_environments: bool,
        environment_creates: usize,
        calls: Vec<PathBuf>,
    }
    #[derive(Default)]
    struct Runner(Mutex<State>);

    #[async_trait::async_trait]
    impl CommandRunner for Runner {
        async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
            let mut state = self.0.lock().unwrap();
            state.calls.push(cwd.into());
            let data = match args[0].as_str() {
                "list" => {
                    serde_json::json!({"projects": state.projects.iter().map(|(name, id)| serde_json::json!({"name":name,"id":id})).collect::<Vec<_>>()})
                }
                "init" if args.iter().any(|a| a == "--preflight") => serde_json::json!({}),
                "init" => {
                    state.creates += 1;
                    let id = format!("project_{}", state.creates);
                    state.projects.insert(args[1].clone(), id);
                    if std::mem::take(&mut state.lose_create_response) {
                        return Err(ProjectsError::Unavailable {
                            detail: "lost create response".into(),
                        });
                    }
                    serde_json::json!({})
                }
                "pull" => {
                    if state.linked.contains_key(cwd) {
                        return Ok(CommandOutput {
                            status: 1,
                            stdout: serde_json::json!({"ok":false,"error":{"code":"PROJECT_ALREADY_CONNECTED","message":"already linked"}}).to_string(),
                            stderr: String::new(),
                        });
                    }
                    state.linked.insert(cwd.into(), args[1].clone());
                    serde_json::json!({})
                }
                "status" => serde_json::json!({"project": {"id": state.linked.get(cwd).unwrap()}}),
                "env" => {
                    let id = state.linked.get(cwd).unwrap().clone();
                    let hidden = state.hide_environments;
                    let environments = state.environments.entry(id).or_default();
                    match args[1].as_str() {
                        "list" => {
                            serde_json::json!({"environments": environments.iter().filter(|_| !hidden).map(|name| (name.clone(), serde_json::json!({}))).collect::<BTreeMap<_,_>>()})
                        }
                        "create" => {
                            assert!(environments.insert(args[2].clone()));
                            state.environment_creates += 1;
                            if std::mem::take(&mut state.lose_environment_response) {
                                return Err(ProjectsError::Unavailable {
                                    detail: "lost environment create response".into(),
                                });
                            }
                            serde_json::json!({})
                        }
                        "use" => {
                            assert!(environments.contains(&args[2]));
                            serde_json::json!({})
                        }
                        other => panic!("unexpected env verb {other}"),
                    }
                }
                other => panic!("unexpected Stripe verb {other}"),
            };
            Ok(CommandOutput {
                status: 0,
                stdout: serde_json::json!({"ok":true,"data":data}).to_string(),
                stderr: String::new(),
            })
        }
    }

    fn fixture() -> (tempfile::TempDir, Paths, Store, StackDef, String, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        std::fs::create_dir(&app).unwrap();
        let text = "[stack]\nname = 'shared-project'\n".to_owned();
        std::fs::write(app.join("stackless.toml"), &text).unwrap();
        let paths = Paths::new(root.path().join("state"));
        let store = Store::open(&paths.db_path()).unwrap();
        let def = StackDef::parse(&text).unwrap();
        (root, paths, store, def, text, app)
    }

    #[tokio::test]
    async fn local_default_with_cloud_placement_prepares_one_stripe_context() {
        let (_root, paths, store, _, _, app) = fixture();
        let text = "[stack]\nname='mixed'\n[workloads.api]\non='fly'\nimage='nginx:alpine'\nhealth={path='/'}\n";
        let def = StackDef::parse(text).unwrap();
        let owner = store
            .create_instance(
                "demo",
                "local",
                text,
                &BTreeMap::new(),
                app.to_str().unwrap(),
                false,
            )
            .unwrap();
        let runner = Runner::default();
        let runtime = prepare(&paths, &store, &owner, &def, text, true, &runner)
            .await
            .unwrap();
        assert!(runtime.project_id.is_some());
        assert_eq!(runner.0.lock().unwrap().creates, 1);
        assert_eq!(runner.0.lock().unwrap().environment_creates, 1);
        assert!(
            store
                .instance_stripe_project(&owner.instance_id)
                .unwrap()
                .is_some()
        );
        drop(runtime);
        let runtime = prepare(&paths, &store, &owner, &def, text, true, &runner)
            .await
            .unwrap();
        assert!(runtime.project_id.is_some());
        assert_eq!(runner.0.lock().unwrap().creates, 1);
        assert_eq!(runner.0.lock().unwrap().environment_creates, 1);
        drop(runtime);
        let teardown = prepare(&paths, &store, &owner, &def, text, false, &runner)
            .await
            .unwrap();
        assert!(teardown.project_id.is_some());
        assert_eq!(runner.0.lock().unwrap().creates, 1);
        assert_eq!(runner.0.lock().unwrap().environment_creates, 1);
    }

    #[tokio::test]
    async fn instances_share_only_project_identity_and_never_write_the_application() {
        let (_root, paths, store, def, text, app) = fixture();
        let runner = Runner::default();
        let one = store
            .create_instance(
                "one",
                "render",
                &text,
                &BTreeMap::new(),
                app.to_str().unwrap(),
                false,
            )
            .unwrap();
        let two = store
            .create_instance(
                "two",
                "render",
                &text,
                &BTreeMap::new(),
                app.to_str().unwrap(),
                false,
            )
            .unwrap();
        let first = prepare(&paths, &store, &one, &def, &text, true, &runner)
            .await
            .unwrap();
        let second = prepare(&paths, &store, &two, &def, &text, true, &runner)
            .await
            .unwrap();
        assert_ne!(first.dir, second.dir);
        assert_eq!(first.project_id, second.project_id);
        assert_eq!(runner.0.lock().unwrap().creates, 1);
        assert!(
            runner
                .0
                .lock()
                .unwrap()
                .calls
                .iter()
                .all(|dir| dir.starts_with(paths.state_dir()))
        );
        assert_eq!(
            std::fs::read_to_string(app.join("stackless.toml")).unwrap(),
            text
        );
        assert_eq!(std::fs::read_dir(&app).unwrap().count(), 1);
        assert_eq!(
            std::fs::metadata(&first.dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(FileLock::try_acquire(&first.dir.join("stripe-session.lock")).is_err());
        for record in [&one, &two] {
            let environment = store
                .resource(&record.instance_id, ENVIRONMENT_KEY)
                .unwrap()
                .unwrap();
            assert_eq!(environment.ownership, Ownership::Owned);
            assert_eq!(environment.resource_id, record.resource_namespace);
            assert_eq!(environment.dependencies, [PROJECT_KEY]);
        }
        drop(first);
        let restarted = prepare(&paths, &store, &one, &def, &text, true, &runner)
            .await
            .unwrap();
        assert_eq!(restarted.project_id, second.project_id);
        assert_eq!(runner.0.lock().unwrap().creates, 1);
    }

    #[tokio::test]
    async fn lost_project_create_response_is_recovered_without_a_second_create() {
        let (_root, paths, store, def, text, app) = fixture();
        let runner = Runner::default();
        runner.0.lock().unwrap().lose_create_response = true;
        let record = store
            .create_instance(
                "lost",
                "render",
                &text,
                &BTreeMap::new(),
                app.to_str().unwrap(),
                false,
            )
            .unwrap();
        assert!(
            prepare(&paths, &store, &record, &def, &text, true, &runner)
                .await
                .is_err()
        );
        let bound = store
            .instance_stripe_project(&record.instance_id)
            .unwrap()
            .unwrap();
        assert!(bound.creation_started);
        assert!(bound.project_id.is_none());
        drop(store);
        let store = Store::open(&paths.db_path()).unwrap();
        let recovered = prepare(&paths, &store, &record, &def, &text, true, &runner)
            .await
            .unwrap();
        assert_eq!(recovered.project_id.as_deref(), Some("project_1"));
        assert_eq!(runner.0.lock().unwrap().creates, 1);
    }

    #[tokio::test]
    async fn unknown_project_creation_never_repeats_the_create() {
        let (_root, paths, store, def, text, app) = fixture();
        let runner = Runner::default();
        let record = store
            .create_instance(
                "unknown",
                "render",
                &text,
                &BTreeMap::new(),
                app.to_str().unwrap(),
                false,
            )
            .unwrap();
        let scope = format!("definition:{}:{}", record.definition_dir, def.stack.name);
        store
            .bind_stripe_project(&record.instance_id, &scope, None)
            .unwrap();
        store.start_stripe_project_creation(&scope).unwrap();
        assert!(
            prepare(&paths, &store, &record, &def, &text, true, &runner)
                .await
                .is_err()
        );
        assert_eq!(runner.0.lock().unwrap().creates, 0);
    }

    #[tokio::test]
    async fn lost_environment_creation_waits_for_discovery_and_never_repeats_the_create() {
        let (_root, paths, store, def, text, app) = fixture();
        let runner = Runner::default();
        runner.0.lock().unwrap().lose_environment_response = true;
        let record = store
            .create_instance(
                "lost-env",
                "render",
                &text,
                &BTreeMap::new(),
                app.to_str().unwrap(),
                false,
            )
            .unwrap();
        assert!(
            prepare(&paths, &store, &record, &def, &text, true, &runner)
                .await
                .is_err()
        );
        let pending = store
            .resource(&record.instance_id, ENVIRONMENT_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(pending.phase, ResourcePhase::Intent);
        let payload: EnvironmentPayload = serde_json::from_str(&pending.payload).unwrap();
        assert!(payload.creation_submitted);
        drop(store);
        let store = Store::open(&paths.db_path()).unwrap();
        runner.0.lock().unwrap().hide_environments = true;
        assert!(
            prepare(&paths, &store, &record, &def, &text, true, &runner)
                .await
                .is_err()
        );
        assert_eq!(runner.0.lock().unwrap().environment_creates, 1);
        runner.0.lock().unwrap().hide_environments = false;
        let context = prepare(&paths, &store, &record, &def, &text, true, &runner)
            .await
            .unwrap();
        assert!(context.project_id.is_some());
        assert_eq!(runner.0.lock().unwrap().environment_creates, 1);
        assert_eq!(
            store
                .resource(&record.instance_id, ENVIRONMENT_KEY)
                .unwrap()
                .unwrap()
                .phase,
            ResourcePhase::Ready
        );
    }
    #[test]
    fn runtime_snapshot_waits_for_the_helper_command_lock() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("command.lock");
        std::fs::write(root.path().join("stackless.toml"), "old snapshot").unwrap();
        let held = FileLock::try_acquire(&path).unwrap();
        let directory = root.path().to_path_buf();
        let (started, wait_started) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            started.send(()).unwrap();
            write_snapshot_with_lock(&directory, "new snapshot", &path)
        });
        wait_started.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!writer.is_finished());
        assert_eq!(
            std::fs::read_to_string(root.path().join("stackless.toml")).unwrap(),
            "old snapshot"
        );
        drop(held);
        writer.join().unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("stackless.toml")).unwrap(),
            "new snapshot"
        );
    }
}
