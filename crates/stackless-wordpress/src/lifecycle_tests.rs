//! The real engine retains native site ownership across catalog cancellation.
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
    matchers::{method, path, path_regex},
};
const CATALOG: &str = include_str!("../../stackless-stripe-projects/tests/fixtures/catalog.json");
const PROVIDER: &str = "prvdr_61UmxGkoAMSyI59y25SzA";
#[derive(Default)]
struct Remote {
    resource: Option<String>,
    project_alive: bool,
    catalog_removed: bool,
    creates: usize,
    project_deletes: usize,
    catalog_deletes: usize,
    page: Option<Value>,
    posts: usize,
    settings: usize,
    homepage: bool,
    serving: bool,
}
struct Runner {
    store: Store,
    remote: Arc<StdMutex<Remote>>,
    origin: String,
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
                    format!("WORDPRESS_COM_ADMIN_URL=https://fixture.wordpress.com/admin\nWORDPRESS_COM_BLOG_ID=99\nWORDPRESS_COM_SITE_URL={}\nWORDPRESS_COM_ACCESS_TOKEN=test\n", self.origin),
                )
                .unwrap();
                test_support::ok_empty()
            }
            "add" => {
                assert_eq!(args[1], "wordpress.com/site");
                let name = args[3].clone();
                let record = self
                    .store
                    .resource(
                        &owner.instance_id,
                        &format!("catalog:wordpress.com/site:{name}"),
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
            assert!(remote.project_alive);
            remote.catalog_removed = true;
            remote.catalog_deletes += 1;
            return Err(ProjectsError::Unavailable {
                detail: "lost catalog removal response".into(),
            });
        }
        let row = remote.resource.as_ref().map(|name| json!({"id":"remote_project", "name":name, "provider":PROVIDER, "service_ref":"site", "status":if remote.catalog_removed {"removed"} else {"complete"}}));
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

fn recorded_native(store: &Store) -> lifecycle::NativeState {
    let owner = store.instance("demo").unwrap().unwrap();
    let record = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "wordpress-site")
        .unwrap();
    let value: Value = serde_json::from_str(&record.payload).unwrap();
    serde_json::from_value(value["_wordpress"].clone()).unwrap()
}
async fn recover(abandon_after_catalog: bool, delayed_delete: bool) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(&repo, &[&[("index.html", "<p>original</p>")]]).unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let remote = Arc::new(StdMutex::new(Remote::default()));
    let server = MockServer::start().await;
    let state = remote.clone();
    let origin = server.uri();
    Mock::given(method("GET")).and(path("/sites/99"))
        .respond_with(move |_: &wiremock::Request| {
            if !state.lock().unwrap().project_alive { return ResponseTemplate::new(404); }
            ResponseTemplate::new(200).set_body_json(json!({"ID":99,"URL":origin,"is_private":false,"is_coming_soon":false,"user_can_manage":true}))
        }).mount(&server).await;
    let state = remote.clone();
    Mock::given(method("GET"))
        .and(path_regex(
            "^/sites/99/posts/(slug:stackless-[a-f0-9]+|42)$",
        ))
        .respond_with(
            move |_: &wiremock::Request| match &state.lock().unwrap().page {
                Some(page) => ResponseTemplate::new(200).set_body_json(page),
                None => ResponseTemplate::new(404),
            },
        )
        .mount(&server)
        .await;
    let state = remote.clone();
    let post_store = store.clone();
    let origin = server.uri();
    Mock::given(method("POST"))
        .and(path("/sites/99/posts/new"))
        .respond_with(move |request: &wiremock::Request| {
            let mut value: Value = request.body_json().unwrap();
            assert!(
                value["content"]
                    .as_str()
                    .unwrap()
                    .starts_with("<p>original</p>\n<!-- stackless-")
            );
            assert_eq!(value["publicize"], false);
            let receipt = value["slug"].as_str().unwrap();
            assert_eq!(value["metadata"][0]["value"], receipt);
            let native = recorded_native(&post_store);
            assert_eq!(native.site_id, Some(99));
            assert!(native.requests[receipt].submitted);
            assert!(native.requests[receipt].page_id.is_none());
            value["ID"] = json!(42);
            value["site_ID"] = json!(99);
            value["has_password"] = json!(false);
            value["URL"] = json!(format!("{origin}/page/"));
            let mut state = state.lock().unwrap();
            state.posts += 1;
            state.page = Some(value);
            ResponseTemplate::new(503)
        })
        .expect(u64::from(!abandon_after_catalog))
        .mount(&server)
        .await;
    let state = remote.clone();
    Mock::given(method("GET")).and(path("/sites/99/settings"))
        .respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(json!({"settings":{"show_on_front":if state.lock().unwrap().homepage {"page"} else {"posts"},"page_on_front":42}})))
        .mount(&server).await;
    let state = remote.clone();
    let setting_store = store.clone();
    Mock::given(method("POST"))
        .and(path("/sites/99/settings"))
        .respond_with(move |_: &wiremock::Request| {
            let native = recorded_native(&setting_store);
            let request = native.requests.values().next().unwrap();
            assert_eq!(request.page_id, Some(42));
            assert!(request.homepage_submitted);
            let mut state = state.lock().unwrap();
            state.homepage = true;
            state.serving = true;
            state.settings += 1;
            ResponseTemplate::new(503)
        })
        .expect(u64::from(!abandon_after_catalog))
        .mount(&server)
        .await;
    let state = remote.clone();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            assert!(!request.headers.contains_key("authorization"));
            let state = state.lock().unwrap();
            ResponseTemplate::new(200).set_body_string(if state.serving {
                state.page.as_ref().unwrap()["content"].as_str().unwrap()
            } else {
                "old homepage"
            })
        })
        .mount(&server)
        .await;
    let state = remote.clone();
    let delete_store = store.clone();
    Mock::given(method("POST"))
        .and(path("/sites/99/delete"))
        .respond_with(move |_: &wiremock::Request| {
            assert!(recorded_native(&delete_store).removal_submitted);
            let mut state = state.lock().unwrap();
            assert!(state.catalog_removed);
            state.project_deletes += 1;
            if !delayed_delete {
                state.project_alive = false;
            }
            ResponseTemplate::new(503)
        })
        .expect(1)
        .mount(&server)
        .await;
    let text = "[stack]\nname='project'\n[services.web]\nsource={repo='https://example.invalid/source',ref='main'}\nhealth={path='/'}\n[services.web.wordpress]\n";
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
        let mut substrate = WordPressSubstrate::for_test(
            Runner {
                store: store.clone(),
                remote: remote.clone(),
                origin: server.uri(),
            },
            dir.path(),
            server.uri(),
            false,
        );
        substrate
            .secrets
            .insert("WORDPRESS_COM_ACCESS_TOKEN".into(), "test".into());
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
    drop(substrate);
    drop(store);
    if !abandon_after_catalog {
        for _ in 0..2 {
            let store = Store::open(&db).unwrap();
            let substrate = make(&store);
            let error = Engine {
                store: &store,
                substrate: &substrate,
            }
            .up(request())
            .await
            .unwrap_err();
            assert!(error.to_string().contains("status 503"), "{error}");
            assert!(store.checkpoint("demo", "start:web").unwrap().is_none());
        }
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
        remote.lock().unwrap().serving = false;
        assert!(matches!(
            substrate.observe(&context, &checkpoint).await.unwrap(),
            Observation::Drifted { .. }
        ));
        remote.lock().unwrap().serving = true;
        remote.lock().unwrap().homepage = false;
        assert!(matches!(
            substrate.observe(&context, &checkpoint).await.unwrap(),
            Observation::Drifted { .. }
        ));
        remote.lock().unwrap().homepage = true;
    }
    let store = Store::open(&db).unwrap();
    let substrate = make(&store);
    let record = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "wordpress-site")
        .unwrap();
    let sibling = store
        .create_instance(
            "sibling",
            "wordpress",
            "definition",
            &BTreeMap::new(),
            "",
            false,
        )
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
    for response in [
        ResponseTemplate::new(403),
        ResponseTemplate::new(200).set_body_string("bad-json"),
        ResponseTemplate::new(200).set_body_json(json!({"ID":100,"URL":server.uri()})),
    ] {
        Mock::given(method("GET"))
            .and(path("/sites/99"))
            .respond_with(response)
            .with_priority(1)
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        let error = substrate
            .destroy_record(&store, &InstanceContext::from_record(&owner, &[]), &record)
            .await
            .unwrap_err();
        assert!(error.code.starts_with("wordpress."), "{error:?}");
        assert_eq!(remote.lock().unwrap().catalog_deletes, 0);
        assert_eq!(remote.lock().unwrap().project_deletes, 0);
    }
    // Stripe removal responds ambiguously after cancellation; native site stays owned.
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
        .find(|r| r.resource_kind == "wordpress-site")
        .unwrap();
    assert_eq!(
        substrate
            .observe_record(&store, &InstanceContext::from_record(&owner, &[]), &record)
            .await
            .unwrap(),
        Observation::Present
    );
    assert_eq!(recorded_native(&store).site_id, Some(99));
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
        assert_eq!(remote.lock().unwrap().project_deletes, 1);
        assert!(remote.lock().unwrap().project_alive);
        remote.lock().unwrap().project_alive = false;
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
            .all(|r| r.phase == ResourcePhase::Absent)
    );
    let state = remote.lock().unwrap();
    assert_eq!(
        (
            state.creates,
            state.posts,
            state.settings,
            state.catalog_deletes,
            state.project_deletes
        ),
        (
            1,
            usize::from(!abandon_after_catalog),
            usize::from(!abandon_after_catalog),
            1,
            1
        )
    );
}
#[tokio::test]
async fn lost_page_and_homepage_responses_recover_one_page_then_verify_native_deletion() {
    recover(false, false).await;
}
#[tokio::test]
async fn down_recovers_lost_catalog_creation_and_retains_unconfirmed_site_deletion() {
    recover(true, true).await;
}
