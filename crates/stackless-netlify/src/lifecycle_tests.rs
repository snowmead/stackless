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
const PROVIDER: &str = "prvdr_61UXju1taz9UCvkk95GEK";
#[derive(Default)]
struct Remote {
    resource: Option<String>,
    native_name: Option<String>,
    service_alive: bool,
    catalog_removed: bool,
    receipt: Option<String>,
    uploaded: bool,
    creates: usize,
    deploys: usize,
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
                assert_eq!(args[1], "netlify/project");
                let name = args[3].clone();
                let record = self
                    .store
                    .resource(
                        &owner.instance_id,
                        &format!("catalog:netlify/project:{name}"),
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
                ok(
                    json!({"service":{"key":"remote_service","provider_id":PROVIDER},"NETLIFY_AUTH_TOKEN":"test","NETLIFY_SITE_ID":"site_one"}),
                )
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
                !remote.service_alive,
                "native deletion precedes catalog removal"
            );
            remote.catalog_removed = true;
        }
        let row = remote.resource.as_ref().map(|name| json!({"id":"remote_service","name":name,"provider":PROVIDER,"service_ref":"project","status":if remote.catalog_removed {"removed"} else {"complete"}}));
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
async fn engine_recovers_lost_upload_and_delete_responses_without_duplicate_mutations() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(
        &repo,
        &[&[("index.html", "hello"), (".env", "private-canary")]],
    )
    .unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let remote = Arc::new(StdMutex::new(Remote::default()));
    let server = MockServer::start().await;
    let state = remote.clone();
    let origin = server.uri();
    Mock::given(method("GET"))
        .and(path("/sites/site_one"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            if state.service_alive {
                ResponseTemplate::new(200).set_body_json(
                    json!({"id":"site_one","name":state.native_name,"ssl_url":origin}),
                )
            } else {
                ResponseTemplate::new(404)
            }
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    let post_store = store.clone();
    Mock::given(method("POST"))
        .and(path("/sites/site_one/deploys"))
        .respond_with(move |request: &wiremock::Request| {
            let receipt = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "title")
                .unwrap()
                .1
                .into_owned();
            let value: Value = request.body_json().unwrap();
            assert_eq!(
                value["files"],
                json!({"/index.html":"aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"})
            );
            let owner = post_store.instance("demo").unwrap().unwrap();
            let record = post_store
                .resources(&owner.instance_id)
                .unwrap()
                .into_iter()
                .find(|r| r.resource_kind == "netlify-site")
                .unwrap();
            let value: Value = serde_json::from_str(&record.payload).unwrap();
            let native: lifecycle::NativeState =
                serde_json::from_value(value["_netlify"].clone()).unwrap();
            assert!(native.requests[&receipt].submitted);
            assert_eq!(native.site_id.as_deref(), Some("site_one"));
            let mut state = state.lock().unwrap();
            state.deploys += 1;
            state.receipt = Some(receipt);
            ResponseTemplate::new(500)
        })
        .expect(1)
        .mount(&server)
        .await;
    let deployment = |state: &Remote, origin: &str| json!({"id":"dep_one","site_id":"site_one","title":state.receipt,"state":if state.uploaded {"ready"} else {"uploading"},"required":if state.uploaded {vec![]} else {vec!["aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"]},"ssl_url":origin});
    let state = remote.clone();
    let origin = server.uri();
    Mock::given(method("GET"))
        .and(path("/sites/site_one/deploys"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            ResponseTemplate::new(200).set_body_json(if state.deploys > 0 {
                vec![deployment(&state, &origin)]
            } else {
                vec![]
            })
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    let origin = server.uri();
    Mock::given(method("GET"))
        .and(path("/sites/site_one/deploys/dep_one"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(deployment(&state.lock().unwrap(), &origin))
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    Mock::given(method("PUT"))
        .and(path("/deploys/dep_one/files/index.html"))
        .respond_with(move |request: &wiremock::Request| {
            assert_eq!(request.body, b"hello");
            state.lock().unwrap().uploaded = true;
            ResponseTemplate::new(200).set_body_json(json!({}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let state = remote.clone();
    let delete_store = store.clone();
    Mock::given(method("DELETE"))
        .and(path("/sites/site_one"))
        .respond_with(move |_: &wiremock::Request| {
            let owner = delete_store.instance("demo").unwrap().unwrap();
            let record = delete_store
                .resources(&owner.instance_id)
                .unwrap()
                .into_iter()
                .find(|r| r.resource_kind == "netlify-site")
                .unwrap();
            let payload: Value = serde_json::from_str(&record.payload).unwrap();
            assert_eq!(payload["_netlify"]["removal_submitted"], true);
            let mut state = state.lock().unwrap();
            assert!(state.service_alive);
            state.service_alive = false;
            state.deletes += 1;
            ResponseTemplate::new(500)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let text = "[stack]\nname='project'\n[services.web]\nsource={repo='https://example.invalid/source',ref='main'}\nhealth={path='/'}\n[services.web.netlify]\n";
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
        let mut substrate = NetlifySubstrate::for_test(
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
            .insert("NETLIFY_AUTH_TOKEN".into(), "test".into());
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
    let failure = engine.run_up(request(), admission).await.unwrap_err();
    assert!(failure.to_string().contains("500"), "{failure}");
    assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
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
    let payload: NetlifyPayload = serde_json::from_str(&checkpoint.payload).unwrap();
    assert_eq!(payload.deploy_id, "dep_one");
    assert_eq!(payload.origin, server.uri());
    assert!(engine.down("demo").await.is_err());
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
        assert_eq!((state.creates, state.deploys, state.deletes), (1, 1, 1));
        assert!(state.catalog_removed);
    }
    server.verify().await;
}
