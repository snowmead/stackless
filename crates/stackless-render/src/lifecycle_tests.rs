//! A lost deployment response must not cause another service or deployment.
use super::*;
use serde_json::{Value, json};
use stackless_core::engine::{Engine, UpRequest};
use stackless_core::state::{ResourcePhase, Store};
use stackless_stripe_projects::{CommandOutput, test_support};
use std::sync::{Arc, Mutex as StdMutex};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const CATALOG: &str = include_str!("../../stackless-stripe-projects/tests/fixtures/catalog.json");
const PROVIDER: &str = "prvdr_61UVwArXdJpejOfkn5POi";
#[derive(Default)]
struct Remote {
    source_root: String,
    resource: Option<String>,
    native_name: Option<String>,
    service_alive: bool,
    catalog_removed: bool,
    catalog_deletes_native: bool,
    commit: Option<String>,
    creates: usize,
    deploys: usize,
    delayed_receipt_reads: usize,
    delayed_deletion_reads: usize,
    deletes: usize,
}
struct Runner {
    store: Store,
    remote: Arc<StdMutex<Remote>>,
}
fn ok(value: Value) -> CommandOutput {
    test_support::ok(value)
}
#[async_trait]
impl CommandRunner for Runner {
    async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        let owner = self.store.instance("demo").unwrap().unwrap();
        Ok(match args[0].as_str() {
            "catalog" => test_support::raw(CATALOG),
            "status" => ok(json!({"project":{"id":"stripe_project"}})),
            "services" => ok(json!({"services":[],"plans":[]})),
            "env" if args.get(1).map(String::as_str) == Some("list") => {
                ok(json!({"environments":[{"name":owner.resource_namespace}]}))
            }
            "env" => ok(json!({})),
            "add" => {
                assert_eq!(args[1], "render/web-service");
                let name = args[3].clone();
                let record = self
                    .store
                    .resource(
                        &owner.instance_id,
                        &format!("catalog:render/web-service:{name}"),
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
                remote.service_alive = true;
                ok(json!({"service":{"key":"remote_service","provider_id":PROVIDER}}))
            }
            other => panic!("unexpected Stripe command {other}"),
        })
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        _cwd: &Path,
    ) -> Result<CommandOutput, ProjectsError> {
        let mut remote = self.remote.lock().unwrap();
        if method == "POST" {
            assert!(path.ends_with("/remote_service/remove"));
            assert!(
                remote.service_alive,
                "catalog removal needs the native service"
            );
            remote.catalog_removed = true;
            if remote.catalog_deletes_native {
                remote.service_alive = false;
            }
        }
        let row = remote.resource.as_ref().map(|name| json!({"id":"remote_service","name":name,"provider":PROVIDER,"service_ref":"web-service","status":if remote.catalog_removed {"removed"} else {"complete"}}));
        let value = if method == "POST" {
            json!({"status":"removed"})
        } else if path.contains('?') {
            json!({"data":row.into_iter().collect::<Vec<_>>(),"next_page_url":null})
        } else {
            row.unwrap()
        };
        Ok(CommandOutput {
            status: 0,
            stdout: value.to_string(),
            stderr: String::new(),
        })
    }
}

// The engine admits a remote Git URL. Start receives a local fixture repo.
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

#[tokio::test]
async fn engine_recovers_lost_deploy_and_delete_responses_without_duplicate_mutations() {
    run_deployment_recovery(false, false).await;
}

#[tokio::test]
async fn engine_waits_for_queued_deployment_and_asynchronous_deletion() {
    run_deployment_recovery(true, false).await;
}

#[tokio::test]
async fn engine_accepts_native_deletion_by_catalog_connector() {
    run_deployment_recovery(true, true).await;
}

