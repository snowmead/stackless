//! Restart and teardown tests use the real engine with simulated provider failures.
use super::*;
use serde_json::{Value, json};
use stackless_core::engine::{Engine, UpRequest};
use stackless_core::state::{ResourcePhase, Store};
use stackless_stripe_projects::{CommandOutput, test_support};
use std::{
    path::Path,
    sync::{Arc, Mutex as StdMutex},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};
const CATALOG: &str = include_str!("../../stackless-stripe-projects/tests/fixtures/catalog.json");
const PROVIDER: &str = "prvdr_61Ulotgugv9hEYa1b5HwO";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
#[derive(Default)]
struct Remote {
    resource: Option<String>,
    native_name: Option<String>,
    project_alive: bool,
    catalog_removed: bool,
    creates: usize,
    project_deletes: usize,
    catalog_deletes: usize,
    posts: usize,
    polls: usize,
    current_changed: bool,
}
struct Runner {
    store: Store,
    remote: Arc<StdMutex<Remote>>,
}
#[async_trait]
impl CommandRunner for Runner {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        let owner = self.store.instance("demo").unwrap().unwrap();
        Ok(match args[0].as_str() {
            "billing" => test_support::ok_empty(),
            "catalog" => test_support::raw(CATALOG),
            "status" => test_support::ok(json!({"project":{"id":"stripe_project"}})),
            "services" => test_support::services(&[]),
            "env" if args.get(1).map(String::as_str) == Some("list") => {
                test_support::ok(json!({"environments":[{"name":owner.resource_namespace}]}))
            }
            "env" => {
                std::fs::write(
                    cwd.join(format!(".env.{}", owner.resource_namespace)),
                    "LARAVEL_CLOUD_APP_ID=app_1\nLARAVEL_CLOUD_API_TOKEN=test\n",
                )
                .unwrap();
                test_support::ok_empty()
            }
            "add" => {
                assert_eq!(args[1], "laravel_cloud/application");
                let name = args[3].clone();
                let record = self
                    .store
                    .resource(
                        &owner.instance_id,
                        &format!("catalog:laravel_cloud/application:{name}"),
                    )
                    .unwrap()
                    .unwrap();
                let value: Value = serde_json::from_str(&record.payload).unwrap();
                assert_eq!(value["_catalog_creation"]["submitted"], true);
                assert_eq!(record.phase, ResourcePhase::Intent);
                let mut remote = self.remote.lock().unwrap();
                assert_eq!(remote.creates, 0);
                remote.creates += 1;
                remote.resource = Some(name);
                remote.native_name = value["_catalog_creation"]["config"]["name"]
                    .as_str()
                    .map(str::to_owned);
                remote.project_alive = true;
                return Err(ProjectsError::Unavailable {
                    detail: "lost catalog creation response".into(),
                });
            }
            other => panic!("unexpected Stripe command {other}"),
        })
    }
    async fn request(
        &self,
        method: &str,
        route: &str,
        _cwd: &Path,
    ) -> Result<CommandOutput, ProjectsError> {
        let mut remote = self.remote.lock().unwrap();
        if method == "POST" {
            assert!(route.ends_with("/remote_project/remove"));
            assert!(!remote.project_alive);
            remote.catalog_removed = true;
            remote.catalog_deletes += 1;
            return Err(ProjectsError::Unavailable {
                detail: "lost catalog removal response".into(),
            });
        }
        let row = remote.resource.as_ref().map(|name| json!({"id":"remote_project", "name":name, "provider":PROVIDER, "service_ref":"application", "status":if remote.catalog_removed {"removed"} else {"complete"}}));
        let body = if route.contains('?') {
            json!({"data":row.into_iter().collect::<Vec<_>>(), "next_page_url":null})
        } else {
            row.unwrap()
        };
        Ok(CommandOutput {
            status: 0,
            stdout: body.to_string(),
            stderr: String::new(),
        })
    }
}

