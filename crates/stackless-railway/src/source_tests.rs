use super::*;
use serde_json::json;
use stackless_core::{engine::Step, state::Store};
use stackless_stripe_projects::{CommandOutput, test_support};
use wiremock::MockServer;

#[derive(Default)]
struct Runner {
    resource: std::sync::Mutex<Option<String>>,
}
#[async_trait]
impl CommandRunner for Runner {
    async fn run(
        &self,
        args: &[String],
        _: &std::path::Path,
    ) -> Result<CommandOutput, ProjectsError> {
        Ok(match args[0].as_str() {
            "catalog" => test_support::raw(include_str!(
                "../../stackless-stripe-projects/tests/fixtures/catalog.json"
            )),
            "services" => test_support::plans(&[("free", "free", "Railway")]),
            "env" => test_support::ok_empty(),
            "add" => {
                *self.resource.lock().unwrap() = Some(args[3].clone());
                test_support::ok(
                    json!({"service":{"key":"remote_project","provider_id":"prvdr_61UK2uWsoydL7tUBp57Fg"},"variables":{"RAILWAY_URL":"https://account.railway.app"}}),
                )
            }
            other => panic!("unexpected command {other}"),
        })
    }
    async fn request(
        &self,
        _: &str,
        route: &str,
        _: &std::path::Path,
    ) -> Result<CommandOutput, ProjectsError> {
        let row = self.resource.lock().unwrap().as_ref().map(|name|json!({"id":"remote_project","name":name,"provider":"prvdr_61UK2uWsoydL7tUBp57Fg","service_ref":"hosting","status":"complete"}));
        let value = if route.contains('?') {
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
fn ctx<'a>(
    store: &'a Store,
    instance: &'a InstanceContext<'a>,
    def: &'a StackDef,
    step: &'a Step,
    prior: &'a [Checkpoint],
    overrides: &'a BTreeMap<String, String>,
) -> StepContext<'a> {
    StepContext {
        operation_id: "snapshot-test",
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
async fn restart_and_prepare_changes_preserve_the_commit_sent_to_railway() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    stackless_git::build_repo(
        &repo,
        &[&[
            ("app/index.html", "original"),
            ("app/.env", "private"),
            ("other.txt", "outside"),
        ]],
    )
    .unwrap();
    let text = "[stack]\nname='fixture'\n[services.web]\nsource={repo='https://github.com/org/repo',ref='main',root='app'}\nprepare='cat index.html > seen.txt; printf changed > index.html'\nhealth={path='/'}\n";
    let def = StackDef::parse(text).unwrap();
    let mut local = def.clone();
    local.services.get_mut("web").unwrap().source.repo = repo.display().to_string();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let record = store
        .create_instance("demo", "railway", text, &BTreeMap::new(), "", false)
        .unwrap();
    store
        .bind_stripe_project(&record.instance_id, "test-project", Some("stripe_project"))
        .unwrap();
    store.grant_host_execution(&record.instance_id).unwrap();
    let instance = InstanceContext::from_record(&record, &[]);
    let overrides = BTreeMap::new();
    let server = MockServer::start().await;
    let mut substrate =
        RailwaySubstrate::for_test(Runner::default(), dir.path(), Some(server.uri()), true);
    *substrate.ensured.lock().await = true;
    substrate
        .secrets
        .insert("RAILWAY_API_TOKEN".into(), "test".into());
    let materialize = Step {
        id: "materialize:web".into(),
        kind: StepKind::Materialize,
        node: "web".into(),
    };
    let resource = substrate
        .execute(ctx(
            &store,
            &instance,
            &local,
            &materialize,
            &[],
            &overrides,
        ))
        .await
        .unwrap();
    let snapshot: stackless_cloud::source::Snapshot =
        serde_json::from_str(&resource.payload).unwrap();
    let commit = snapshot.commit().unwrap().to_owned();
    drop(store);
    std::fs::remove_dir_all(repo).unwrap();
    std::fs::remove_dir_all(&snapshot.path).unwrap();
    let store = Store::open(&db).unwrap();
    let recovered = substrate
        .execute(ctx(
            &store,
            &instance,
            &local,
            &materialize,
            &[],
            &overrides,
        ))
        .await
        .unwrap();
    assert_eq!(recovered.payload, resource.payload);
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
        .execute(ctx(&store, &instance, &def, &prepare, &prior, &overrides))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(snapshot.path.join("app/seen.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(snapshot.path.join("app/index.html")).unwrap(),
        "changed"
    );
    assert!(
        snapshot
            .archive(Some("app"))
            .unwrap()
            .files
            .iter()
            .all(|f| f.path != ".env" && f.path != "seen.txt")
    );
    let remote = std::sync::Arc::new(std::sync::Mutex::new(
        super::lifecycle_tests::NativeRemote::source(
            commit.clone(),
            "https://test.up.railway.app".into(),
        ),
    ));
    super::lifecycle_tests::mount_native(&server, remote.clone(), store.clone()).await;
    let start = Step {
        id: "start:web".into(),
        kind: StepKind::Start,
        node: "web".into(),
    };
    let result = substrate
        .execute(ctx(&store, &instance, &def, &start, &prior, &overrides))
        .await
        .unwrap();
    let payload: RailwayPayload = serde_json::from_str(&result.payload).unwrap();
    assert_eq!(payload.commit_sha.as_deref(), Some(commit.as_str()));
    assert_eq!(remote.lock().unwrap().settings()["rootDirectory"], "/app");
    assert_eq!(
        remote.lock().unwrap().settings()["source"]["repo"],
        "org/repo"
    );
    assert_eq!(payload.origin, "https://test.up.railway.app");
    assert_eq!(payload.url, payload.origin);
    let checkpoint = Checkpoint {
        instance: "demo".into(),
        step_id: start.id,
        resource_kind: result.resource_kind,
        resource_id: result.resource_id,
        payload: result.payload,
        recorded_at: 0,
    };
    assert_eq!(
        substrate.observe(&instance, &checkpoint).await.unwrap(),
        Observation::Present
    );
    let sibling = InstanceContext {
        routed_origins: None,
        name: "sibling",
        id: "foreign",
        resource_namespace: "foreign",
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
fn root_and_input_types_fail_before_execution() {
    let parse = |extra: &str| {
        StackDef::parse(&format!("[stack]\nname='fixture'\n[services.web]\nsource={{repo='https://github.com/org/repo',root='app'}}\nhealth={{path='/'}}\n[services.web.railway]\n{extra}")).unwrap()
    };
    assert_eq!(
        config::service_railway(&parse("root='./app'"), "web")
            .unwrap()
            .root
            .as_deref(),
        Some("app")
    );
    for extra in [
        "root='../outside'",
        "root='other'",
        "image=5",
        "image=''",
        "cmd=5",
        "image='nginx'\ncmd=[]",
        "image='nginx'\ncmd=['a',2]",
    ] {
        assert!(
            config::service_railway(&parse(extra), "web").is_err(),
            "{extra}"
        );
    }
    let mut reserved = parse("image='nginx'");
    reserved
        .services
        .get_mut("web")
        .unwrap()
        .env
        .insert(lifecycle::SERVICE_RECEIPT.into(), "foreign".into());
    let substrate =
        RailwaySubstrate::for_test(Runner::default(), std::path::Path::new("."), None, false);
    assert!(substrate.validate_definition(&reserved).is_err());
    for url in [
        "https://github.com/org/repo?token=secret",
        "https://github.com/org/repo#fragment",
        "https://github.com/org/..",
        "https://github.com/org/%2e%2e",
    ] {
        assert!(parse_github_repo(url).is_err());
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
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    let record = store
        .create_instance("demo", "railway", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    store
        .bind_stripe_project(&record.instance_id, "test-project", Some("stripe_project"))
        .unwrap();
    let instance = InstanceContext::from_record(&record, &[]);
    let mut def=StackDef::parse("[stack]\nname='fixture'\n[services.web]\nsource={repo='https://unavailable.invalid'}\nhealth={path='/'}\n[services.web.railway]\nimage='nginx'\n").unwrap();
    let substrate = RailwaySubstrate::for_test(Runner::default(), dir.path(), None, false);
    *substrate.ensured.lock().await = true;
    let step = Step {
        id: "materialize:web".into(),
        kind: StepKind::Materialize,
        node: "web".into(),
    };
    let resource = substrate
        .execute(ctx(&store, &instance, &def, &step, &[], &BTreeMap::new()))
        .await
        .unwrap();
    assert_eq!(resource.resource_kind, "action");
    assert!(!dir.path().join(".stackless-sources").exists());
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
        .execute(ctx(&store, &instance, &def, &step, &[], &BTreeMap::new()))
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
        .execute(ctx(&store, &instance, &def, &step, &[], &BTreeMap::new()))
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
        .execute(ctx(
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
        .execute(ctx(
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
        .execute(ctx(
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