async fn run_deployment_recovery(queued: bool, catalog_deletes_native: bool) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let commit = stackless_git::build_repo(&repo, &[&[("app/app.txt", "revision one")]]).unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let remote = Arc::new(StdMutex::new(Remote {
        catalog_deletes_native,
        ..Default::default()
    }));
    let server = MockServer::start().await;
    let state = remote.clone();
    Mock::given(method("GET"))
        .and(path("/services"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            let rows = if state.service_alive {
                vec![json!({"cursor":"c1","service":{"id":"srv_one","name":state.native_name}})]
            } else {
                vec![]
            };
            ResponseTemplate::new(200).set_body_json(rows)
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    let origin = server.uri();
    Mock::given(method("GET")).and(path("/services/srv_one"))
        .respond_with(move |_: &wiremock::Request| {
            let mut state = state.lock().unwrap();
            if state.deletes > 0 {
                if state.delayed_deletion_reads > 0 {
                    state.delayed_deletion_reads -= 1;
                } else {
                    state.service_alive = false;
                }
            }
            if state.service_alive { ResponseTemplate::new(200).set_body_json(json!({"id":"srv_one","name":state.native_name,"serviceDetails":{"url":origin},"autoDeploy":"no","rootDir":state.source_root})) }
            else { ResponseTemplate::new(404) }
        }).mount(&server).await;
    let state = remote.clone();
    Mock::given(method("PATCH"))
        .and(path("/services/srv_one"))
        .respond_with(move |request: &wiremock::Request| {
            assert_eq!(
                request.body_json::<Value>().unwrap(),
                json!({"autoDeploy":"no","rootDir":"app"})
            );
            let mut state = state.lock().unwrap();
            state.source_root = "app".into();
            ResponseTemplate::new(200).set_body_json(
                json!({"id":"srv_one","name":state.native_name,"autoDeploy":"no","rootDir":state.source_root}),
            )
        })
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/services/srv_one/env-vars"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let state = remote.clone();
    let post_store = store.clone();
    let expected_commit = commit.clone();
    Mock::given(method("POST"))
        .and(path("/services/srv_one/deploys"))
        .respond_with(move |request: &wiremock::Request| {
            let value: Value = request.body_json().unwrap();
            assert_eq!(value["commitId"], expected_commit);
            let owner = post_store.instance("demo").unwrap().unwrap();
            let resource = post_store
                .resources(&owner.instance_id)
                .unwrap()
                .into_iter()
                .find(|r| r.resource_kind == "render-service")
                .unwrap();
            let payload: ServicePayload = serde_json::from_str(&resource.payload).unwrap();
            let attempt = payload.deployments.last().unwrap();
            assert!(attempt.submitted);
            assert_eq!(attempt.commit, expected_commit);
            assert_eq!(attempt.before.len(), 1);
            assert!(attempt.before.contains("dep_initial"));
            let mut state = state.lock().unwrap();
            state.deploys += 1;
            state.commit = Some(expected_commit.clone());
            state.delayed_receipt_reads = if queued { 2 } else { 0 };
            ResponseTemplate::new(if queued { 202 } else { 500 })
        })
        .expect(1)
        .mount(&server)
        .await;
    let state = remote.clone();
    Mock::given(method("GET")).and(path("/services/srv_one/deploys"))
        .respond_with(move |_: &wiremock::Request| {
            let mut state = state.lock().unwrap();
            let mut rows = vec![json!({"cursor":"old","deploy":{"id":"dep_initial","status":"build_failed","commit":{"id":state.commit},"trigger":"service_created"}})];
            if state.delayed_receipt_reads > 0 {
                state.delayed_receipt_reads -= 1;
            } else if state.deploys > 0 { rows.push(json!({"cursor":"new","deploy":{"id":"dep_one","status":"live","commit":{"id":state.commit},"trigger":"api"}})); }
            ResponseTemplate::new(200).set_body_json(rows)
        }).mount(&server).await;
    let state = remote.clone();
    Mock::given(method("GET")).and(path("/services/srv_one/deploys/dep_one"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"id":"dep_one","status":"live","commit":{"id":state.commit},"trigger":"api"}))
        }).mount(&server).await;
    let state = remote.clone();
    let delete_store = store.clone();
    Mock::given(method("DELETE"))
        .and(path("/services/srv_one"))
        .respond_with(move |_: &wiremock::Request| {
            let owner = delete_store.instance("demo").unwrap().unwrap();
            let record = delete_store
                .resources(&owner.instance_id)
                .unwrap()
                .into_iter()
                .find(|r| r.resource_kind == "render-service")
                .unwrap();
            let payload: Value = serde_json::from_str(&record.payload).unwrap();
            assert_eq!(payload["removal_submitted"], true);
            let mut state = state.lock().unwrap();
            assert!(state.service_alive);
            assert!(
                state.catalog_removed,
                "catalog removal precedes native DELETE"
            );
            state.service_alive = queued;
            state.delayed_deletion_reads = if queued { 2 } else { 0 };
            state.deletes += 1;
            ResponseTemplate::new(if queued { 204 } else { 500 })
        })
        .expect(if catalog_deletes_native { 0 } else { 1 })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let text = "[stack]\nname='project'\n[services.web]\nsource={repo='https://example.invalid/source',ref='main',root='app'}\nhealth={path='/'}\n[services.web.render]\nruntime='node'\nbuild='true'\nstart='node app.js'\n";
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
        let mut substrate = RenderSubstrate::for_test(
            Runner {
                store: store.clone(),
                remote: remote.clone(),
            },
            dir.path(),
            server.uri(),
            false,
        );
        substrate
            .secrets
            .insert("RENDER_API_KEY".into(), "test".into());
        FixtureSubstrate(substrate, repo.clone())
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
    let result = engine.run_up(request(), admission).await;
    if queued {
        result.unwrap();
        assert_eq!(remote.lock().unwrap().delayed_receipt_reads, 0);
    } else {
        let failure = result.unwrap_err();
        assert!(failure.to_string().contains("500"), "{failure}");
        assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
    }
    drop(substrate);
    drop(store);
    let store = Store::open(&db).unwrap();
    let substrate = make(&store);
    let engine = Engine {
        store: &store,
        substrate: &substrate,
    };
    engine.up(request()).await.unwrap();
    let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let payload: ServicePayload = serde_json::from_str(&checkpoint.payload).unwrap();
    assert_eq!(
        payload.deployments.last().unwrap().id.as_deref(),
        Some("dep_one")
    );
    assert_eq!(payload.deployments.last().unwrap().commit, commit);
    assert_eq!(payload.origin, server.uri());
    let down = engine.down("demo").await;
    if queued {
        down.unwrap();
        assert_eq!(remote.lock().unwrap().delayed_deletion_reads, 0);
    } else {
        assert!(down.is_err());
    }
    drop(substrate);
    drop(store);
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
    {
        let state = remote.lock().unwrap();
        assert_eq!((state.creates, state.deploys), (1, 1));
        assert_eq!(state.deletes, usize::from(!catalog_deletes_native));
        assert!(state.catalog_removed);
        assert!(!state.service_alive);
    }
    server.verify().await;
}