fn application() -> Value {
    json!({"data": {"id":"app_1", "type":"applications", "attributes": {
            "name":"demo", "root_directory":null, "repository":{"full_name":"org/repo"}
        }, "relationships":{"repository":{"data":{"type":"repositories","id":"repo_1"}},"defaultEnvironment":{"data":{"type":"environments","id":"env_1"}}}}})
}
fn environment() -> Value {
    json!({"data":{"id":"env_1","type":"environments", "attributes": {"slug":"isolated", "status":"running", "vanity_domain":"demo.laravel.cloud"},
            "relationships": {
                "application":{"data":{"type":"applications","id":"app_1"}},
                "branch":{"data":{"type":"branches","id":"branch_1"}},
                "currentDeployment":{"data":{"type":"deployments","id":"dep_1"}}
            }}, "included":[{"id":"branch_1","type":"branches","attributes":{"name":"main"},"relationships":{"repository":{"data":{"type":"repositories","id":"repo_1"}}}}]})
}
fn deployment(status: &str) -> Value {
    json!({"data":{"id":"dep_1","type":"deployments", "attributes":{
            "status":status, "branch_name":"main", "commit_hash":SHA
        }, "relationships":{"environment":{"data":{"type":"environments","id":"env_1"}}}}})
}

// Resolve the admitted remote source from a local fixture during materialization.
struct FixtureSubstrate<T>(T, PathBuf);
#[async_trait]
impl<T: Substrate> Substrate for FixtureSubstrate<T> {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn capabilities(&self) -> stackless_core::capabilities::Capabilities {
        self.0.capabilities()
    }
    fn validate_definition(&self, def: &StackDef) -> Result<(), SubstrateFault> {
        self.0.validate_definition(def)
    }
    fn supports_source_override(&self) -> bool {
        self.0.supports_source_override()
    }
    fn build_namespace(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        prior: &[Checkpoint],
        secrets: &BTreeMap<String, String>,
        purpose: stackless_core::substrate::NamespacePurpose,
    ) -> stackless_core::def::Namespace {
        self.0
            .build_namespace(def, instance, prior, secrets, purpose)
    }
    fn default_lease(&self) -> Duration {
        self.0.default_lease()
    }
    fn service_origin(
        &self,
        def: &StackDef,
        instance: &InstanceContext<'_>,
        service: &str,
    ) -> String {
        self.0.service_origin(def, instance, service)
    }
    fn refresh_each_operation(&self, step: &stackless_core::engine::Step) -> bool {
        self.0.refresh_each_operation(step)
    }
    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        let mut definition = ctx.def.clone();
        if ctx.step.kind == StepKind::Materialize {
            definition
                .services
                .get_mut(&ctx.step.node)
                .unwrap()
                .source
                .repo = self.1.display().to_string();
        }
        self.0
            .execute(StepContext {
                def: &definition,
                ..ctx
            })
            .await
    }
    async fn observe(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<Observation, SubstrateFault> {
        self.0.observe(instance, checkpoint).await
    }
    async fn destroy(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        self.0.destroy(instance, checkpoint).await
    }
    async fn destroy_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        record: &stackless_core::state::ResourceRecord,
    ) -> Result<(), SubstrateFault> {
        self.0.destroy_record(store, instance, record).await
    }
    async fn observe_record(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
        record: &stackless_core::state::ResourceRecord,
    ) -> Result<Observation, SubstrateFault> {
        self.0.observe_record(store, instance, record).await
    }
}

