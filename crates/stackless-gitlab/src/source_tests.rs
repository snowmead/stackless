use super::*;
use base64::Engine as _;
use serde_json::json;
use stackless_core::{engine::Step, state::Store};
use stackless_stripe_projects::{CommandOutput, test_support};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

struct Runner(
    std::sync::Arc<std::sync::Mutex<String>>,
    std::sync::Arc<std::sync::Mutex<String>>,
);
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
                assert_eq!(args[1], "gitlab/project");
                *self.1.lock().unwrap() = args[3].clone();
                let config: serde_json::Value = serde_json::from_str(&args[5]).unwrap();
                *self.0.lock().unwrap() = config["name"].as_str().unwrap().into();
                test_support::ok(
                    json!({"service":{"key":"remote_project", "provider_id":"prvdr_61UU5TYtG0f1neJCz5EwK"}, "variables": {
                        "GITLAB_PROJECT_ID": "42",
                        "GITLAB_WEB_URL": "https://gitlab.com/acme/fixture"
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
        let name = self.1.lock().unwrap().clone();
        let resource = json!({"id":"remote_project", "provider":"prvdr_61UU5TYtG0f1neJCz5EwK", "service_ref":"project", "status":"complete", "name":name});
        let body = if route.contains('?') {
            json!({"data":if name.is_empty() {vec![]} else {vec![resource]}, "next_page_url":null})
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
    crate::lifecycle_tests::fresh_repository(&server, "deploy-main").await;
    let project_name = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let native_name = project_name.clone();
    Mock::given(method("GET")).and(path("/projects/42"))
        .respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(json!({"id":42, "default_branch":"deploy-main", "path_with_namespace":format!("acme/{}", native_name.lock().unwrap()), "visibility":"private"})))
        .mount(&server).await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            "^/projects/42/repository/files/",
        ))
        .and(wiremock::matchers::query_param("ref", "deploy-main"))
        .respond_with(ResponseTemplate::new(404))
        .expect(0)
        .mount(&server)
        .await;
    let receipt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let created_receipt = receipt.clone();
    Mock::given(method("POST"))
        .and(path("/projects/42/repository/commits"))
        .and(body_partial_json(json!({"branch":"deploy-main"})))
        .respond_with(move |request: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let actions = body["actions"].as_array().unwrap();
            assert_eq!(actions.len(), 3);
            let index = actions
                .iter()
                .find(|action| action["file_path"] == "public/index.html")
                .unwrap();
            assert_eq!(
                index["content"],
                base64::engine::general_purpose::STANDARD.encode("<p>original</p>")
            );
            assert_eq!(index["encoding"], "base64");
            let marker = actions
                .iter()
                .find(|action| {
                    action["file_path"] == "public/.well-known/stackless-deployment.json"
                })
                .unwrap();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(marker["content"].as_str().unwrap())
                .unwrap();
            let marker: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(marker["receipt"], body["commit_message"]);
            *created_receipt.lock().unwrap() = marker["receipt"].as_str().unwrap().into();
            ResponseTemplate::new(201)
                .set_body_json(json!({"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}))
        })
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/stackless-deployment.json"))
        .respond_with(move |request: &wiremock::Request| {
            assert!(!request.headers.contains_key("private-token"));
            ResponseTemplate::new(200)
                .set_body_json(json!({"receipt":receipt.lock().unwrap().clone()}))
        })
        .mount(&server)
        .await;
    for (endpoint, response) in [
        (
            "/projects/42/pipelines",
            json!([{"id":99, "project_id":42, "sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ref":"deploy-main", "status":"running"}]),
        ),
        (
            "/projects/42/pipelines/99/jobs",
            json!([{"id":7, "name":"pages", "status":"success"}]),
        ),
        (
            "/projects/42/pipelines/99",
            json!({"id":99, "project_id":42, "sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ref":"deploy-main", "status":"success"}),
        ),
        (
            "/projects/42/pages",
            json!({"url":server.uri(), "deployments":[{"path_prefix":"", "url":server.uri()}]}),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;
    }
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
        .create_instance("demo", "gitlab", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    let record = store.instance("demo").unwrap().unwrap();
    store.grant_host_execution(&record.instance_id).unwrap();
    store
        .bind_stripe_project(
            &record.instance_id,
            "fixture-project",
            Some("stripe_project"),
        )
        .unwrap();
    let instance = InstanceContext::from_record(&record, &[]);
    let overrides = BTreeMap::new();
    let mut substrate = GitLabSubstrate::for_test(
        Runner(project_name, Default::default()),
        dir.path(),
        server.uri(),
        false,
    );
    *substrate.ensured.lock().await = true;
    substrate
        .secrets
        .insert("GITLAB_TOKEN".into(), "test".into());
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
    let payload: GitLabPayload = serde_json::from_str(&deployed.payload).unwrap();
    assert_eq!(payload.project_id, "42");
    assert_eq!(payload.pipeline_id, 99);
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
fn common_root_aliases_are_validated_before_provisioning() {
    let parse = |source: &str, provider: &str| {
        StackDef::parse(&format!("[stack]\nname='fixture'\n[services.web]\nsource={{repo='r',root={source:?}}}\nhealth={{path='/'}}\n{provider}")).unwrap()
    };
    assert_eq!(
        config::service_gitlab(&parse("app", ""), "web")
            .unwrap()
            .root
            .as_deref(),
        Some("app")
    );
    assert_eq!(
        config::service_gitlab(&parse("./app", "[services.web.gitlab]\nroot='app'"), "web")
            .unwrap()
            .root
            .as_deref(),
        Some("app")
    );
    for (root, provider) in [
        ("app", "[services.web.gitlab]\nroot='other'"),
        ("../outside", ""),
        ("/tmp", ""),
        (".env", ""),
    ] {
        assert!(config::service_gitlab(&parse(root, provider), "web").is_err());
    }
}

#[tokio::test]
async fn binary_assets_use_base64_commit_actions() {
    use stackless_core::source_archive::{SourceArchive, SourceFile};
    let bytes = vec![0, 255, 128, 42];
    let files = collect_public_files(SourceArchive {
        directories: vec!["images".into()],
        files: vec![SourceFile {
            path: "images/pixel.png".into(),
            executable: false,
            contents: base64::engine::general_purpose::STANDARD.encode(&bytes),
        }],
    })
    .unwrap();
    assert_eq!(files[0].content, bytes);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/projects/42/repository/files/images%2Fpixel.png"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(path("/projects/42/repository/commits"))
        .and(body_partial_json(json!({"actions":[{"action":"create", "file_path":"images/pixel.png", "content":"AP+AKg==", "encoding":"base64"}]})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id":"a".repeat(40)})))
        .expect(1).mount(&server).await;
    let api = GitLabApi::with_base("test", server.uri());
    assert_eq!(
        api.commit_files("42", "main", "fixture", &files)
            .await
            .unwrap(),
        "a".repeat(40)
    );
}
