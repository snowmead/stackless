use super::*;
use crate::gitlab_api::{GitLabApi, RepoFile};
use base64::Engine as _;
use serde_json::json;
use stackless_core::state::ResourceIntent;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const KEY: &str = "catalog:gitlab/project:owned-project";
struct Repo {
    head: String,
    trees: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    receipts: BTreeMap<String, String>,
    serving: String,
    commits: usize,
    lose_commit: Option<usize>,
    actions: Vec<Value>,
    malformed_tree: bool,
}
struct Fixture {
    _dir: tempfile::TempDir,
    db: PathBuf,
    owner: String,
    server: MockServer,
    repo: Arc<Mutex<Repo>>,
}
fn open(db: &Path, owner: &str, revision: &str) -> Journal {
    Journal {
        store: Store::open(db).unwrap(),
        owner: owner.into(),
        key: KEY.into(),
        revision: revision.into(),
    }
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let store = Store::open(&db).unwrap();
        let owner = store
            .create_instance("demo", "gitlab", "definition", &BTreeMap::new(), "", false)
            .unwrap()
            .instance_id;
        let payload =
            json!({"_catalog_creation":{"config":{"name":"owned-project","visibility":"private"}}})
                .to_string();
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                key: KEY,
                step_id: "start:web",
                provider: "gitlab",
                ownership: Ownership::Owned,
                resource_kind: "gitlab-project",
                resource_id: "owned-project",
                payload: &payload,
                dependencies: &[],
            })
            .unwrap();
        store
            .resource_created(&owner, KEY, "owned-project", &payload)
            .unwrap();
        open(&db, &owner, "one").bind(42).unwrap();
        let server = MockServer::start().await;
        let repo = Arc::new(Mutex::new(Repo {
            head: BASE.into(),
            trees: BTreeMap::from([(
                BASE.into(),
                BTreeMap::from([
                    ("README.md".into(), b"keep this".to_vec()),
                    ("public/old.js".into(), b"stale".to_vec()),
                ]),
            )]),
            receipts: BTreeMap::new(),
            serving: String::new(),
            commits: 0,
            lose_commit: None,
            actions: Vec::new(),
            malformed_tree: false,
        }));
        let state = repo.clone();
        let origin = server.uri();
        let handler_db = db.clone();
        let handler_owner = owner.clone();
        Mock::given(|_:&wiremock::Request|true).respond_with(move|r:&wiremock::Request|{
            let mut state=state.lock().unwrap();let path=r.url.path();let query:BTreeMap<_,_>=r.url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect();
            let value=if path=="/projects/42" {json!({"id":42,"default_branch":"main","path_with_namespace":"acme/owned-project","visibility":"private","empty_repo":false})}
            else if path=="/projects/42/repository/branches/main" {json!({"name":"main","commit":{"id":state.head}})}
            else if path=="/projects/42/repository/tree" {
                if state.malformed_tree {return ResponseTemplate::new(200).set_body_json(json!([{"path":"../foreign","id":BASE,"type":"blob"}]))}
                let sha=query.get("ref").unwrap();let tree=state.trees.get(sha).unwrap();
                let mut nodes:BTreeMap<String,Value>=BTreeMap::new();
                for name in tree.keys(){for (index,_) in name.match_indices('/') {let path=&name[..index];nodes.insert(path.into(),json!({"id":BASE,"path":path,"type":"tree"}));}nodes.insert(name.clone(),json!({"id":BASE,"path":name,"type":"blob"}));}
                let page=query.get("page").unwrap().parse::<usize>().unwrap();json!(nodes.values().skip((page-1)*100).take(100).collect::<Vec<_>>())
            }
            else if let Some(encoded)=path.strip_prefix("/projects/42/repository/files/") {
                let name=urlencoding::decode(encoded).unwrap();let sha=query.get("ref").unwrap();assert!(state.trees[sha].contains_key(name.as_ref()));json!({"file_path":name,"commit_id":sha,"last_commit_id":sha})
            }
            else if path=="/projects/42/repository/commits" && r.method.as_str()=="POST" {
                let body:Value=r.body_json().unwrap();let receipt=body["commit_message"].as_str().unwrap();
                let recorded=open(&handler_db,&handler_owner,"one").load().unwrap();let request=&recorded.requests[receipt];assert!(request.submitted);assert_eq!(request.base_commit.as_deref(),Some(state.head.as_str()));assert_eq!(request.actions_digest.as_deref(),Some(stackless_core::engine::revision::digest(&body["actions"]).unwrap().as_str()));
                let mut tree=state.trees[&state.head].clone();
                for action in body["actions"].as_array().unwrap(){let name=action["file_path"].as_str().unwrap();assert_ne!(name,"README.md");match action["action"].as_str().unwrap(){
                    "delete"=>{assert_eq!(action["last_commit_id"],state.head);assert!(tree.remove(name).is_some());},
                    "update"=>{assert_eq!(action["last_commit_id"],state.head);assert!(tree.contains_key(name));tree.insert(name.into(),base64::engine::general_purpose::STANDARD.decode(action["content"].as_str().unwrap()).unwrap());},
                    "create"=>{assert!(!tree.contains_key(name));tree.insert(name.into(),base64::engine::general_purpose::STANDARD.decode(action["content"].as_str().unwrap()).unwrap());},_=>panic!("unexpected action")}}
                state.commits+=1;let sha=format!("{:040x}",state.commits);state.head=sha.clone();state.trees.insert(sha.clone(),tree);state.receipts.insert(sha.clone(),receipt.into());state.serving=receipt.into();state.actions.push(body["actions"].clone());
                if state.lose_commit==Some(state.commits){return ResponseTemplate::new(503);}
                json!({"id":sha,"message":receipt})
            }
            else if path=="/projects/42/repository/commits" {json!(state.receipts.iter().map(|(id,message)|json!({"id":id,"message":message})).collect::<Vec<_>>())}
            else if let Some(sha)=path.strip_prefix("/projects/42/repository/commits/"){json!({"id":sha,"message":state.receipts[sha]})}
            else if path=="/projects/42/pipelines" {let sha=query.get("sha").unwrap();let id=u64::from_str_radix(sha,16).unwrap()+100;json!([{"id":id,"project_id":42,"ref":"main","sha":sha,"status":"success"}])}
            else if let Some(tail)=path.strip_prefix("/projects/42/pipelines/") {let id=tail.split('/').next().unwrap().parse::<u64>().unwrap();if tail.ends_with("/jobs"){json!([{"id":id+100,"name":"pages","status":"success"}])}else{json!({"id":id,"project_id":42,"ref":"main","sha":format!("{:040x}",id-100),"status":"success"})}}
            else if path=="/projects/42/pages" {json!({"deployments":[{"path_prefix":"","url":origin}]})}
            else if path=="/.well-known/stackless-deployment.json" {assert!(!r.headers.contains_key("private-token"));json!({"receipt":state.serving})}
            else {panic!("unexpected {} {}",r.method,r.url)};
            ResponseTemplate::new(200).set_body_json(value)
        }).mount(&server).await;
        Self {
            _dir: dir,
            db,
            owner,
            server,
            repo,
        }
    }
    fn api(&self, revision: &str) -> GitLabApi {
        GitLabApi::with_base("test", self.server.uri())
            .with_poll_interval(Duration::from_millis(1))
            .with_journal(open(&self.db, &self.owner, revision))
    }
    async fn deploy(
        &self,
        revision: &str,
        files: &[RepoFile],
    ) -> Result<crate::gitlab_api::PagesDeployResult, GitLabError> {
        self.api(revision)
            .deploy_pages("42", "main", files, "web", Duration::from_secs(1))
            .await
    }
}
fn files(entries: &[(&str, &[u8])]) -> Vec<RepoFile> {
    entries
        .iter()
        .map(|(path, content)| RepoFile {
            path: (*path).into(),
            content: content.to_vec(),
        })
        .collect()
}
#[tokio::test]
async fn lost_repair_response_recovers_one_new_generation_and_removes_foreign_serving_files() {
    let fixture = Fixture::new().await;
    let source = files(&[
        ("index.html", b"desired"),
        ("assets/app.js", b"binary\0\xff"),
    ]);
    fixture.deploy("one", &source).await.unwrap();
    let old_receipt = open(&fixture.db, &fixture.owner, "one").receipt().unwrap();
    {
        let mut remote = fixture.repo.lock().unwrap();
        let head = remote.head.clone();
        assert!(!remote.trees[&head].contains_key("public/old.js"));
        let mut external = remote.trees[&head].clone();
        external.insert("public/rogue.js".into(), b"rogue".to_vec());
        remote.head = "b".repeat(40);
        let head = remote.head.clone();
        remote.trees.insert(head, external);
        remote.serving = "foreign receipt".into();
        remote.lose_commit = Some(2);
    }
    assert!(
        fixture
            .deploy("one", &source)
            .await
            .unwrap_err()
            .to_string()
            .contains("503")
    );
    let after_loss = open(&fixture.db, &fixture.owner, "one");
    let receipt = after_loss.receipt().unwrap();
    assert_ne!(receipt, old_receipt);
    assert!(
        after_loss.load().unwrap().requests[&receipt]
            .commit_sha
            .is_none()
    );
    drop(after_loss);
    fixture.deploy("one", &source).await.unwrap();
    fixture.deploy("one", &source).await.unwrap();
    let state = open(&fixture.db, &fixture.owner, "one").load().unwrap();
    assert_eq!(state.requests.len(), 2);
    assert!(state.requests.values().all(|r| r.completed));
    let remote = fixture.repo.lock().unwrap();
    assert_eq!(remote.commits, 2);
    assert_eq!(remote.trees[&remote.head]["README.md"], b"keep this");
    assert!(!remote.trees[&remote.head].contains_key("public/rogue.js"));
    assert_eq!(
        remote.trees[&remote.head]["public/assets/app.js"],
        b"binary\0\xff"
    );
}
#[tokio::test]
async fn new_revision_deletes_stale_paths_and_handles_file_directory_replacements() {
    let fixture = Fixture::new().await;
    fixture
        .deploy(
            "one",
            &files(&[
                ("index.html", b"old"),
                ("assets/icon.svg", b"old-icon"),
                ("route", b"old-file"),
            ]),
        )
        .await
        .unwrap();
    fixture
        .deploy(
            "two",
            &files(&[
                ("index.html", b"new"),
                ("assets", b"now-file"),
                ("route/page.html", b"now-directory"),
            ]),
        )
        .await
        .unwrap();
    let remote = fixture.repo.lock().unwrap();
    let tree = &remote.trees[&remote.head];
    assert_eq!(remote.commits, 2);
    assert!(!tree.contains_key("public/assets/icon.svg"));
    assert!(!tree.contains_key("public/route"));
    assert_eq!(tree["public/assets"], b"now-file");
    assert_eq!(tree["public/route/page.html"], b"now-directory");
    assert_eq!(tree["README.md"], b"keep this");
}
#[tokio::test]
async fn malformed_inventory_cannot_submit_a_repair_or_delete_paths() {
    let fixture = Fixture::new().await;
    fixture.repo.lock().unwrap().malformed_tree = true;
    assert!(
        fixture
            .deploy("one", &files(&[("index.html", b"desired")]))
            .await
            .is_err()
    );
    assert_eq!(fixture.repo.lock().unwrap().commits, 0);
    let state = open(&fixture.db, &fixture.owner, "one").load().unwrap();
    assert!(state.requests.values().all(|r| !r.submitted));
}

#[tokio::test]
async fn serving_drift_repairs_even_when_the_branch_still_names_the_recorded_commit() {
    let fixture = Fixture::new().await;
    let source = files(&[("index.html", b"desired")]);
    fixture.deploy("one", &source).await.unwrap();
    let journal = open(&fixture.db, &fixture.owner, "one");
    let original = journal.receipt().unwrap();
    fixture.repo.lock().unwrap().serving = "an older deployment".into();
    fixture.deploy("one", &source).await.unwrap();
    assert_ne!(journal.receipt().unwrap(), original);
    assert_eq!(fixture.repo.lock().unwrap().commits, 2);
}
