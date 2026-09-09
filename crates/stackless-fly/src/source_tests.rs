use super::*;
use serde_json::json;
use stackless_core::{engine::Step, state::Store};
use stackless_stripe_projects::{CommandOutput, test_support};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const PROVIDER: &str = "prvdr_61UQUoA4NItxbLNIU5FF2";
struct Runner(std::sync::Arc<std::sync::Mutex<String>>);
#[async_trait]
impl CommandRunner for Runner {
    async fn run(
        &self,
        args: &[String],
        _cwd: &std::path::Path,
    ) -> Result<CommandOutput, ProjectsError> {
        Ok(match args[0].as_str() {
            "catalog" => test_support::raw(include_str!(
                "../../stackless-stripe-projects/tests/fixtures/catalog.json"
            )),
            "services" => test_support::services(&[]),
            "env" => test_support::ok_empty(),
            "add" => {
                assert_eq!(args[1], "flyio/app");
                *self.0.lock().unwrap() = args[3].clone();
                test_support::ok(
                    json!({"service":{"key":"remote_app", "provider_id":PROVIDER},"variables": {
                        "FLYIO_DEPLOY_TOKEN": "fixture-deploy-token"
                    }}),
                )
            }
            other => panic!("unexpected Stripe command {other}"),
        })
    }
    async fn request(
        &self,
        method: &str,
        route: &str,
        _cwd: &std::path::Path,
    ) -> Result<CommandOutput, ProjectsError> {
        assert_eq!(method, "GET");
        let name = self.0.lock().unwrap().clone();
        let resource = json!({"id":"remote_app", "provider":PROVIDER,"service_ref":"app","status":"complete","name":name});
        let body = if route.contains('?') {
            json!({"data":if name.is_empty() {vec![]} else {vec![resource]},"next_page_url":null})
        } else {
            resource
        };
        Ok(CommandOutput {
            status: 0,
            stdout: body.to_string(),
            stderr: String::new(),
        })
    }
}

fn context<'a>(
    store: &'a Store,
    instance: &'a InstanceContext<'a>,
    def: &'a StackDef,
    step: &'a Step,
    prior: &'a [Checkpoint],
    overrides: &'a BTreeMap<String, String>,
) -> StepContext<'a> {
    StepContext {
        operation_id: "source-test",
        store,
        instance,
        def,
        step,
        source_overrides: overrides,
        dirty: false,
        prior,
        parent_resources: &[],
        cancelled: None,
    }
}

#[tokio::test]
async fn source_build_uses_saved_root_after_restart_and_prepare_edits() {
    source_build_scenario(BuildScenario::LostResponse, false).await;
}

#[tokio::test]
async fn source_build_reconnects_to_running_process_before_reading_native_receipt() {
    source_build_scenario(BuildScenario::Resume, false).await;
}

#[tokio::test]
async fn source_build_cancellation_stops_the_recorded_process() {
    source_build_scenario(BuildScenario::Cancel, false).await;
}

#[tokio::test]
async fn source_build_deadline_stops_the_recorded_process() {
    source_build_scenario(BuildScenario::Timeout, false).await;
}

