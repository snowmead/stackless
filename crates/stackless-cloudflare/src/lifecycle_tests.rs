//! Two instance Workers share account enablement without sharing deletion authority.
use super::*;
use serde_json::{Value, json};
use stackless_core::engine::{Engine, UpRequest};
use stackless_core::state::{Ownership, ResourcePhase, Store};
use stackless_stripe_projects::CommandOutput;
use std::sync::{Arc, Mutex as StdMutex};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path_regex},
};

const CATALOG: &str = include_str!("../../stackless-stripe-projects/tests/fixtures/catalog.json");
const PROVIDER: &str = "prvdr_61UQRSWMdrciFdBNs5Ipc";
#[derive(Default)]
struct Remote {
    registrations: BTreeMap<String, String>,
    workers: BTreeMap<String, Value>,
    deleted: Vec<String>,
}
struct Runner {
    store: Store,
    remote: Arc<StdMutex<Remote>>,
}
fn ok(value: Value) -> CommandOutput {
    test_support::ok(value)
}
use stackless_stripe_projects::test_support;

#[async_trait]
impl CommandRunner for Runner {
    async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        Ok(match args[0].as_str() {
            "catalog" => test_support::raw(CATALOG),
            "status" => ok(json!({"project":{"id":"stripe_project"}})),
            "services" => ok(
                json!({"services":[],"plans":[{"name":"workers:free","service_id":"workers:free","provider_name":"Cloudflare"}]}),
            ),
            "env" if args.get(1).map(String::as_str) == Some("list") => ok(
                json!({"environments": self.store.instances().unwrap().iter().map(|record| json!({"name":record.resource_namespace})).collect::<Vec<_>>()}),
            ),
            "env" => ok(json!({})),
            "add" => {
                assert_eq!(args[1], "cloudflare/workers");
                let name = &args[3];
                let mut remote = self.remote.lock().unwrap();
                assert!(!remote.registrations.contains_key(name));
                let id = format!("remote_{}", remote.registrations.len());
                remote.registrations.insert(name.clone(), id.clone());
                ok(
                    json!({"service":{"key":id,"provider_id":PROVIDER},"CLOUDFLARE_ACCOUNT_ID":"acc_one","CLOUDFLARE_WORKERS_DEV_SUBDOMAIN":"shared"}),
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
        assert_eq!(
            method, "GET",
            "instance teardown must not remove account enablement"
        );
        let remote = self.remote.lock().unwrap();
        let rows: Vec<Value> = remote.registrations.iter().map(|(name,id)| json!({"id":id,"name":name,"provider":PROVIDER,"service_ref":"workers","status":"complete"})).collect();
        let value = if path.contains('?') {
            json!({"data":rows,"next_page_url":null})
        } else {
            rows.into_iter()
                .find(|row| path.ends_with(row["id"].as_str().unwrap()))
                .unwrap()
        };
        Ok(CommandOutput {
            status: 0,
            stdout: value.to_string(),
            stderr: String::new(),
        })
    }
}

// HTTP health is tested separately. This test covers native API ownership and teardown.
struct NativeLifecycle<T>(T, PathBuf);
#[async_trait]
impl<T: Substrate> Substrate for NativeLifecycle<T> {
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
        if ctx.step.kind == StepKind::HealthGate {
            return Ok(stackless_core::substrate::action_resource(&ctx.step.id));
        }
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
async fn down_deletes_only_the_owned_worker_and_retains_shared_account_enablement() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(
        &repo,
        &[&[(
            "worker.mjs",
            "export default {async fetch(){return new Response('ok')}}",
        )]],
    )
    .unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    let remote = Arc::new(StdMutex::new(Remote::default()));
    let server = MockServer::start().await;
    let put_remote = remote.clone();
    let put_store = store.clone();
    Mock::given(method("PUT")).and(path_regex("/accounts/acc_one/workers/scripts/[^/]+$"))
        .respond_with(move |request: &wiremock::Request| {
            let body = String::from_utf8_lossy(&request.body);
            let metadata: Value = serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap().split("\r\n--").next().unwrap()).unwrap();
            let owner = metadata["tags"][0].as_str().unwrap().strip_prefix("stackless-owner:").unwrap();
            let name = request.url.path().split('/').nth(5).unwrap().to_owned();
            let record = put_store.resource(owner, &format!("cloudflare-script:acc_one:{name}")).unwrap().unwrap();
            assert_eq!(record.phase, ResourcePhase::Intent);
            assert_eq!(serde_json::from_str::<Value>(&record.payload).unwrap()["submitted_revision"], metadata["annotations"]["workers/tag"]);
            let settings = json!({"tags":metadata["tags"],"annotations":metadata["annotations"],"script":{"etag":"etag"}});
            assert!(put_remote.lock().unwrap().workers.insert(name.clone(), settings).is_none());
            ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":{"id":name,"etag":"etag"}}))
        }).expect(2).mount(&server).await;
    let get_remote = remote.clone();
    Mock::given(method("GET"))
        .and(path_regex(
            "/accounts/acc_one/workers/scripts/[^/]+/settings$",
        ))
        .respond_with(move |request: &wiremock::Request| {
            let name = request.url.path().split('/').nth(5).unwrap();
            match get_remote.lock().unwrap().workers.get(name) {
                Some(settings) => ResponseTemplate::new(200)
                    .set_body_json(json!({"success":true,"result":settings})),
                None => ResponseTemplate::new(404),
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(
            "/accounts/acc_one/workers/scripts/[^/]+/subdomain$",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"success":true,"result":{"enabled":true}})),
        )
        .mount(&server)
        .await;
    let delete_remote = remote.clone();
    Mock::given(method("DELETE"))
        .and(path_regex("/accounts/acc_one/workers/scripts/[^/]+$"))
        .respond_with(move |request: &wiremock::Request| {
            let name = request.url.path().split('/').nth(5).unwrap().to_owned();
            let mut remote = delete_remote.lock().unwrap();
            assert!(remote.workers.remove(&name).is_some());
            remote.deleted.push(name);
            ResponseTemplate::new(200).set_body_json(json!({"success":true,"result":null}))
        })
        .expect(2)
        .mount(&server)
        .await;
    let text = format!(
        "[stack]\nname='project'\n[services.web]\nsource={{repo={:?},ref='main'}}\nhealth={{path='/'}}\n[services.web.cloudflare]\n",
        "https://example.invalid/source"
    );
    let def = StackDef::parse(&text).unwrap();
    let make = || {
        let mut substrate = CloudflareSubstrate::for_test(
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
            .insert("CLOUDFLARE_API_TOKEN".into(), "test".into());
        NativeLifecycle(substrate, repo.clone())
    };
    for name in ["one", "two"] {
        let substrate = make();
        let engine = Engine {
            store: &store,
            substrate: &substrate,
        };
        let request = UpRequest {
            instance: name,
            definition_text: &text,
            def: &def,
            source_overrides: BTreeMap::new(),
            dirty: false,
            definition_dir: dir.path().display().to_string(),
            lease: None,
            progress: None,
        };
        let admission = engine.begin_up(&request).unwrap();
        let owner = store.instance(name).unwrap().unwrap();
        store
            .bind_stripe_project(&owner.instance_id, "project:test", Some("stripe_project"))
            .unwrap();
        engine.run_up(request, admission).await.unwrap();
        assert!(
            store
                .resources(&owner.instance_id)
                .unwrap()
                .iter()
                .any(|record| record.resource_kind == lifecycle::CATALOG_KIND
                    && record.ownership == Ownership::Shared)
        );
    }
    let two = store.instance("two").unwrap().unwrap();
    let two_name = InstanceContext::from_record(&two, &[]).resource_name("web");
    let substrate = make();
    let engine = Engine {
        store: &store,
        substrate: &substrate,
    };
    engine.down("one").await.unwrap();
    {
        let remote = remote.lock().unwrap();
        assert_eq!(remote.workers.len(), 1);
        assert!(remote.workers.contains_key(&two_name));
        assert_eq!(remote.registrations.len(), 2);
    }
    let checkpoints = store.checkpoints("two").unwrap();
    let context = InstanceContext::from_record(&two, &checkpoints);
    assert_eq!(
        substrate
            .observe(
                &context,
                checkpoints
                    .iter()
                    .find(|c| c.step_id == "start:web")
                    .unwrap()
            )
            .await
            .unwrap(),
        Observation::Present
    );
    engine.down("two").await.unwrap();
    {
        let remote = remote.lock().unwrap();
        assert!(remote.workers.is_empty());
        assert_eq!(remote.deleted.len(), 2);
        assert_eq!(remote.registrations.len(), 2);
    }
    server.verify().await;
}
