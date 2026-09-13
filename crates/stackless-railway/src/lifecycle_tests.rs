use super::*;
use serde_json::{Value, json};
use stackless_core::{
    engine::{Engine, UpRequest},
    state::{ResourcePhase, Store},
};
use stackless_stripe_projects::{CommandOutput, test_support};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex as StdMutex},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};
const PROVIDER: &str = "prvdr_61UK2uWsoydL7tUBp57Fg";

#[derive(Default)]
pub(super) struct NativeRemote {
    project: Value,
    service: Value,
    settings: Value,
    variables: Value,
    domain: bool,
    active: bool,
    pub commit: Option<String>,
    pub origin: String,
    failures: BTreeSet<&'static str>,
    counts: BTreeMap<String, usize>,
    deleted: bool,
    hidden: bool,
    delayed_delete: bool,
}
impl NativeRemote {
    pub(super) fn settings(&self) -> &Value {
        &self.settings
    }
    pub(super) fn source(commit: String, origin: String) -> Self {
        Self {
            commit: Some(commit),
            origin,
            ..Default::default()
        }
    }
}
fn connection(rows: Vec<Value>) -> Value {
    json!({"edges":rows.into_iter().map(|node|json!({"node":node})).collect::<Vec<_>>(),"pageInfo":{"hasNextPage":false,"endCursor":null}})
}
fn native_state(store: &Store) -> lifecycle::NativeState {
    let owner = store.instance("demo").unwrap().unwrap();
    let record = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "railway-service")
        .unwrap();
    let value: Value = serde_json::from_str(&record.payload).unwrap();
    value
        .get("_railway")
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .unwrap_or_default()
}
pub(super) async fn mount_native(
    server: &MockServer,
    state: Arc<StdMutex<NativeRemote>>,
    store: Store,
) {
    Mock::given(method("POST")).respond_with(move|request:&wiremock::Request|{
        let body:Value=request.body_json().unwrap();let query=body["query"].as_str().unwrap();let v=&body["variables"];
        let mut remote=state.lock().unwrap();
        let mutation=query.starts_with("mutation");
        let op=query.split_whitespace().nth(1).unwrap().split('(').next().unwrap();
        *remote.counts.entry(op.into()).or_default()+=1;
        if mutation {
            let native=native_state(&store);
            let kind=match op {"ProjectCreate"=>"project-create","ServiceCreate"=>"service-create","ServiceDomainCreate"=>"domain-create","ServiceInstanceUpdate"=>"settings","VariableCollectionUpsert"=>"variables","Deploy"=>"deploy","ProjectDelete"=>"delete",_=>panic!("unexpected {op}")};
            if kind=="delete"{assert!(native.removal_submitted);}else{assert!(native.effects.iter().any(|(k,e)|(k==kind||k.ends_with(&format!(":{kind}")))&&e.submitted),"intent must precede {op}");}
        }
        let data=match op {
            "OwnedProjects"=>{let rows=if remote.project.is_null()||remote.hidden{vec![]}else{let mut p=remote.project.clone();p["deletedAt"]=if remote.deleted{json!("2026-09-06T00:00:00Z")}else{Value::Null};vec![p]};json!({"projects":connection(rows)})},
            "ProjectCreate"=>{assert!(remote.project.is_null());assert_eq!(v["input"]["defaultEnvironmentName"],"production");remote.project=json!({"id":"proj_1","name":v["input"]["name"],"description":v["input"]["description"],"workspaceId":"workspace_1","deletedAt":null});json!({"projectCreate":{"id":"proj_1"}})},
            "OwnedProject"=>json!({"project":remote.project}),
            "OwnedEnvironments"=>json!({"project":{"id":"proj_1","environments":connection(vec![json!({"id":"env_1","name":"production","projectId":"proj_1","canAccess":true,"deletedAt":null})])}}),
            "OwnedServices"=>json!({"project":{"id":"proj_1","services":connection(if remote.service.is_null(){vec![]}else{vec![remote.service.clone()]})}}),
            "ServiceCreate"=>{assert!(remote.service.is_null());assert!(v["input"].get("source").is_none());remote.service=json!({"id":"svc_1","projectId":"proj_1","name":v["input"]["name"]});remote.variables=v["input"]["variables"].clone();assert!(remote.variables.get(lifecycle::SERVICE_RECEIPT).is_some());remote.settings=json!({"source":{"image":null,"repo":null},"rootDirectory":"/","startCommand":null});json!({"serviceCreate":{"id":"svc_1"}})},
            "OwnedService"=>json!({"service":remote.service}),
            "Variables"=>json!({"variables":remote.variables}),
            "VariableCollectionUpsert"=>{assert_eq!(v["input"]["skipDeploys"],true);assert_eq!(v["input"]["replace"],true);remote.variables=v["input"]["variables"].clone();json!({"variableCollectionUpsert":true})},
            "ServiceInstanceUpdate"=>{assert_eq!(v["serviceId"],"svc_1");assert_eq!(v["environmentId"],"env_1");remote.settings=v["input"].clone();json!({"serviceInstanceUpdate":true})},
            "ServiceInstance"=>{let mut value=remote.settings.clone();value["serviceId"]=json!("svc_1");value["environmentId"]=json!("env_1");value["activeDeployments"]=if remote.active{json!([{"id":"dep_1","status":"SUCCESS"}])}else{json!([])};json!({"serviceInstance":value})},
            "Domains"=>json!({"domains":{"serviceDomains":if remote.domain{vec![json!({"id":"domain_1","domain":remote.origin,"projectId":"proj_1","serviceId":"svc_1","environmentId":"env_1","deletedAt":null})]}else{vec![]}}}),
            "ServiceDomainCreate"=>{assert!(!remote.domain);remote.domain=true;json!({"serviceDomainCreate":{"id":"domain_1","domain":remote.origin}})},
            "Deploy"=>{assert!(!remote.active);assert_eq!(v["commitSha"],json!(remote.commit));remote.active=true;json!({"serviceInstanceDeployV2":"dep_1"})},
            "Deployment"|"DeploymentIdentity"=>{let native=native_state(&store);assert!(native.effects.values().any(|e|e.id.as_deref()==Some("dep_1")));json!({"deployment":{"id":"dep_1","projectId":"proj_1","serviceId":"svc_1","environmentId":"env_1","status":"SUCCESS","meta":{"commitHash":remote.commit}}})},
            "ProjectDelete"=>{if !remote.delayed_delete{remote.deleted=true;}json!({"projectDelete":true})},
            _=>panic!("unexpected query {query}"),
        };
        if remote.failures.remove(op){ResponseTemplate::new(503)}else{ResponseTemplate::new(200).set_body_json(json!({"data":data}))}
    }).mount(server).await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
}
#[derive(Default)]
struct CatalogRemote {
    resource: Option<String>,
    removed: bool,
    creates: usize,
    deletes: usize,
    lose_create: bool,
    lose_remove: bool,
}
struct Runner {
    store: Store,
    remote: Arc<StdMutex<CatalogRemote>>,
    native: Arc<StdMutex<NativeRemote>>,
}
#[async_trait]
impl CommandRunner for Runner {
    async fn run(&self, args: &[String], cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        let owner = self.store.instance("demo").unwrap().unwrap();
        Ok(match args[0].as_str() {
            "billing" => test_support::ok_empty(),
            "catalog" => test_support::raw(include_str!(
                "../../stackless-stripe-projects/tests/fixtures/catalog.json"
            )),
            "status" => test_support::ok(json!({"project":{"id":"stripe_project"}})),
            "services" => test_support::plans(&[("free", "free", "Railway")]),
            "env" if args.get(1).map(String::as_str) == Some("list") => {
                test_support::ok(json!({"environments":[{"name":owner.resource_namespace}]}))
            }
            "env" => {
                std::fs::write(
                    cwd.join(format!(".env.{}", owner.resource_namespace)),
                    "RAILWAY_URL=https://account.railway.app\nRAILWAY_API_TOKEN=test\n",
                )
                .unwrap();
                test_support::ok_empty()
            }
            "add" => {
                assert_eq!(args[1], "railway/hosting");
                let name = args[3].clone();
                let row = self
                    .store
                    .resource(
                        &owner.instance_id,
                        &format!("catalog:railway/hosting:{name}"),
                    )
                    .unwrap()
                    .unwrap();
                let value: Value = serde_json::from_str(&row.payload).unwrap();
                assert_eq!(value["_catalog_creation"]["submitted"], true);
                let mut remote = self.remote.lock().unwrap();
                assert_eq!(remote.creates, 0);
                remote.creates += 1;
                remote.resource = Some(name);
                if remote.lose_create {
                    remote.lose_create = false;
                    return Err(ProjectsError::Unavailable {
                        detail: "lost catalog creation response".into(),
                    });
                }
                test_support::ok(
                    json!({"service":{"key":"remote_project","provider_id":PROVIDER},"variables":{"RAILWAY_URL":"https://account.railway.app"}}),
                )
            }
            other => panic!("unexpected {other}"),
        })
    }
    async fn request(
        &self,
        method: &str,
        route: &str,
        _: &Path,
    ) -> Result<CommandOutput, ProjectsError> {
        let mut remote = self.remote.lock().unwrap();
        if method == "POST" {
            assert!(route.ends_with("/remote_project/remove"));
            let native = native_state(&self.store);
            assert!(
                native.absence_verified
                    || !native
                        .effects
                        .get("project-create")
                        .is_some_and(|e| e.submitted)
            );
            {
                let native = self.native.lock().unwrap();
                assert!(native.deleted || native.project.is_null());
            }
            remote.removed = true;
            remote.deletes += 1;
            if remote.lose_remove {
                remote.lose_remove = false;
                return Err(ProjectsError::Unavailable {
                    detail: "lost catalog removal response".into(),
                });
            }
        }
        let row=remote.resource.as_ref().map(|name|json!({"id":"remote_project","name":name,"provider":PROVIDER,"service_ref":"hosting","status":if remote.removed{"removed"}else{"complete"}}));
        let body = if route.contains('?') {
            json!({"data":row.into_iter().collect::<Vec<_>>(),"next_page_url":null})
        } else {
            row.unwrap_or(json!({"status":"removed"}))
        };
        Ok(CommandOutput {
            status: 0,
            stdout: body.to_string(),
            stderr: String::new(),
        })
    }
}

