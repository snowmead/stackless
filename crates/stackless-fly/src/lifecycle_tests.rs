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
const PROVIDER: &str = "prvdr_61UQUoA4NItxbLNIU5FF2";
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
    starts: usize,
    polls: usize,
    current_changed: bool,
    machine: Option<Value>,
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
                    "FLYIO_DEPLOY_TOKEN=test\n",
                )
                .unwrap();
                test_support::ok_empty()
            }
            "add" => {
                assert_eq!(args[1], "flyio/app");
                let name = args[3].clone();
                let record = self
                    .store
                    .resource(&owner.instance_id, &format!("catalog:flyio/app:{name}"))
                    .unwrap()
                    .unwrap();
                let value: Value = serde_json::from_str(&record.payload).unwrap();
                assert_eq!(value["_catalog_creation"]["submitted"], true);
                assert_eq!(record.phase, ResourcePhase::Intent);
                let mut remote = self.remote.lock().unwrap();
                assert_eq!(remote.creates, 0);
                remote.creates += 1;
                remote.resource = Some(name);
                remote.native_name = value["_catalog_creation"]["config"]["app_name"]
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
        let row = remote.resource.as_ref().map(|name| json!({"id":"remote_project", "name":name, "provider":PROVIDER, "service_ref":"app", "status":if remote.catalog_removed {"removed"} else {"complete"}}));
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

struct FixtureSubstrate<T>(T, String);
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
    fn step_revision(&self, ctx: &StepContext<'_>) -> Result<String, SubstrateFault> {
        self.0.step_revision(ctx)
    }
    fn refresh_each_operation(&self, step: &stackless_core::engine::Step) -> bool {
        self.0.refresh_each_operation(step)
    }
    async fn execute(&self, ctx: StepContext<'_>) -> Result<StepResource, SubstrateFault> {
        if ctx.step.kind == StepKind::HealthGate
            && ctx.def.services[&ctx.step.node].health.is_some()
        {
            stackless_cloud::health::poll(&self.1, 200, None, Duration::from_secs(1))
                .await
                .map_err(|e| fault(lifecycle::invalid(e.detail)))?;
            return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
        }
        self.0.execute(ctx).await
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

fn application(name: &str) -> Value {
    json!({"id":"app_1", "name":name, "organization":{"slug":"managed"}})
}

async fn engine_recovery(
    lost_post: bool,
    delayed_delete: bool,
    abandon_after_catalog: bool,
    worker: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let remote = Arc::new(StdMutex::new(Remote::default()));
    let server = MockServer::start().await;
    let state = remote.clone();
    let post_store = store.clone();
    Mock::given(method("POST"))
        .respond_with(move |http: &wiremock::Request| {
            if http.url.path().ends_with("/machines/machine_1/start") {
                assert!(worker);
                let native = recorded_native(&post_store);
                let pending: Vec<_> = native
                    .starts
                    .values()
                    .filter(|start| !start.observed_started)
                    .collect();
                assert_eq!(pending.len(), 1);
                assert_eq!(pending[0].machine_id, "machine_1");
                assert_eq!(pending[0].instance_id, "version_1");
                let mut state = state.lock().unwrap();
                state.starts += 1;
                state.machine.as_mut().unwrap()["state"] = json!("started");
                return ResponseTemplate::new(503);
            }
            assert!(http.url.path().ends_with("/machines"));
            let native = recorded_native(&post_store);
            assert_eq!(native.app.as_ref().unwrap().id, "app_1");
            let request = native.requests.values().next().unwrap();
            assert!(request.submitted);
            assert!(request.machine_id.is_none());
            let mut machine: Value = serde_json::from_slice(&http.body).unwrap();
            assert_eq!(
                fly_api::machine_receipt(&machine),
                Some(request.receipt.as_str())
            );
            assert_eq!(machine["config"]["image"], "hashicorp/http-echo");
            if worker {
                assert!(machine["config"]["env"].get("PORT").is_none());
                assert_eq!(machine["config"]["services"], json!([]));
                assert_eq!(machine["config"]["restart"]["policy"], "always");
                // Fly may omit the optional empty services field on readback.
                machine["config"]
                    .as_object_mut()
                    .unwrap()
                    .remove("services");
            } else {
                assert_eq!(machine["config"]["env"]["PORT"], "5678");
            }
            assert_eq!(
                machine["config"]["init"],
                json!({"exec":["/bin/sh", "-c", "exec server --port 5678"]})
            );
            machine["id"] = "machine_1".into();
            machine["instance_id"] = "version_1".into();
            machine["state"] = "started".into();
            machine["image_ref"] = json!({"digest":format!("sha256:{}", "a".repeat(64))});
            let mut state = state.lock().unwrap();
            state.posts += 1;
            state.machine = Some(machine.clone());
            if lost_post {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(machine)
            }
        })
        .expect(u64::from(!abandon_after_catalog) + u64::from(worker))
        .mount(&server)
        .await;
    let state = remote.clone();
    let delete_store = store.clone();
    Mock::given(method("DELETE"))
        .respond_with(move |http: &wiremock::Request| {
            assert!(
                http.url
                    .query_pairs()
                    .any(|(key, value)| key == "force" && value == "true")
            );
            assert!(recorded_native(&delete_store).removal_submitted);
            let mut state = state.lock().unwrap();
            assert_eq!(
                http.url.path(),
                format!("/apps/{}", state.native_name.as_deref().unwrap())
            );
            state.project_deletes += 1;
            if !delayed_delete {
                state.project_alive = false;
            }
            ResponseTemplate::new(503)
        })
        .expect(1)
        .mount(&server)
        .await;
    let state = remote.clone();
    let poll_store = store.clone();
    Mock::given(method("GET"))
        .respond_with(move |http: &wiremock::Request| {
            let mut state = state.lock().unwrap();
            if http.url.path() == "/" {
                assert!(
                    !worker,
                    "worker readiness must not issue HTTP health probes"
                );
                return ResponseTemplate::new(200);
            }
            let app = format!("/apps/{}", state.native_name.as_deref().unwrap());
            if http.url.path() == app {
                return if state.project_alive {
                    ResponseTemplate::new(200)
                        .set_body_json(application(state.native_name.as_deref().unwrap()))
                } else {
                    ResponseTemplate::new(404)
                };
            }
            if http.url.path() == format!("{app}/ip_assignments") {
                assert!(
                    !worker,
                    "workers without listeners must not allocate public IPs"
                );
                return ResponseTemplate::new(200)
                    .set_body_json(json!({"ips":[{"ip":"127.0.0.1"},{"ip":"::1"}]}));
            }
            if http.url.path() == format!("{app}/machines") {
                return ResponseTemplate::new(200)
                    .set_body_json(state.machine.iter().cloned().collect::<Vec<_>>());
            }
            if http.url.path() == format!("{app}/machines/machine_1/events") {
                assert_eq!(http.url.query(), Some("limit=10"));
                return ResponseTemplate::new(200).set_body_json(json!([{"type":"start", "status":"started", "source":"flyd", "timestamp":1234}]));
            }
            assert_eq!(http.url.path(), format!("{app}/machines/machine_1"));
            let native = recorded_native(&poll_store);
            assert_eq!(
                native
                    .requests
                    .values()
                    .next()
                    .unwrap()
                    .machine_id
                    .as_deref(),
                Some("machine_1")
            );
            state.polls += 1;
            if state.polls == 1 && !lost_post {
                return ResponseTemplate::new(503);
            }
            let mut machine = state.machine.clone().unwrap();
            if state.current_changed {
                machine["config"]["env"]["FOREIGN"] = "changed".into();
            }
            ResponseTemplate::new(200).set_body_json(machine)
        })
        .mount(&server)
        .await;
    let text = "[stack]\nname='project'\n[services.web]\nimage='hashicorp/http-echo'\nrun='exec server --port 5678'\nhealth={path='/'}\n[services.web.fly]\ninternal_port=5678\n";
    let text = if worker {
        text.replace("health={path='/'}", "kind='worker'")
    } else {
        text.into()
    };
    let def = StackDef::parse(&text).unwrap();
    let request = || UpRequest {
        instance: "demo",
        definition_text: &text,
        def: &def,
        source_overrides: BTreeMap::new(),
        dirty: false,
        definition_dir: dir.path().display().to_string(),
        lease: None,
        progress: None,
    };
    let make = |store: &Store| {
        let mut substrate = FlySubstrate::for_test(
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
            .insert("FLY_API_TOKEN".into(), "test".into());
        FixtureSubstrate(substrate, server.uri())
    };
    let substrate = make(&store);
    let engine = Engine {
        store: &store,
        substrate: &substrate,
    };
    let admission = engine.begin_up(&request()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap();
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
        let substrate = make(&store);
        let result = Engine {
            store: &store,
            substrate: &substrate,
        }
        .up(request())
        .await;
        result.unwrap();
        let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
        let context = InstanceContext::from_record(&owner, &[]);
        assert_eq!(
            substrate.observe(&context, &checkpoint).await.unwrap(),
            Observation::Present
        );
        if worker {
            assert!(substrate.service_origin(&def, &context, "web").is_empty());
            assert!(
                !substrate
                    .0
                    .namespace(&def, &context, &[])
                    .service_origins
                    .contains_key("web")
            );
            let payload: Value = serde_json::from_str(&checkpoint.payload).unwrap();
            assert_eq!(payload["origin"], "");
            let log_context =
                InstanceContext::from_record(&owner, std::slice::from_ref(&checkpoint));
            let logs = substrate
                .0
                .fetch_logs(&store, &def, &log_context, &["web".into()], 10)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(logs[0].source, "fly_events");
            assert!(logs[0].lines[0].contains("started"));

            remote.lock().unwrap().machine.as_mut().unwrap()["state"] = json!("stopped");
            assert!(matches!(
                substrate.observe(&context, &checkpoint).await.unwrap(),
                Observation::Drifted { .. }
            ));
            let error = substrate
                .0
                .health_gate(&def, &context, "web", std::slice::from_ref(&checkpoint))
                .await
                .unwrap_err();
            assert_eq!(error.code.as_ref(), codes::FLY_WORKER_NOT_READY);
            let error = Engine {
                store: &store,
                substrate: &substrate,
            }
            .up(request())
            .await
            .unwrap_err();
            assert!(error.to_string().contains("503"), "{error}");
            assert_eq!(remote.lock().unwrap().starts, 1);
            assert!(
                recorded_native(&store)
                    .starts
                    .values()
                    .any(|start| !start.observed_started)
            );
            let reopened = Store::open(&db).unwrap();
            let recovered = make(&reopened);
            Engine {
                store: &reopened,
                substrate: &recovered,
            }
            .up(request())
            .await
            .unwrap();
            assert_eq!(remote.lock().unwrap().starts, 1);
            assert!(
                recorded_native(&reopened)
                    .starts
                    .values()
                    .all(|start| start.observed_started)
            );

            substrate
                .0
                .health_gate(&def, &context, "web", std::slice::from_ref(&checkpoint))
                .await
                .unwrap();
        }
        remote.lock().unwrap().current_changed = true;
        assert!(matches!(
            substrate.observe(&context, &checkpoint).await.unwrap(),
            Observation::Drifted { .. }
        ));
    }
    let store = Store::open(&db).unwrap();
    let substrate = make(&store);
    let sibling = store
        .create_instance("sibling", "fly", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    let record = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "fly-machine")
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
        ResponseTemplate::new(200).set_body_json(application("foreign-app")),
    ] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/apps/{}",
                remote.lock().unwrap().native_name.as_deref().unwrap()
            )))
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
            .find(|r| r.resource_kind == "fly-machine")
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
        .find(|r| r.resource_kind == "fly-machine")
        .unwrap();
    let value: Value = serde_json::from_str(&record.payload).unwrap();
    serde_json::from_value(value["_fly"].clone()).unwrap()
}
#[tokio::test]
async fn known_machine_resumes_after_reopen_and_native_deletion_precedes_catalog() {
    engine_recovery(false, false, false, false).await;
}
#[tokio::test]
async fn lost_post_recovers_receipt_and_unconfirmed_delete_retains_ownership() {
    engine_recovery(true, true, false, false).await;
}