#[tokio::test]
async fn source_build_teardown_stops_the_builder_before_a_native_authorization_failure() {
    source_build_scenario(BuildScenario::Teardown, false).await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BuildScenario {
    LostResponse,
    Resume,
    Cancel,
    Timeout,
    Teardown,
}

#[tokio::test]
async fn worker_source_build_recovers_without_http_config_or_public_ips() {
    source_build_scenario(BuildScenario::LostResponse, true).await;
}

async fn source_build_scenario(scenario: BuildScenario, worker: bool) {
    let interrupt = scenario != BuildScenario::LostResponse;
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(
        &repo,
        &[&[
            ("app/index.html", "<p>original</p>"),
            ("app/docker/Dockerfile", "FROM scratch"),
            ("app/.env", "private"),
            ("index.html", "wrong root"),
        ]],
    )
    .unwrap();
    let mut def = StackDef::parse(&format!("[stack]\nname='fixture'\n[services.web]\nsource={{repo={:?},ref='main',root='app'}}\nprepare='cat index.html > seen.txt; printf changed > index.html'\nhealth={{path='/'}}\n[services.web.fly]\ndockerfile='docker/Dockerfile'\n", repo.display().to_string())).unwrap();
    if worker {
        let spec = def.services.get_mut("web").unwrap();
        spec.kind = stackless_core::def::WorkloadKind::Worker;
        spec.health = None;
    }
    if scenario == BuildScenario::Timeout {
        def.services.get_mut("web").unwrap().timeout_secs = 3;
    }
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    store
        .create_instance("demo", "fly", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    let record = store.instance("demo").unwrap().unwrap();
    store.grant_host_execution(&record.instance_id).unwrap();
    let instance = InstanceContext::from_record(&record, &[]);
    store
        .bind_stripe_project(&record.instance_id, "source:test", Some("stripe_project"))
        .unwrap();
    let overrides = BTreeMap::new();
    let mut substrate =
        FlySubstrate::for_test(Runner(Default::default()), dir.path(), server.uri(), true);
    *substrate.ensured.lock().await = true;
    let app = FlySubstrate::<Runner>::resource_name(&def, &instance, "web");
    Mock::given(method("GET"))
        .and(path(format!("/apps/{app}/ip_assignments")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ips": [{"ip":"127.0.0.1"},{"ip":"::1"}]})),
        )
        .expect(0..=if worker { 0 } else { u64::MAX })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/apps/{app}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":"app_1","name":app,"organization":{"slug":"managed"}})),
        )
        .mount(&server)
        .await;
    let submitted_config = dir.path().join("submitted.toml");
    let recorded_machine = {
        let app = app.clone();
        let submitted_config = submitted_config.clone();
        move || {
            if !submitted_config.exists() {
                return None;
            }
            let text = std::fs::read_to_string(&submitted_config).unwrap();
            let config: toml::Value = toml::from_str(&text).unwrap();
            let env: Vec<(String, String)> = config["env"]
                .as_table()
                .unwrap()
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
                .collect();
            if worker {
                assert!(config.get("http_service").is_none());
                assert_eq!(config["restart"][0]["policy"].as_str(), Some("always"));
                assert!(config["env"].get("PORT").is_none());
            }
            let spec = MachineSpec {
                run: None,
                name: &app,
                region: "iad",
                image: "registry.fly.io/fixture:receipt",
                cmd: None,
                env: &env,
                internal_port: (!worker).then_some(8080),
                worker,
                cpu_kind: "shared",
                cpus: 1,
                memory_mb: 256,
            };
            let mut value = spec.to_body();
            value["id"] = "machine-one".into();
            value["state"] = "started".into();
            value["image_ref"] = json!({"digest":format!("sha256:{}", "a".repeat(64))});
            Some(value)
        }
    };
    let list_machine = recorded_machine.clone();
    Mock::given(method("GET"))
        .and(path(format!("/apps/{app}/machines")))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(list_machine().into_iter().collect::<Vec<_>>())
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/apps/{app}/machines/machine-one")))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(recorded_machine().unwrap())
        })
        .mount(&server)
        .await;
    let binary = dir.path().join("fake-flyctl");
    let started = dir.path().join("build-started");
    let launches = dir.path().join("build-launches");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
set -eu
test "$(cat index.html)" = '<p>original</p>'
test "$(cat docker/Dockerfile)" = 'FROM scratch'
test ! -e .env
test ! -e seen.txt
test ! -e .git
test ! -e home
test "$FLY_API_TOKEN" = 'fixture-deploy-token'
test -d "$HOME"
test -z "${STRIPE_API_KEY+x}"
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--config' ]; then
    shift
    test -f "$1"
    case "$1" in "$PWD"/*) exit 23;; esac
  fi
  shift
done
printf 'lost build response' >&2
exit 1
"#
        .replace(
            "test -f \"$1\"",
            &format!(
                "test -f \"$1\"\n    cp \"$1\" '{}'",
                submitted_config.display()
            ),
        )
        .replace(
            "test -f \"$1\"",
            &format!(
                "test -f \"$1\"\nprintf x >> '{}'\ntouch '{}'\n{}",
                launches.display(),
                started.display(),
                match scenario {
                    BuildScenario::LostResponse => ":",
                    BuildScenario::Resume => "sleep 0.5",
                    _ => "sleep 30",
                }
            ),
        )
        .replace("exit 1\n", if interrupt { "exit 0\n" } else { "exit 1\n" }),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    substrate.flyctl = Some(binary.clone());
    let materialize = Step {
        id: "materialize:web".into(),
        kind: StepKind::Materialize,
        node: "web".into(),
    };
    assert!(substrate.refresh_each_operation(&materialize));
    let resource = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &materialize,
            &[],
            &overrides,
        ))
        .await
        .unwrap();
    let snapshot: stackless_cloud::source::Snapshot =
        serde_json::from_str(&resource.payload).unwrap();
    drop(store);
    std::fs::remove_dir_all(&repo).unwrap();
    std::fs::remove_dir_all(&snapshot.path).unwrap();
    let store = Store::open(&db).unwrap();
    let resumed = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &materialize,
            &[],
            &overrides,
        ))
        .await
        .unwrap();
    assert_eq!(resumed.payload, resource.payload);
    let prior = [Checkpoint {
        instance: "demo".into(),
        step_id: materialize.id,
        resource_kind: resource.resource_kind,
        resource_id: resource.resource_id,
        payload: resource.payload,
        recorded_at: 0,
    }];
    let prepare = Step {
        id: "prepare:web".into(),
        kind: StepKind::Prepare,
        node: "web".into(),
    };
    substrate
        .execute(context(
            &store, &instance, &def, &prepare, &prior, &overrides,
        ))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(snapshot.path.join("app/seen.txt")).unwrap(),
        "<p>original</p>"
    );
    assert_eq!(
        std::fs::read_to_string(snapshot.path.join("app/index.html")).unwrap(),
        "changed"
    );
    let start = Step {
        id: "start:web".into(),
        kind: StepKind::Start,
        node: "web".into(),
    };
    if interrupt {
        let mut running = Box::pin(
            substrate.execute(context(&store, &instance, &def, &start, &prior, &overrides)),
        );
        tokio::select! {
            result = &mut running => panic!("builder ended before interruption: {result:?}"),
            _ = async {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !started.exists() {
                    assert!(std::time::Instant::now() < deadline);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } => (),
        }
        drop(running);
        assert!(!submitted_config.exists());
    } else {
        let error = substrate
            .execute(context(&store, &instance, &def, &start, &prior, &overrides))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("lost build response"), "{error}");
        assert!(submitted_config.exists());
    }
    let catalog = store
        .resources(instance.id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "fly-machine")
        .unwrap();
    let native = native_identity(&serde_json::from_str(&catalog.payload).unwrap())
        .unwrap()
        .0;
    let request = native.requests.values().next().unwrap();
    let build_process = request
        .build
        .as_ref()
        .unwrap()
        .process
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(build_process.process().is_alive(), interrupt);
    if matches!(scenario, BuildScenario::Cancel | BuildScenario::Timeout) {
        let mut ctx = context(&store, &instance, &def, &start, &prior, &overrides);
        if scenario == BuildScenario::Cancel {
            ctx.cancelled = Some(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                true,
            )));
        }
        let error = substrate.execute(ctx).await.unwrap_err();
        assert_eq!(error.code.as_ref(), codes::FLY_BUILD_STOPPED, "{error}");
        assert!(build_process.is_stopped());
        assert!(!submitted_config.exists());
        assert_eq!(std::fs::read_to_string(&launches).unwrap(), "x");
        assert!(remote_build::durable::present(dir.path(), instance.id, request).unwrap());
        assert!(
            substrate
                .execute(context(&store, &instance, &def, &start, &prior, &overrides))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&launches).unwrap(), "x");
        remote_build::durable::cleanup(dir.path(), instance.id, request)
            .await
            .unwrap();
        return;
    }
    if scenario == BuildScenario::Teardown {
        let other_id = "a".repeat(32);
        let sibling = InstanceContext {
            routed_origins: None,
            name: "other",
            id: &other_id,
            resource_namespace: "other",
            checkpoints: &[],
        };
        assert!(
            substrate
                .destroy_record(&store, &sibling, &catalog)
                .await
                .is_err()
        );
        assert!(build_process.process().is_alive());
        Mock::given(method("GET"))
            .and(path(format!("/apps/{app}")))
            .respond_with(ResponseTemplate::new(401))
            .with_priority(1)
            .mount(&server)
            .await;
        assert!(
            substrate
                .destroy_record(&store, &instance, &catalog)
                .await
                .is_err()
        );
        assert!(build_process.is_stopped());
        assert!(!submitted_config.exists());
        assert!(!remote_build::durable::present(dir.path(), instance.id, request).unwrap());
        assert_ne!(
            store
                .resource(instance.id, &catalog.key)
                .unwrap()
                .unwrap()
                .phase,
            stackless_core::state::ResourcePhase::Absent
        );
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|request| request.method == "DELETE")
        );
        return;
    }
    std::fs::remove_file(&binary).unwrap();
    drop(store);
    let store = Store::open(&db).unwrap();
    let deployed = substrate
        .execute(context(&store, &instance, &def, &start, &prior, &overrides))
        .await
        .unwrap();
    let payload: ServicePayload = serde_json::from_str(&deployed.payload).unwrap();
    assert_eq!(payload.machine_id, "machine-one");
    assert!(build_process.is_stopped());
    assert_eq!(std::fs::read_to_string(&launches).unwrap(), "x");
    let current = payload
        .native
        .as_ref()
        .unwrap()
        .requests
        .values()
        .next()
        .unwrap();
    assert_eq!(
        current.build.as_ref().unwrap().process.as_ref().unwrap(),
        &build_process
    );
    assert!(
        remote_build::durable::cleanup(dir.path(), &"a".repeat(32), current)
            .await
            .is_err()
    );
    remote_build::durable::cleanup(dir.path(), instance.id, current)
        .await
        .unwrap();
    assert!(!remote_build::durable::present(dir.path(), instance.id, current).unwrap());
    assert_eq!(
        substrate.observe(&instance, &prior[0]).await.unwrap(),
        Observation::Present
    );
    let sibling = InstanceContext {
        routed_origins: None,
        name: "sibling",
        id: "another-owner",
        resource_namespace: "sibling",
        checkpoints: &[],
    };
    assert!(substrate.destroy(&sibling, &prior[0]).await.is_err());
    substrate.destroy(&instance, &prior[0]).await.unwrap();
    assert_eq!(
        substrate.observe(&instance, &prior[0]).await.unwrap(),
        Observation::Gone
    );
}

#[test]
fn common_root_and_dockerfile_paths_fail_closed() {
    let parse = |root: &str, provider: &str| {
        StackDef::parse(&format!("[stack]\nname='fixture'\n[services.web]\nsource={{repo='r',root={root:?}}}\nhealth={{path='/'}}\n[services.web.fly]\n{provider}")).unwrap()
    };
    assert_eq!(
        config::service_fly(&parse("./app", "root='app'"), "web")
            .unwrap()
            .root
            .as_deref(),
        Some("app")
    );
    assert_eq!(
        config::service_fly(&parse("app", "dockerfile='docker/Dockerfile'"), "web")
            .unwrap()
            .root
            .as_deref(),
        Some("app")
    );
    for root in ["../outside", "/app", ".env", "app/../outside"] {
        assert!(config::service_fly(&parse(root, ""), "web").is_err());
    }
    for provider in [
        "root='other'",
        "dockerfile='../Dockerfile'",
        "dockerfile='/Dockerfile'",
    ] {
        assert!(config::service_fly(&parse("app", provider), "web").is_err());
    }
}

#[tokio::test]
async fn image_skips_unused_source_but_setup_materializes_and_executes() {
    image_setup(false).await;
}
#[tokio::test]
async fn source_free_image_setup_recovers_an_owned_empty_workspace() {
    image_setup(true).await;
}
async fn image_setup(empty: bool) {
    let dir = tempfile::tempdir().unwrap();
    let mut def = StackDef::parse("[stack]\nname='fixture'\n[services.web]\nsource={repo='https://unavailable.invalid'}\nhealth={path='/'}\n[services.web.fly]\nimage='nginx'").unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    store
        .create_instance("demo", "fly", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    let record = store.instance("demo").unwrap().unwrap();
    let instance = InstanceContext::from_record(&record, &[]);
    let substrate = FlySubstrate::for_test(
        Runner(Default::default()),
        dir.path(),
        "http://127.0.0.1:1",
        false,
    );
    *substrate.ensured.lock().await = true;
    let step = Step {
        id: "materialize:web".into(),
        kind: StepKind::Materialize,
        node: "web".into(),
    };
    let resource = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &step,
            &[],
            &BTreeMap::new(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resource.resource_kind,
        stackless_core::substrate::action_resource(&step.id).resource_kind
    );
    assert!(store.resources(instance.id).unwrap().is_empty());
    substrate
        .capabilities()
        .validate(SUBSTRATE_NAME, &def)
        .unwrap();
    substrate.validate_definition(&def).unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(&repo, &[&[("data", "fixture")]]).unwrap();
    let spec = def.services.get_mut("web").unwrap();
    spec.source.repo = if empty {
        String::new()
    } else {
        repo.display().to_string()
    };
    spec.setup = Some("printf ready > setup-output".into());
    let source = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &step,
            &[],
            &BTreeMap::new(),
        ))
        .await
        .unwrap();
    let snapshot: stackless_cloud::source::Snapshot =
        serde_json::from_str(&source.payload).unwrap();
    if empty {
        assert_eq!(snapshot.kind, stackless_cloud::source::SourceKind::Empty);
        assert!(snapshot.commit.is_none());
        assert!(snapshot.archive(None).unwrap().files.is_empty());
        assert!(std::fs::read_dir(&snapshot.path).unwrap().next().is_none());
    }
    let recovered = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &step,
            &[],
            &BTreeMap::new(),
        ))
        .await
        .unwrap();
    assert_eq!(recovered.payload, source.payload);
    let prior = [Checkpoint {
        instance: "demo".into(),
        step_id: step.id,
        resource_kind: source.resource_kind,
        resource_id: source.resource_id,
        payload: source.payload,
        recorded_at: 0,
    }];
    let setup = Step {
        id: "setup:web".into(),
        kind: StepKind::Setup,
        node: "web".into(),
    };
    let error = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &setup,
            &prior,
            &BTreeMap::new(),
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_ref(), "execution.host_grant_required");
    assert!(!snapshot.path.join("setup-output").exists());
    store.grant_host_execution(instance.id).unwrap();
    let resource = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &setup,
            &prior,
            &BTreeMap::new(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resource.resource_kind,
        stackless_cloud::prepare::durable::KIND
    );
    assert_eq!(
        std::fs::read(snapshot.path.join("setup-output")).unwrap(),
        b"ready"
    );
    let recovered = substrate
        .execute(context(
            &store,
            &instance,
            &def,
            &setup,
            &prior,
            &BTreeMap::new(),
        ))
        .await
        .unwrap();
    assert_eq!(resource.payload, recovered.payload);
    let command_record = store
        .resources(instance.id)
        .unwrap()
        .into_iter()
        .find(|record| record.resource_kind == resource.resource_kind && record.step_id == setup.id)
        .unwrap();
    substrate
        .destroy_record(&store, &instance, &command_record)
        .await
        .unwrap();
    substrate.destroy(&instance, &prior[0]).await.unwrap();
    assert!(!snapshot.root.exists());
}