async fn engine_recovery(
    failures: &[&'static str],
    unknown_deploy: bool,
    delayed_delete: bool,
    abandon_catalog: bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let store = Store::open(&db).unwrap();
    let server = MockServer::start().await;
    let native = Arc::new(StdMutex::new(NativeRemote {
        origin: server.uri(),
        failures: failures.iter().copied().collect(),
        delayed_delete,
        ..Default::default()
    }));
    mount_native(&server, native.clone(), store.clone()).await;
    let remote = Arc::new(StdMutex::new(CatalogRemote {
        lose_create: true,
        lose_remove: true,
        ..Default::default()
    }));
    let make = |store: &Store| {
        let mut s = RailwaySubstrate::for_test(
            Runner {
                store: store.clone(),
                remote: remote.clone(),
                native: native.clone(),
            },
            dir.path(),
            Some(server.uri()),
            true,
        );
        s.secrets.insert("RAILWAY_API_TOKEN".into(), "test".into());
        s
    };
    let text = "[stack]\nname='fixture'\n[services.web]\nimage='nginx:alpine'\nrun='exec server --port $PORT'\nhealth={path='/'}\nenv={APP_MODE='fixture'}\n";
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
    assert!(
        engine
            .run_up(request(), admission)
            .await
            .unwrap_err()
            .to_string()
            .contains("lost catalog creation")
    );
    drop(substrate);
    drop(store);
    if !abandon_catalog {
        for _ in 0..failures.iter().filter(|s| **s != "ProjectDelete").count() {
            let store = Store::open(&db).unwrap();
            let s = make(&store);
            let error = Engine {
                store: &store,
                substrate: &s,
            }
            .up(request())
            .await
            .unwrap_err();
            assert!(error.to_string().contains("503"), "{error}");
        }
        let store = Store::open(&db).unwrap();
        let s = make(&store);
        let result = Engine {
            store: &store,
            substrate: &s,
        }
        .up(request())
        .await;
        assert_eq!(
            native.lock().unwrap().settings["source"],
            json!({"image":"nginx:alpine", "repo":null})
        );
        assert_eq!(
            native.lock().unwrap().settings["startCommand"],
            "/bin/sh -c 'exec server --port $PORT'"
        );
        if unknown_deploy {
            assert!(result.unwrap_err().to_string().contains("unknown"));
        } else {
            result.unwrap();
            let checkpoint = store.checkpoint("demo", "start:web").unwrap().unwrap();
            assert_eq!(
                s.observe(&InstanceContext::from_record(&owner, &[]), &checkpoint)
                    .await
                    .unwrap(),
                Observation::Present
            );
        }
    }
    let store = Store::open(&db).unwrap();
    let s = make(&store);
    let context = InstanceContext::from_record(&owner, &[]);
    let record = store
        .resources(&owner.instance_id)
        .unwrap()
        .into_iter()
        .find(|r| r.resource_kind == "railway-service")
        .unwrap();
    let sibling = InstanceContext {
        routed_origins: None,
        id: "foreign",
        name: "foreign",
        resource_namespace: "foreign",
        checkpoints: &[],
    };
    assert!(s.destroy_record(&store, &sibling, &record).await.is_err());
    if !abandon_catalog {
        for response in [ResponseTemplate::new(403),ResponseTemplate::new(200).set_body_json(json!({"errors":[{"message":"Not Authorized","extensions":{"code":"INTERNAL_SERVER_ERROR"}}],"data":null})),ResponseTemplate::new(200).set_body_json(json!({"data":{"projects":connection(vec![])}}))] {
            Mock::given(method("POST")).and(body_string_contains("query OwnedProjects")).respond_with(response).with_priority(1).up_to_n_times(2).expect(2).mount(&server).await;
            assert!(s.destroy_record(&store,&context,&record).await.is_err());assert!(s.observe_record(&store,&context,&record).await.is_err());assert_eq!(remote.lock().unwrap().deletes,0);
        }
        remote.lock().unwrap().removed = true;
        assert_eq!(
            s.observe_record(&store, &context, &record).await.unwrap(),
            Observation::Present
        );
        remote.lock().unwrap().removed = false;
    }
    assert!(
        Engine {
            store: &store,
            substrate: &s
        }
        .down("demo")
        .await
        .is_err()
    );
    if delayed_delete {
        let store = Store::open(&db).unwrap();
        let s = make(&store);
        assert!(
            Engine {
                store: &store,
                substrate: &s
            }
            .down("demo")
            .await
            .is_err()
        );
        assert_eq!(remote.lock().unwrap().deletes, 0);
        assert_eq!(native.lock().unwrap().counts["ProjectDelete"], 1);
        native.lock().unwrap().deleted = true;
    }
    // A lost native-delete reply and a lost catalog-remove reply each require a reopen.
    for _ in 0..3 {
        let store = Store::open(&db).unwrap();
        let s = make(&store);
        if (Engine {
            store: &store,
            substrate: &s,
        })
        .down("demo")
        .await
        .is_ok()
        {
            break;
        }
    }
    let store = Store::open(&db).unwrap();
    assert!(
        store
            .resources(&owner.instance_id)
            .unwrap()
            .iter()
            .all(|r| r.phase == ResourcePhase::Absent)
    );
    let state = native.lock().unwrap();
    for name in [
        "ProjectCreate",
        "ServiceCreate",
        "ServiceInstanceUpdate",
        "VariableCollectionUpsert",
        "ServiceDomainCreate",
        "Deploy",
        "ProjectDelete",
    ] {
        assert_eq!(
            state.counts.get(name).copied().unwrap_or(0),
            usize::from(!abandon_catalog),
            "{name}"
        );
    }
    let state = remote.lock().unwrap();
    assert_eq!((state.creates, state.deletes), (1, 1));
}
#[tokio::test]
async fn engine_recovers_every_returned_handle_and_applied_update_after_reopen() {
    engine_recovery(
        &[
            "ProjectCreate",
            "ServiceCreate",
            "ServiceInstanceUpdate",
            "VariableCollectionUpsert",
            "ServiceDomainCreate",
            "Deployment",
            "ProjectDelete",
        ],
        false,
        false,
        false,
    )
    .await;
}
#[tokio::test]
async fn unknown_deployment_and_pending_delete_never_repeat_mutations() {
    engine_recovery(&["Deploy", "ProjectDelete"], true, true, false).await;
}
#[tokio::test]
async fn down_after_lost_catalog_creation_does_not_create_native_resources() {
    engine_recovery(&[], false, false, true).await;
}

fn journal_fixture() -> (tempfile::TempDir, Store, lifecycle::Journal) {
    use stackless_core::state::{Ownership, ResourceIntent};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("state.db")).unwrap();
    let owner = store
        .create_instance("demo", "railway", "definition", &BTreeMap::new(), "", false)
        .unwrap();
    let resource = InstanceContext::from_record(&owner, &[]).resource_name("web");
    store
        .resource_intent(ResourceIntent {
            owner_id: &owner.instance_id,
            key: &format!("catalog:railway/hosting:{resource}"),
            step_id: "start:web",
            provider: "railway",
            ownership: Ownership::Owned,
            resource_kind: "railway-service",
            resource_id: &resource,
            payload: "{}",
            dependencies: &[],
        })
        .unwrap();
    store
        .resource_created(
            &owner.instance_id,
            &format!("catalog:railway/hosting:{resource}"),
            &resource,
            "{}",
        )
        .unwrap();
    let journal = lifecycle::Journal::for_record(&store, &owner.instance_id, &resource).unwrap();
    journal.initialize("project-name", "service-name").unwrap();
    (dir, store, journal)
}
async fn deploy(api: &RailwayApi) -> Result<railway_api::DeployOutcome, RailwayError> {
    api.deploy_service(
        "project-name",
        "service-name",
        ServiceSource::Image {
            image: "nginx:alpine".into(),
            start_command: None,
        },
        BTreeMap::from([("APP_MODE".into(), "test".into())]),
        "web",
        Duration::from_millis(50),
    )
    .await
}
#[tokio::test]
async fn missing_ambiguous_foreign_and_malformed_receipts_never_authorize_project_creation() {
    for mode in [
        "missing",
        "ambiguous",
        "foreign",
        "malformed",
        "repeated_cursor",
        "bad_timestamp",
    ] {
        let (_dir, store, journal) = journal_fixture();
        let server = MockServer::start().await;
        let remote = Arc::new(StdMutex::new(NativeRemote {
            origin: server.uri(),
            failures: BTreeSet::from(["ProjectCreate"]),
            ..Default::default()
        }));
        mount_native(&server, remote.clone(), store.clone()).await;
        let api = RailwayApi::with_base("test", server.uri()).with_journal(journal.clone());
        assert!(deploy(&api).await.unwrap_err().to_string().contains("503"));
        let mut node = remote.lock().unwrap().project.clone();
        let response = match mode {
            "missing" => connection(vec![]),
            "ambiguous" => {
                let mut other = node.clone();
                other["id"] = json!("proj_2");
                connection(vec![node, other])
            }
            "foreign" => {
                node["description"] = json!("another-owner");
                connection(vec![node])
            }
            "malformed" => json!({"edges":null,"pageInfo":{"hasNextPage":false}}),
            "bad_timestamp" => {
                node["deletedAt"] = json!("not-a-date-T-00000000");
                connection(vec![node])
            }
            "repeated_cursor" => {
                json!({"edges":[],"pageInfo":{"hasNextPage":true,"endCursor":"same"}})
            }
            _ => unreachable!(),
        };
        Mock::given(method("POST"))
            .and(body_string_contains("query OwnedProjects"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"data":{"projects":response}})),
            )
            .with_priority(1)
            .mount(&server)
            .await;
        assert!(deploy(&api).await.is_err(), "{mode}");
        assert!(api.remove_native_project(&journal).await.is_err(), "{mode}");
        assert_eq!(remote.lock().unwrap().counts["ProjectCreate"], 1);
        assert!(!remote.lock().unwrap().counts.contains_key("ProjectDelete"));
    }
}
#[tokio::test]
async fn preexisting_names_and_workspace_changes_are_not_adopted() {
    let (_dir, store, journal) = journal_fixture();
    let server = MockServer::start().await;
    let remote = Arc::new(StdMutex::new(NativeRemote {
        origin: server.uri(),
        project: json!({"id":"foreign_project","name":"project-name","description":"unowned","workspaceId":"workspace_1","deletedAt":null}),
        ..Default::default()
    }));
    mount_native(&server, remote.clone(), store.clone()).await;
    let api = RailwayApi::with_base("test", server.uri()).with_journal(journal.clone());
    assert!(
        deploy(&api)
            .await
            .unwrap_err()
            .to_string()
            .contains("existed before")
    );
    assert!(!remote.lock().unwrap().counts.contains_key("ProjectCreate"));
    remote.lock().unwrap().project = Value::Null;
    deploy(&api).await.unwrap();
    remote.lock().unwrap().project["workspaceId"] = json!("foreign_workspace");
    assert!(
        deploy(&api)
            .await
            .unwrap_err()
            .to_string()
            .contains("workspace")
    );
    assert!(
        api.remove_native_project(&journal)
            .await
            .unwrap_err()
            .to_string()
            .contains("workspace")
    );
    assert!(!remote.lock().unwrap().counts.contains_key("ProjectDelete"));
}
#[tokio::test]
async fn pagination_recovers_a_project_receipt_on_the_second_page() {
    let (_dir, store, journal) = journal_fixture();
    let server = MockServer::start().await;
    let remote = Arc::new(StdMutex::new(NativeRemote {
        origin: server.uri(),
        failures: BTreeSet::from(["ProjectCreate"]),
        ..Default::default()
    }));
    mount_native(&server, remote.clone(), store.clone()).await;
    let api = RailwayApi::with_base("test", server.uri()).with_journal(journal);
    assert!(deploy(&api).await.is_err());
    let node = remote.lock().unwrap().project.clone();
    Mock::given(method("POST")).and(body_string_contains("query OwnedProjects")).respond_with(move |r:&wiremock::Request|{
        let body:Value=r.body_json().unwrap();let rows=if body["variables"]["after"].is_null(){json!({"edges":[{"node":{"id":"foreign","name":"foreign","description":null,"workspaceId":null,"deletedAt":null}}],"pageInfo":{"hasNextPage":true,"endCursor":"second"}})}else{assert_eq!(body["variables"]["after"],"second");connection(vec![node.clone()])};ResponseTemplate::new(200).set_body_json(json!({"data":{"projects":rows}}))
    }).with_priority(1).expect(2).mount(&server).await;
    deploy(&api).await.unwrap();
    assert_eq!(remote.lock().unwrap().counts["ProjectCreate"], 1);
}
#[tokio::test]
async fn lost_service_and_domain_responses_require_unique_owned_children() {
    for kind in ["service", "domain"] {
        let (_dir, store, journal) = journal_fixture();
        let server = MockServer::start().await;
        let fail = if kind == "service" {
            "ServiceCreate"
        } else {
            "ServiceDomainCreate"
        };
        let remote = Arc::new(StdMutex::new(NativeRemote {
            origin: server.uri(),
            failures: BTreeSet::from([fail]),
            ..Default::default()
        }));
        mount_native(&server, remote.clone(), store.clone()).await;
        let api = RailwayApi::with_base("test", server.uri()).with_journal(journal.clone());
        assert!(deploy(&api).await.is_err());
        if kind == "service" {
            remote.lock().unwrap().variables[lifecycle::SERVICE_RECEIPT] = json!("foreign-owner");
            assert!(deploy(&api).await.is_err());
            assert_eq!(remote.lock().unwrap().counts[fail], 1);
            remote.lock().unwrap().variables[lifecycle::SERVICE_RECEIPT] =
                json!(journal.receipt().unwrap());
        } else {
            remote.lock().unwrap().domain = false;
            assert!(deploy(&api).await.is_err());
            assert_eq!(remote.lock().unwrap().counts[fail], 1);
            remote.lock().unwrap().domain = true;
        }
        deploy(&api).await.unwrap();
        assert_eq!(remote.lock().unwrap().counts[fail], 1);
    }
}
#[tokio::test]
async fn checkpoint_detects_configuration_drift_and_rejects_foreign_identity() {
    let (_dir, store, journal) = journal_fixture();
    let server = MockServer::start().await;
    let remote = Arc::new(StdMutex::new(NativeRemote {
        origin: server.uri(),
        ..Default::default()
    }));
    mount_native(&server, remote.clone(), store.clone()).await;
    let api = RailwayApi::with_base("test", server.uri()).with_journal(journal.clone());
    let outcome = deploy(&api).await.unwrap();
    let payload = RailwayPayload {
        native: Some(journal.load().unwrap()),
        stripe_resource: "resource".into(),
        url: outcome.origin.clone(),
        origin: outcome.origin,
        domain: Some(outcome.domain),
        service_name: "service-name".into(),
        project_id: outcome.project_id,
        environment_id: outcome.environment_id,
        railway_service_id: outcome.service_id,
        deployment_id: outcome.deployment_id,
        commit_sha: None,
    };
    assert!(api.checkpoint_ready(&payload).await.unwrap());
    remote.lock().unwrap().settings["rootDirectory"] = json!("/wrong");
    assert!(!api.checkpoint_ready(&payload).await.unwrap());
    remote.lock().unwrap().settings["rootDirectory"] = json!("/");
    remote.lock().unwrap().variables["APP_MODE"] = json!("wrong");
    assert!(!api.checkpoint_ready(&payload).await.unwrap());
    remote.lock().unwrap().variables["APP_MODE"] = json!("test");
    remote.lock().unwrap().active = false;
    assert!(!api.checkpoint_ready(&payload).await.unwrap());
    remote.lock().unwrap().active = true;
    remote.lock().unwrap().service["projectId"] = json!("foreign");
    assert!(api.checkpoint_ready(&payload).await.is_err());
    remote.lock().unwrap().service["projectId"] = json!("proj_1");
    assert!(api.checkpoint_ready(&payload).await.unwrap());
}