#[tokio::test]
async fn down_recovers_native_id_after_lost_catalog_creation_without_a_start_checkpoint() {
    engine_recovery(false, false, true, false).await;
}

#[tokio::test]
async fn worker_recovers_a_lost_machine_response_without_public_networking() {
    engine_recovery(true, true, false, true).await;
}

#[tokio::test]
async fn mixed_local_job_consumes_fly_outputs_and_tears_down_before_the_cloud_app() {
    use stackless_core::routing::RoutedSubstrate;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let remote = Arc::new(StdMutex::new(Remote::default()));
    let server = MockServer::start().await;
    let state = remote.clone();
    let journal = store.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            assert!(request.url.path().ends_with("/machines"));
            let native = recorded_native(&journal);
            assert!(native.requests.values().next().unwrap().submitted);
            let mut machine: Value = serde_json::from_slice(&request.body).unwrap();
            machine["id"] = "machine_1".into();
            machine["instance_id"] = "version_1".into();
            machine["state"] = "started".into();
            machine["image_ref"] = json!({"digest":format!("sha256:{}", "a".repeat(64))});
            let mut state = state.lock().unwrap();
            state.posts += 1;
            state.machine = Some(machine.clone());
            ResponseTemplate::new(200).set_body_json(machine)
        })
        .expect(1)
        .mount(&server)
        .await;
    let state = remote.clone();
    Mock::given(method("GET"))
        .respond_with(move |request: &wiremock::Request| {
            if request.url.path() == "/" {
                return ResponseTemplate::new(200);
            }
            let state = state.lock().unwrap();
            if !state.project_alive {
                return ResponseTemplate::new(404);
            }
            let app = format!("/apps/{}", state.native_name.as_deref().unwrap());
            if request.url.path() == app {
                return ResponseTemplate::new(200)
                    .set_body_json(application(state.native_name.as_deref().unwrap()));
            }
            if request.url.path() == format!("{app}/ip_assignments") {
                return ResponseTemplate::new(200)
                    .set_body_json(json!({"ips":[{"ip":"127.0.0.1"},{"ip":"::1"}]}));
            }
            if request.url.path() == format!("{app}/machines") {
                return ResponseTemplate::new(200)
                    .set_body_json(state.machine.iter().cloned().collect::<Vec<_>>());
            }
            assert_eq!(request.url.path(), format!("{app}/machines/machine_1"));
            ResponseTemplate::new(200).set_body_json(state.machine.clone().unwrap())
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    let journal = store.clone();
    Mock::given(method("DELETE"))
        .respond_with(move |request: &wiremock::Request| {
            let owner = journal.instance("demo").unwrap().unwrap();
            let jobs: Vec<_> = journal
                .resources(&owner.instance_id)
                .unwrap()
                .into_iter()
                .filter(|resource| resource.resource_kind == stackless_local::job::KIND)
                .collect();
            assert!(!jobs.is_empty());
            assert!(jobs.iter().all(|job| job.phase == ResourcePhase::Absent));
            let mut state = state.lock().unwrap();
            assert_eq!(
                request.url.path(),
                format!("/apps/{}", state.native_name.as_deref().unwrap())
            );
            state.project_alive = false;
            state.project_deletes += 1;
            ResponseTemplate::new(200)
        })
        .expect(1)
        .mount(&server)
        .await;
    let text = r#"
[stack]
name = "project"
[workloads.web]
on = "fly"
image = "hashicorp/http-echo"
run = "exec server --port 5678"
health = { path = "/" }
[workloads.web.fly]
internal_port = 5678
[jobs.check]
run = "test \"$API\" = \"$ENDPOINT\" && printf '%s' \"$API\""
env = { API = "${services.web.origin}", ENDPOINT = "${endpoints.api.url}" }
depends_on = { web = "ready" }
[endpoints.api]
workload = "web"
"#;
    let def = StackDef::parse(text).unwrap();
    let make = |store: &Store| {
        let mut fly = FlySubstrate::for_test(
            Runner {
                store: store.clone(),
                remote: remote.clone(),
            },
            dir.path(),
            server.uri(),
            true,
        );
        fly.secrets.insert("FLY_API_TOKEN".into(), "test".into());
        let local = stackless_local::LocalSubstrate {
            state_root: dir.path().join("local"),
            definition_dir: dir.path().into(),
            ..Default::default()
        };
        let recorded = store
            .instance("demo")
            .unwrap()
            .map(|owner| store.placements(&owner.instance_id).unwrap())
            .unwrap_or_default();
        let providers: BTreeMap<String, Box<dyn Substrate>> = BTreeMap::from([
            (
                "fly".into(),
                Box::new(FixtureSubstrate(fly, server.uri())) as Box<dyn Substrate>,
            ),
            ("local".into(), Box::new(local) as Box<dyn Substrate>),
        ]);
        RoutedSubstrate::new(
            Some(store.clone()),
            "local".into(),
            def.clone(),
            recorded,
            providers,
        )
        .unwrap()
    };
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
    let provider = make(&store);
    let engine = Engine {
        store: &store,
        substrate: &provider,
    };
    let admission = engine.begin_up(&request()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap();
    store.grant_host_execution(&owner.instance_id).unwrap();
    store
        .bind_stripe_project(&owner.instance_id, "project:test", Some("stripe_project"))
        .unwrap();
    assert!(
        engine
            .run_up(request(), admission)
            .await
            .unwrap_err()
            .to_string()
            .contains("lost catalog creation")
    );
    let reopened = Store::open(&db).unwrap();
    let provider = make(&reopened);
    let engine = Engine {
        store: &reopened,
        substrate: &provider,
    };
    engine.up(request()).await.unwrap();
    let cp = reopened.checkpoint("demo", "job:check").unwrap().unwrap();
    let job: stackless_local::job::JobCheckpoint = serde_json::from_str(&cp.payload).unwrap();
    assert_eq!(job.result().unwrap(), Some(0));
    let expected = format!(
        "https://{}.fly.dev",
        stackless_core::substrate::namespaced_resource_name(&owner.resource_namespace, "web")
    );
    assert_eq!(std::fs::read_to_string(&job.log_path).unwrap(), expected);
    assert_eq!(
        reopened.placements(&owner.instance_id).unwrap()["service:web"],
        "fly"
    );
    assert_eq!(
        reopened.placements(&owner.instance_id).unwrap()["service:check"],
        "local"
    );
    engine.up(request()).await.unwrap();
    assert_eq!(
        reopened
            .checkpoint("demo", "job:check")
            .unwrap()
            .unwrap()
            .resource_id,
        cp.resource_id
    );
    assert_eq!(remote.lock().unwrap().posts, 1);
    // The fake catalog loses its removal response after native deletion.
    assert!(engine.down("demo").await.is_err());
    engine.down("demo").await.unwrap();
    assert!(
        reopened
            .resources(&owner.instance_id)
            .unwrap()
            .iter()
            .all(|resource| resource.phase == ResourcePhase::Absent)
    );
    assert_eq!(remote.lock().unwrap().project_deletes, 1);
    assert_eq!(remote.lock().unwrap().catalog_deletes, 1);
}
