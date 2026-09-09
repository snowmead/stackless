//! Exercise the catalog, native deployment journal, engine resume, and teardown together.
use super::*;
use serde_json::{Value, json};
use stackless_core::engine::{Engine, UpRequest};
use stackless_core::state::{ResourcePhase, Store};
use stackless_stripe_projects::CommandOutput;
use std::sync::{Arc, Mutex as StdMutex};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, path_regex},
};

const CATALOG: &str = include_str!("../../stackless-stripe-projects/tests/fixtures/catalog.json");
const PROVIDER_ID: &str = "prvdr_61ULUAr4zlQgZkvXY5LsG";

#[derive(Default)]
struct Remote {
    name: Option<String>,
    project_removed: bool,
    creates: usize,
    receipt: Option<String>,
    deployment_alive: bool,
    deploys: usize,
    deletes: usize,
}
struct Runner {
    store: Store,
    remote: Arc<StdMutex<Remote>>,
}
fn ok(value: Value) -> CommandOutput {
    stackless_stripe_projects::test_support::ok(value)
}
#[async_trait]
impl CommandRunner for Runner {
    async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        let owner = self.store.instance("demo").unwrap().unwrap();
        let mut remote = self.remote.lock().unwrap();
        Ok(match args[0].as_str() {
            "catalog" => stackless_stripe_projects::test_support::raw(CATALOG),
            "status" => ok(json!({"project":{"id":"stripe_project"}})),
            "services" => ok(
                json!({"services": [], "plans":[{"name":"hobby","provider_name":"Vercel","service_id":"hobby"}]}),
            ),
            "env" if args.get(1).map(String::as_str) == Some("list") => {
                ok(json!({"environments":[{"name":owner.resource_namespace}]}))
            }
            "env" => ok(json!({})),
            "add" => {
                assert_eq!(args[1], "vercel/project");
                let name = args[3].clone();
                let key = format!("catalog:vercel/project:{name}");
                let record = self
                    .store
                    .resource(&owner.instance_id, &key)
                    .unwrap()
                    .unwrap();
                let value: Value = serde_json::from_str(&record.payload).unwrap();
                assert_eq!(record.phase, ResourcePhase::Intent);
                assert_eq!(value["_catalog_creation"]["submitted"], true);
                assert_eq!(remote.creates, 0);
                remote.creates += 1;
                remote.name = Some(name);
                ok(json!({"service":{"key":"remote_project","provider_id":PROVIDER_ID}}))
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
            assert!(path.ends_with("/remote_project/remove"));
            assert!(
                !remote.deployment_alive,
                "project removal must follow deployment deletion"
            );
            remote.project_removed = true;
        }
        let row = remote.name.as_ref().map(|name| json!({"id":"remote_project","name":name,"provider":PROVIDER_ID,"service_ref":"project","status":if remote.project_removed {"removed"} else {"complete"}}));
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
async fn engine_resumes_lost_deployment_response_then_deletes_children_before_catalog_project() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let commit = stackless_git::build_repo(&repo, &[&[("index.html", "source snapshot")]]).unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let remote = Arc::new(StdMutex::new(Remote::default()));
    let server = MockServer::start().await;
    let project_state = remote.clone();
    Mock::given(method("GET"))
        .and(path_regex("/v9/projects/.*"))
        .respond_with(move |_: &wiremock::Request| {
            let state = project_state.lock().unwrap();
            assert!(state.name.is_some());
            if state.project_removed {
                ResponseTemplate::new(404)
            } else {
                ResponseTemplate::new(200).set_body_json(json!({"id":"prj_one","name":state.name}))
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let post_state = remote.clone();
    Mock::given(method("POST"))
        .and(path("/v13/deployments"))
        .respond_with(move |request: &wiremock::Request| {
            let value: Value = request.body_json().unwrap();
            assert_eq!(value["gitSource"]["ref"], "main");
            assert_eq!(value["gitSource"]["sha"], commit);
            let mut state = post_state.lock().unwrap();
            state.receipt = value["meta"]["stacklessReceipt"]
                .as_str()
                .map(str::to_owned);
            assert!(state.receipt.is_some());
            state.deployment_alive = true;
            state.deploys += 1;
            ResponseTemplate::new(500)
        })
        .expect(1)
        .mount(&server)
        .await;
    let list_state = remote.clone();
    let origin = server.uri();
    let list_origin = origin.clone();
    Mock::given(method("GET")).and(path("/v7/deployments"))
        .respond_with(move |_: &wiremock::Request| {
            let state = list_state.lock().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"pagination":{"next":null},"deployments":[{
                "uid":"dpl_one","projectId":"prj_one","url":list_origin,"readyState":"READY","meta":{"stacklessReceipt":state.receipt}
            }]}))
        }).mount(&server).await;
    let get_state = remote.clone();
    Mock::given(method("GET")).and(path("/v13/deployments/dpl_one"))
        .respond_with(move |_: &wiremock::Request| {
            let state = get_state.lock().unwrap();
            if !state.deployment_alive { ResponseTemplate::new(404) } else {
                ResponseTemplate::new(200).set_body_json(json!({"id":"dpl_one","projectId":"prj_one","url":origin,"readyState":"READY","meta":{"stacklessReceipt":state.receipt}}))
            }
        }).mount(&server).await;
    let delete_state = remote.clone();
    Mock::given(method("DELETE"))
        .and(path("/v13/deployments/dpl_one"))
        .respond_with(move |_: &wiremock::Request| {
            let mut state = delete_state.lock().unwrap();
            state.deletes += 1;
            state.deployment_alive = false;
            ResponseTemplate::new(200)
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let text = "[stack]\nname='project'\n[services.web]\nsource={repo='https://github.com/acme/web',ref='main'}\nhealth={path='/'}\n[services.web.vercel]\n";
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
    let runner = Runner {
        store: store.clone(),
        remote: remote.clone(),
    };
    let mut substrate = VercelSubstrate::for_test(runner, dir.path(), server.uri(), false);
    substrate
        .secrets
        .insert("VERCEL_TOKEN".into(), "test".into());
    let substrate = FixtureSubstrate(substrate, repo.clone());
    let engine = Engine {
        store: &store,
        substrate: &substrate,
    };
    let admission = engine.begin_up(&request()).unwrap();
    let owner = store.instance("demo").unwrap().unwrap();
    store
        .bind_stripe_project(&owner.instance_id, "project:test", Some("stripe_project"))
        .unwrap();
    assert!(engine.run_up(request(), admission).await.is_err());
    assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
    drop(substrate);
    drop(store);
    let store = Store::open(&db).unwrap();
    let runner = Runner {
        store: store.clone(),
        remote: remote.clone(),
    };
    let mut substrate = VercelSubstrate::for_test(runner, dir.path(), server.uri(), false);
    substrate
        .secrets
        .insert("VERCEL_TOKEN".into(), "test".into());
    let substrate = FixtureSubstrate(substrate, repo.clone());
    let engine = Engine {
        store: &store,
        substrate: &substrate,
    };
    engine.up(request()).await.unwrap();
    let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let value: Value = serde_json::from_str(&checkpoint.payload).unwrap();
    assert_eq!(value["project_id"], "prj_one");
    assert_eq!(value["_catalog_creation"]["remote_id"], "remote_project");
    engine.down("demo").await.unwrap();
    assert!(
        store
            .resources(&owner.instance_id)
            .unwrap()
            .iter()
            .all(|record| record.phase == ResourcePhase::Absent)
    );
    {
        let state = remote.lock().unwrap();
        assert_eq!(state.creates, 1);
        assert_eq!(state.deploys, 1);
        assert_eq!(state.deletes, 1);
        assert!(state.project_removed);
    }
    server.verify().await;
}
