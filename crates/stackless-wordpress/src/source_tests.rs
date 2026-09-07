use super::*;
use serde_json::json;
use stackless_core::{engine::Step, state::Store};
use stackless_stripe_projects::{CommandOutput, test_support};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const PROVIDER: &str = "prvdr_61UmxGkoAMSyI59y25SzA";
struct Runner(std::sync::Arc<std::sync::Mutex<String>>, String);
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
                assert_eq!(args[1], "wordpress.com/site");
                *self.0.lock().unwrap() = args[3].clone();
                test_support::ok(
                    json!({"service":{"key":"remote_site", "provider_id":PROVIDER},"variables": {
                        "WORDPRESS_COM_SITE_URL": self.1,
                        "WORDPRESS_COM_ADMIN_URL": format!("{}/admin", self.1),
                        "WORDPRESS_COM_BLOG_ID": "99"
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
        let resource = json!({"id":"remote_site", "provider":PROVIDER,"service_ref":"site","status":"complete","name":name});
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
async fn source_survives_restart_and_prepare_cannot_change_published_html() {
    let server = MockServer::start().await;
    let origin = server.uri();
    Mock::given(method("GET")).and(path("/sites/99"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ID":99,"URL":origin,"is_private":false,"is_coming_soon":false,"user_can_manage":true}))).mount(&server).await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex("^/sites/99/posts/slug:"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let published = std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
    let saved = published.clone();
    Mock::given(method("POST"))
        .and(path("/sites/99/posts/new"))
        .respond_with(move |request: &wiremock::Request| {
            let mut value: serde_json::Value = request.body_json().unwrap();
            value["ID"] = json!(42);
            value["site_ID"] = json!(99);
            value["has_password"] = json!(false);
            value["URL"] = json!(format!("{origin}/page/"));
            assert!(
                value["content"]
                    .as_str()
                    .unwrap()
                    .starts_with("<p>original</p>\n<!-- stackless-")
            );
            *saved.lock().unwrap() = value.clone();
            ResponseTemplate::new(200).set_body_json(value)
        })
        .expect(1)
        .mount(&server)
        .await;
    let saved = published.clone();
    Mock::given(method("GET"))
        .and(path("/sites/99/posts/42"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(saved.lock().unwrap().clone())
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(move |request: &wiremock::Request| {
            assert!(!request.headers.contains_key("authorization"));
            ResponseTemplate::new(200)
                .set_body_string(published.lock().unwrap()["content"].as_str().unwrap())
        })
        .mount(&server)
        .await;
    let homepage = std::sync::Arc::new(std::sync::Mutex::new(false));
    let state = homepage.clone();
    Mock::given(method("GET")).and(path("/sites/99/settings"))
        .respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(json!({"settings":{"show_on_front":if *state.lock().unwrap() {"page"} else {"posts"},"page_on_front":42}})))
        .mount(&server).await;
    Mock::given(method("POST"))
        .and(path("/sites/99/settings"))
        .respond_with(move |_: &wiremock::Request| {
            *homepage.lock().unwrap() = true;
            ResponseTemplate::new(200).set_body_json(json!({}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(
        &repo,
        &[&[
            ("app/index.html", "<p>original</p>"),
            ("index.html", "wrong root"),
        ]],
    )
    .unwrap();
    let def = StackDef::parse(&format!("[stack]\nname='fixture'\n[services.web]\nsource={{repo={:?},ref='main',root='app'}}\nprepare='cat index.html > seen.txt; printf changed > index.html'\nhealth={{path='/'}}\n", repo.display().to_string())).unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    store
        .create_instance(
            "demo",
            "wordpress",
            "definition",
            &BTreeMap::new(),
            "",
            false,
        )
        .unwrap();
    let record = store.instance("demo").unwrap().unwrap();
    store.grant_host_execution(&record.instance_id).unwrap();
    store
        .bind_stripe_project(&record.instance_id, "project:test", Some("stripe_project"))
        .unwrap();
    let instance = InstanceContext::from_record(&record, &[]);
    let overrides = BTreeMap::new();
    let mut substrate = WordPressSubstrate::for_test(
        Runner(Default::default(), server.uri()),
        dir.path(),
        server.uri(),
        false,
    );
    *substrate.ensured.lock().await = true;
    substrate
        .secrets
        .insert("WORDPRESS_COM_ACCESS_TOKEN".into(), "test".into());
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
    let deployed = substrate
        .execute(context(&store, &instance, &def, &start, &prior, &overrides))
        .await
        .unwrap();
    let payload: WordPressPayload = serde_json::from_str(&deployed.payload).unwrap();
    assert_eq!(payload.page_id, "42");
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
fn html_selection_is_deterministic_and_rejects_missing_or_non_utf8_content() {
    use base64::Engine as _;
    use stackless_core::source_archive::{SourceArchive, SourceFile};
    let file = |path: &str, bytes: &[u8]| SourceFile {
        path: path.into(),
        executable: false,
        contents: base64::engine::general_purpose::STANDARD.encode(bytes),
    };
    let mut archive = SourceArchive {
        directories: vec![],
        files: vec![file("z.html", b"last"), file("a.html", b"first")],
    };
    assert_eq!(read_deploy_html(archive.clone()).unwrap(), "first");
    archive.files.push(file("index.html", b"index"));
    assert_eq!(read_deploy_html(archive.clone()).unwrap(), "index");
    archive.files = vec![file("index.html", &[0xff])];
    assert!(read_deploy_html(archive.clone()).is_err());
    archive.files.clear();
    assert!(read_deploy_html(archive).is_err());
}

#[test]
fn common_root_aliases_are_validated_before_provisioning() {
    let parse = |source: &str, provider: &str| {
        StackDef::parse(&format!("[stack]\nname='fixture'\n[services.web]\nsource={{repo='r',root={source:?}}}\nhealth={{path='/'}}\n{provider}")).unwrap()
    };
    assert_eq!(
        config::service_wordpress(&parse("app", ""), "web")
            .unwrap()
            .root
            .as_deref(),
        Some("app")
    );
    assert_eq!(
        config::service_wordpress(
            &parse("./app", "[services.web.wordpress]\nroot='app'"),
            "web"
        )
        .unwrap()
        .root
        .as_deref(),
        Some("app")
    );
    for (root, provider) in [
        ("app", "[services.web.wordpress]\nroot='other'"),
        ("../outside", ""),
        ("/tmp", ""),
        (".env", ""),
    ] {
        assert!(config::service_wordpress(&parse(root, provider), "web").is_err());
    }
}