async fn engine_recovery(lost_post: bool, delayed_delete: bool, abandon_after_catalog: bool) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(&repo, &[&[("index.php", "fixture")]]).unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let remote = Arc::new(StdMutex::new(Remote::default()));
    let server = MockServer::start().await;
    let state = remote.clone();
    Mock::given(method("GET"))
        .and(path("/applications/app_1"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            if !state.project_alive {
                return ResponseTemplate::new(404);
            }
            let mut value = application();
            value["data"]["attributes"]["name"] = json!(state.native_name);
            ResponseTemplate::new(200).set_body_json(value)
        })
        .mount(&server)
        .await;
    let origin = server.uri();
    let state = remote.clone();
    Mock::given(method("GET"))
        .and(path("/environments/env_1"))
        .respond_with(move |_: &wiremock::Request| {
            let mut value = environment();
            value["data"]["attributes"]["vanity_domain"] = json!(origin);
            if state.lock().unwrap().current_changed {
                value["data"]["relationships"]["currentDeployment"]["data"]["id"] =
                    json!("another_deployment");
            }
            ResponseTemplate::new(200).set_body_json(value)
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    let post_store = store.clone();
    Mock::given(method("POST"))
        .and(path("/environments/env_1/deployments"))
        .respond_with(move |_: &wiremock::Request| {
            let native = recorded_native(&post_store);
            assert_eq!(native.app_id.as_deref(), Some("app_1"));
            let request = native.requests.values().next().unwrap();
            assert!(request.submitted);
            assert!(request.deployment_id.is_none());
            state.lock().unwrap().posts += 1;
            if lost_post {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(deployment("pending"))
            }
        })
        .expect(u64::from(!abandon_after_catalog))
        .mount(&server)
        .await;
    let state = remote.clone();
    let poll_store = store.clone();
    Mock::given(method("GET"))
        .and(path("/deployments/dep_1"))
        .respond_with(move |_: &wiremock::Request| {
            let native = recorded_native(&poll_store);
            let request = native.requests.values().next().unwrap();
            assert_eq!(request.deployment_id.as_deref(), Some("dep_1"));
            assert_eq!(request.commit.as_deref(), Some(SHA));
            let mut state = state.lock().unwrap();
            state.polls += 1;
            if state.polls == 1 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(deployment("deployment.succeeded"))
            }
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    let delete_store = store.clone();
    Mock::given(method("DELETE"))
        .and(path("/applications/app_1"))
        .respond_with(move |_: &wiremock::Request| {
            assert!(recorded_native(&delete_store).removal_submitted);
            let mut state = state.lock().unwrap();
            state.project_deletes += 1;
            if !delayed_delete {
                state.project_alive = false;
            }
            ResponseTemplate::new(503)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let text = "[stack]\nname='project'\n[services.web]\nsource={repo='https://github.com/org/repo',ref='main'}\nsetup='printf x >> setup-count'\nprepare='test -f setup-count && printf x >> prepare-count'\nhealth={path='/'}\n[services.web.laravel-cloud]\nrepository='org/repo'\nregion='us-east-1'\n";
    let def = StackDef::parse(text).unwrap();
    let request = || UpRequest {
        instance: "demo",
        definition_text: text,
        def: &def,
        source_overrides: BTreeMap::new(),
        dirty: false,
        definition_dir: dir.path().display().to_string(),
        lease: None,
        progress: None,
    };
    let make = |store: &Store| {
        let mut substrate = LaravelCloudSubstrate::for_test(
            Runner {
                store: store.clone(),
                remote: remote.clone(),
            },
            dir.path(),
            server.uri(),
            true,
        );
        substrate
            .secrets
            .insert("LARAVEL_CLOUD_API_TOKEN".into(), "test".into());
        FixtureSubstrate(substrate, repo.clone())
    };
    let substrate = make(&store);
    let engine = Engine {
        store: &store,
        substrate: &substrate,
    };
    let admission = engine.begin_up(&request()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap();
    store.grant_host_execution(&owner.instance_id).unwrap();
    store
        .submit_operation(
            "fixture-operation",
            "demo",
            "up",
            &json!({"definition":text}),
        )
        .unwrap();
    assert!(store.start_operation("fixture-operation").unwrap());
    store
        .bind_stripe_project(&owner.instance_id, "project:test", Some("stripe_project"))
        .unwrap();
    let error = engine.run_up(request(), admission).await.unwrap_err();
    assert!(
        error.to_string().contains("lost catalog creation"),
        "{error}"
    );
    drop(substrate);
    drop(store);
    if !abandon_after_catalog {
        let store = Store::open(&db).unwrap();
        store.recover_operations().unwrap();
        assert!(store.start_operation("fixture-operation").unwrap());
        let substrate = make(&store);
        let error = Engine {
            store: &store,
            substrate: &substrate,
        }
        .up(request())
        .await
        .unwrap_err();
        assert!(error.to_string().contains("503"), "{error}");
        assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
        drop(substrate);
        drop(store);
        let store = Store::open(&db).unwrap();
        store.recover_operations().unwrap();
        assert!(store.start_operation("fixture-operation").unwrap());
        let substrate = make(&store);
        let result = Engine {
            store: &store,
            substrate: &substrate,
        }
        .up(request())
        .await;
        if lost_post {
            assert!(result.unwrap_err().to_string().contains("unknown"));
            assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
        } else {
            result.unwrap();
            store
                .finish_operation(
                    "fixture-operation",
                    stackless_core::state::OperationStatus::Succeeded,
                    None,
                    None,
                )
                .unwrap();
            store
                .submit_operation(
                    "fixture-next-operation",
                    "demo",
                    "up",
                    &json!({"definition":text}),
                )
                .unwrap();
            assert!(store.start_operation("fixture-next-operation").unwrap());
            Engine {
                store: &store,
                substrate: &substrate,
            }
            .up(request())
            .await
            .unwrap();
            let snapshots: Vec<stackless_cloud::source::Snapshot> = store
                .resources(&owner.instance_id)
                .unwrap()
                .into_iter()
                .filter(|r| r.resource_kind == stackless_cloud::source::KIND)
                .map(|r| serde_json::from_str(&r.payload).unwrap())
                .collect();
            assert_eq!(snapshots.len(), 2);
            let operations: std::collections::BTreeSet<_> =
                snapshots.iter().map(|s| s.operation.as_str()).collect();
            assert_eq!(
                operations,
                std::collections::BTreeSet::from(["fixture-operation", "fixture-next-operation"])
            );
            store
                .finish_operation(
                    "fixture-next-operation",
                    stackless_core::state::OperationStatus::Succeeded,
                    None,
                    None,
                )
                .unwrap();
            for snapshot in snapshots {
                assert_eq!(
                    std::fs::read(snapshot.path.join("setup-count")).unwrap(),
                    b"x"
                );
                assert_eq!(
                    std::fs::read(snapshot.path.join("prepare-count")).unwrap(),
                    b"x"
                );
            }
            let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
            let context = InstanceContext::from_record(&owner, &[]);
            assert_eq!(
                substrate.observe(&context, &checkpoint).await.unwrap(),
                Observation::Present
            );
            remote.lock().unwrap().current_changed = true;
            assert!(matches!(
                substrate.observe(&context, &checkpoint).await.unwrap(),
                Observation::Drifted { .. }
            ));
        }
    }
    let store = Store::open(&db).unwrap();
    let substrate = make(&store);
    let source = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == stackless_cloud::source::KIND)
        .unwrap();
    let snapshot: stackless_cloud::source::Snapshot =
        serde_json::from_str(&source.payload).unwrap();
    assert_eq!(
        std::fs::read(snapshot.path.join("prepare-count")).unwrap(),
        b"x"
    );
    assert_eq!(
        std::fs::read(snapshot.path.join("setup-count")).unwrap(),
        b"x"
    );
    let sibling = store
        .create_instance(
            "sibling",
            "laravel-cloud",
            "definition",
            &BTreeMap::new(),
            "",
            false,
        )
        .unwrap();
    let record = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "laravel-cloud-application")
        .unwrap();
    assert!(
        substrate
            .destroy_record(
                &store,
                &InstanceContext::from_record(&sibling, &[]),
                &record
            )
            .await
            .is_err()
    );
    assert!(remote.lock().unwrap().project_alive);
    for response in [
        ResponseTemplate::new(403),
        ResponseTemplate::new(200).set_body_string("malformed"),
        ResponseTemplate::new(200).set_body_json(application()),
    ] {
        Mock::given(method("GET"))
            .and(path("/applications/app_1"))
            .respond_with(response)
            .with_priority(1)
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        assert!(
            substrate
                .destroy_record(&store, &InstanceContext::from_record(&owner, &[]), &record)
                .await
                .is_err()
        );
        assert!(
            substrate
                .observe_record(&store, &InstanceContext::from_record(&owner, &[]), &record)
                .await
                .is_err()
        );
        assert_eq!(remote.lock().unwrap().project_deletes, 0);
        assert_eq!(remote.lock().unwrap().catalog_deletes, 0);
    }
    remote.lock().unwrap().catalog_removed = true;
    assert_eq!(
        substrate
            .observe_record(&store, &InstanceContext::from_record(&owner, &[]), &record)
            .await
            .unwrap(),
        Observation::Present
    );
    remote.lock().unwrap().catalog_removed = false;

    assert!(
        Engine {
            store: &store,
            substrate: &substrate
        }
        .down("demo")
        .await
        .is_err()
    );
    if delayed_delete {
        let store = Store::open(&db).unwrap();
        let substrate = make(&store);
        assert!(
            Engine {
                store: &store,
                substrate: &substrate
            }
            .down("demo")
            .await
            .is_err()
        );
        let record = store
            .resources(&owner.instance_id)
            .unwrap()
            .into_iter()
            .find(|r| r.resource_kind == "laravel-cloud-application")
            .unwrap();
        assert_eq!(
            substrate
                .observe_record(&store, &InstanceContext::from_record(&owner, &[]), &record)
                .await
                .unwrap(),
            Observation::Present
        );
        assert_eq!(remote.lock().unwrap().catalog_deletes, 0);
        remote.lock().unwrap().project_alive = false;
    }
    let store = Store::open(&db).unwrap();
    let substrate = make(&store);
    assert!(
        Engine {
            store: &store,
            substrate: &substrate
        }
        .down("demo")
        .await
        .is_err()
    );
    let store = Store::open(&db).unwrap();
    let substrate = make(&store);
    Engine {
        store: &store,
        substrate: &substrate,
    }
    .down("demo")
    .await
    .unwrap();
    assert!(
        store
            .resources(&owner.instance_id)
            .unwrap()
            .iter()
            .all(|r| r.phase == ResourcePhase::Absent)
    );
    let state = remote.lock().unwrap();
    assert_eq!(
        (
            state.creates,
            state.posts,
            state.project_deletes,
            state.catalog_deletes
        ),
        (1, usize::from(!abandon_after_catalog), 1, 1)
    );
}
fn recorded_native(store: &Store) -> lifecycle::NativeState {
    let owner = store.instance("demo").unwrap().unwrap();
    let record = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "laravel-cloud-application")
        .unwrap();
    let value: Value = serde_json::from_str(&record.payload).unwrap();
    serde_json::from_value(value["_laravel_cloud"].clone()).unwrap()
}
#[tokio::test]
async fn known_deployment_resumes_after_reopen_and_native_deletion_precedes_catalog() {
    engine_recovery(false, false, false).await;
}
#[tokio::test]
async fn unknown_post_is_not_repeated_and_unconfirmed_delete_retains_ownership() {
    engine_recovery(true, true, false).await;
}

#[tokio::test]
async fn down_recovers_native_id_after_lost_catalog_creation_without_a_start_checkpoint() {
    engine_recovery(false, false, true).await;
}
