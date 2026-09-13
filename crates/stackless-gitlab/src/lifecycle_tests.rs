//! Engine recovery crosses the catalog, commit, serving, and teardown boundaries.
use super::*;
use base64::Engine as _;
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
    matchers::{method, path, path_regex},
};

const CATALOG: &str = include_str!("../../stackless-stripe-projects/tests/fixtures/catalog.json");
const PROVIDER: &str = "prvdr_61UU5TYtG0f1neJCz5EwK";
const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
#[derive(Default)]
struct Remote {
    resource: Option<String>,
    native_name: Option<String>,
    project_alive: bool,
    pages_alive: bool,
    catalog_removed: bool,
    receipt: Option<String>,
    serving_receipt: Option<String>,
    creates: usize,
    commits: usize,
    pages_deletes: usize,
    project_deletes: usize,
    catalog_deletes: usize,
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
            "catalog" => test_support::raw(CATALOG),
            "status" => test_support::ok(json!({"project":{"id":"stripe_project"}})),
            "services" => test_support::services(&[]),
            "env" if args.get(1).map(String::as_str) == Some("list") => {
                test_support::ok(json!({"environments":[{"name":owner.resource_namespace}]}))
            }
            "env" => {
                std::fs::write(
                    cwd.join(format!(".env.{}", owner.resource_namespace)),
                    "GITLAB_PROJECT_ID=42\nGITLAB_TOKEN=test\n",
                )
                .unwrap();
                test_support::ok_empty()
            }
            "add" => {
                assert_eq!(args[1], "gitlab/project");
                let name = args[3].clone();
                let record = self
                    .store
                    .resource(
                        &owner.instance_id,
                        &format!("catalog:gitlab/project:{name}"),
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
            assert!(!remote.project_alive && !remote.pages_alive);
            remote.catalog_removed = true;
            remote.catalog_deletes += 1;
            return Err(ProjectsError::Unavailable {
                detail: "lost catalog removal response".into(),
            });
        }
        let row = remote.resource.as_ref().map(|name| json!({"id":"remote_project", "name":name, "provider":PROVIDER, "service_ref":"project", "status":if remote.catalog_removed {"removed"} else {"complete"}}));
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
async fn engine_recovers_lost_creations_and_deletions_without_duplicate_mutations() {
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
    fresh_repository(&server, "main").await;
    let state = remote.clone();
    Mock::given(method("GET")).and(path("/projects/42"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            if state.project_alive { ResponseTemplate::new(200).set_body_json(json!({"id":42,"path_with_namespace":format!("acme/{}",state.native_name.as_ref().unwrap()),"default_branch":"main","visibility":"private"})) }
            else { ResponseTemplate::new(404) }
        }).mount(&server).await;
    Mock::given(method("GET"))
        .and(path_regex("^/projects/42/repository/files/"))
        .respond_with(ResponseTemplate::new(404))
        .expect(0)
        .mount(&server)
        .await;
    let state = remote.clone();
    let post_store = store.clone();
    Mock::given(method("POST"))
        .and(path("/projects/42/repository/commits"))
        .respond_with(move |request: &wiremock::Request| {
            let value: Value = request.body_json().unwrap();
            let receipt = value["commit_message"].as_str().unwrap();
            let actions = value["actions"].as_array().unwrap();
            assert_eq!(actions.len(), 3);
            assert!(
                actions
                    .iter()
                    .any(|action| action["file_path"] == "public/index.html"
                        && action["content"] == "aGVsbG8=")
            );
            let marker = actions
                .iter()
                .find(|action| {
                    action["file_path"] == "public/.well-known/stackless-deployment.json"
                })
                .unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(marker["content"].as_str().unwrap())
                .unwrap();
            let marker: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(marker["receipt"], receipt);
            let owner = post_store.instance("demo").unwrap().unwrap();
            let record = post_store
                .resources(&owner.instance_id)
                .unwrap()
                .into_iter()
                .find(|r| r.resource_kind == "gitlab-project")
                .unwrap();
            let value: Value = serde_json::from_str(&record.payload).unwrap();
            let native: lifecycle::NativeState =
                serde_json::from_value(value["_gitlab"].clone()).unwrap();
            assert_eq!(native.project_id, Some(42));
            assert!(native.requests[receipt].submitted);
            let mut state = state.lock().unwrap();
            state.commits += 1;
            state.receipt = Some(receipt.into());
            state.serving_receipt = Some(receipt.into());
            state.pages_alive = true;
            ResponseTemplate::new(503)
        })
        .expect(1)
        .mount(&server)
        .await;
    let state = remote.clone();
    Mock::given(method("GET"))
        .and(path("/projects/42/repository/commits"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200)
                .set_body_json(json!([{"id":SHA,"message":state.lock().unwrap().receipt}]))
        })
        .mount(&server)
        .await;
    for (route, body) in [
        (
            "/projects/42/pipelines",
            json!([{"id":99,"project_id":42,"ref":"main","sha":SHA,"status":"success"}]),
        ),
        (
            "/projects/42/pipelines/99",
            json!({"id":99,"project_id":42,"ref":"main","sha":SHA,"status":"success"}),
        ),
        (
            "/projects/42/pipelines/99/jobs",
            json!([{"id":7,"name":"pages","status":"success"}]),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    let state = remote.clone();
    let origin = server.uri();
    Mock::given(method("GET")).and(path("/projects/42/pages"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"deployments":if state.pages_alive { vec![json!({"path_prefix":"","url":origin})] } else { vec![] }}))
        }).mount(&server).await;
    let state = remote.clone();
    Mock::given(method("GET"))
        .and(path("/.well-known/stackless-deployment.json"))
        .respond_with(move |request: &wiremock::Request| {
            assert!(!request.headers.contains_key("private-token"));
            ResponseTemplate::new(200)
                .set_body_json(json!({"receipt":state.lock().unwrap().serving_receipt}))
        })
        .mount(&server)
        .await;
    for (route, field, pages) in [
        ("/projects/42/pages", "pages_removal_submitted", true),
        ("/projects/42", "project_removal_submitted", false),
    ] {
        let state = remote.clone();
        let delete_store = store.clone();
        Mock::given(method("DELETE"))
            .and(path(route))
            .respond_with(move |_: &wiremock::Request| {
                let owner = delete_store.instance("demo").unwrap().unwrap();
                let record = delete_store
                    .resources(&owner.instance_id)
                    .unwrap()
                    .into_iter()
                    .find(|r| r.resource_kind == "gitlab-project")
                    .unwrap();
                let value: Value = serde_json::from_str(&record.payload).unwrap();
                assert_eq!(value["_gitlab"][field], true);
                let mut state = state.lock().unwrap();
                if pages {
                    state.pages_deletes += 1;
                    state.pages_alive = false;
                } else {
                    assert!(!state.pages_alive);
                    state.project_deletes += 1;
                    state.project_alive = false;
                }
                ResponseTemplate::new(503)
            })
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let text = "[stack]\nname='project'\n[services.web]\nsource={repo='https://example.invalid/source',ref='main'}\nhealth={path='/'}\n[services.web.gitlab]\n";
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
        let mut substrate = GitLabSubstrate::for_test(
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
            .insert("GITLAB_TOKEN".into(), "test".into());
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
    let error = engine.run_up(request(), admission).await.unwrap_err();
    assert!(
        error.to_string().contains("lost catalog creation"),
        "{error}"
    );
    assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
    drop(substrate);
    drop(store);
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
    drop(substrate);
    drop(store);
    let store = Store::open(&db).unwrap();
    let substrate = make(&store);
    Engine {
        store: &store,
        substrate: &substrate,
    }
    .up(request())
    .await
    .unwrap();
    let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
    let context = InstanceContext::from_record(&owner, &[]);
    assert_eq!(
        substrate.observe(&context, &checkpoint).await.unwrap(),
        Observation::Present
    );
    remote.lock().unwrap().serving_receipt = Some("another deployment".into());
    assert!(matches!(
        substrate.observe(&context, &checkpoint).await.unwrap(),
        Observation::Drifted { .. }
    ));
    {
        let mut remote = remote.lock().unwrap();
        remote.serving_receipt = remote.receipt.clone();
    }
    let sibling = store
        .create_instance(
            "sibling",
            "gitlab",
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
        .find(|r| r.resource_kind == "gitlab-project")
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
    for _ in 0..3 {
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
    }
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
            .all(|record| record.phase == ResourcePhase::Absent)
    );
    let remote = remote.lock().unwrap();
    assert_eq!(
        (
            remote.creates,
            remote.commits,
            remote.pages_deletes,
            remote.project_deletes,
            remote.catalog_deletes
        ),
        (1, 1, 1, 1, 1)
    );
}

#[tokio::test]
async fn soft_deleted_project_stays_owned_until_native_absence() {
    use stackless_core::state::{Ownership, ResourceIntent};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let owner = store
        .create_instance("demo", "gitlab", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    let instance = InstanceContext::from_record(&owner, &[]);
    let name = instance.resource_name("web");
    let key = format!("catalog:gitlab/project:{name}");
    let native = lifecycle::NativeState {
        project_id: Some(42),
        namespace_path: Some(format!("acme/{name}")),
        ..Default::default()
    };
    let payload = json!({"project_id":"42", "project_name":name, "stripe_resource":name, "_gitlab":native,
        "_catalog_creation":{"reference":"gitlab/project", "requested_name":name, "config":{"name":name,"visibility":"private"}, "submitted":true,"confirmed":true,"response":{}, "project_id":"stripe_project", "remote_id":"remote_project", "provider_id":PROVIDER, "removal_submitted":false}}).to_string();
    store
        .resource_intent(ResourceIntent {
            owner_id: &owner.instance_id,
            key: &key,
            step_id: "start:web",
            provider: "gitlab",
            ownership: Ownership::Owned,
            resource_kind: "gitlab-project",
            resource_id: &name,
            payload: &payload,
            dependencies: &[],
        })
        .unwrap();
    store
        .resource_created(&owner.instance_id, &key, &name, &payload)
        .unwrap();
    let remote = Arc::new(StdMutex::new(Remote {
        resource: Some(name.clone()),
        native_name: Some(name),
        project_alive: true,
        ..Default::default()
    }));
    let server = MockServer::start().await;
    let state = remote.clone();
    Mock::given(method("GET")).and(path("/projects/42"))
        .respond_with(move |_: &wiremock::Request| {
            let state = state.lock().unwrap();
            if state.project_alive { ResponseTemplate::new(200).set_body_json(json!({"id":42,"path_with_namespace":format!("acme/{}",state.native_name.as_ref().unwrap()),"default_branch":"main","visibility":"private","marked_for_deletion_on":"2026-09-06"})) }
            else { ResponseTemplate::new(404) }
        }).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/projects/42/pages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"deployments":[]})))
        .mount(&server)
        .await;
    let state = remote.clone();
    Mock::given(method("DELETE"))
        .and(path("/projects/42"))
        .respond_with(move |_: &wiremock::Request| {
            state.lock().unwrap().project_deletes += 1;
            ResponseTemplate::new(202)
        })
        .expect(1)
        .mount(&server)
        .await;
    let mut substrate = GitLabSubstrate::for_test(
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
        .insert("GITLAB_TOKEN".into(), "test".into());
    for _ in 0..2 {
        let reopened = Store::open(&db).unwrap();
        let record = reopened
            .resource(&owner.instance_id, &key)
            .unwrap()
            .unwrap();
        let error = substrate
            .destroy_record(&reopened, &instance, &record)
            .await
            .unwrap_err();
        assert!(error.message.contains("deletion is pending"), "{error:?}");
        assert_eq!(
            substrate
                .observe_record(&reopened, &instance, &record)
                .await
                .unwrap(),
            Observation::Present
        );
        assert_ne!(
            reopened
                .resource(&owner.instance_id, &key)
                .unwrap()
                .unwrap()
                .phase,
            ResourcePhase::Absent
        );
    }
    assert_eq!(remote.lock().unwrap().catalog_deletes, 0);
    remote.lock().unwrap().project_alive = false;
    let record = store.resource(&owner.instance_id, &key).unwrap().unwrap();
    assert!(
        substrate
            .destroy_record(&store, &instance, &record)
            .await
            .is_err()
    );
    substrate
        .destroy_record(&store, &instance, &record)
        .await
        .unwrap();
    assert_eq!(
        substrate
            .observe_record(&store, &instance, &record)
            .await
            .unwrap(),
        Observation::Gone
    );
    assert_eq!(remote.lock().unwrap().catalog_deletes, 1);
}

pub(super) async fn fresh_repository(server: &MockServer, branch: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::query_param;
    let reads = Arc::new(AtomicUsize::new(0));
    let name = branch.to_owned();
    Mock::given(method("GET"))
        .and(path(format!("/projects/42/repository/branches/{branch}")))
        .respond_with(move |_: &wiremock::Request| {
            let sha = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                "b".repeat(40)
            } else {
                SHA.into()
            };
            ResponseTemplate::new(200).set_body_json(json!({"name":name,"commit":{"id":sha}}))
        })
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects/42/repository/tree"))
        .and(query_param("ref", "b".repeat(40)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(server)
        .await;
    let rows: Vec<_> = [
        "public/index.html",
        "public/.well-known/stackless-deployment.json",
        ".gitlab-ci.yml",
    ]
    .into_iter()
    .map(|p| json!({"id":"c".repeat(40),"type":"blob","path":p}))
    .collect();
    Mock::given(method("GET"))
        .and(path("/projects/42/repository/tree"))
        .and(query_param("ref", SHA))
        .respond_with(ResponseTemplate::new(200).set_body_json(rows))
        .mount(server)
        .await;
}
