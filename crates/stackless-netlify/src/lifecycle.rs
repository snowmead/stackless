//! Native request state stays inside the owned catalog site's inventory entry.

use crate::error::NetlifyError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stackless_core::state::{Ownership, Store};
use stackless_core::substrate::StepContext;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NativeState {
    pub site_id: Option<String>,
    pub site_submitted: bool,
    #[serde(default)]
    pub site_conflict: bool,
    pub removal_submitted: bool,
    pub requests: BTreeMap<String, Request>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    pub fingerprint: String,
    pub submitted: bool,
    pub deploy_id: Option<String>,
    pub build_id: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Journal {
    store: Store,
    owner: String,
    key: String,
    revision: String,
}

pub(crate) fn invalid(detail: impl Into<String>) -> NetlifyError {
    NetlifyError::ConfigInvalid {
        location: "native resource journal".into(),
        detail: detail.into(),
    }
}

impl Journal {
    pub fn new(
        ctx: &StepContext<'_>,
        resource: &str,
        revision: String,
    ) -> Result<Self, NetlifyError> {
        let journal = Self {
            store: ctx.store.clone(),
            owner: ctx.instance.id.into(),
            key: format!("catalog:netlify/project:{resource}"),
            revision,
        };
        journal.load()?;
        Ok(journal)
    }
    fn payload(&self) -> Result<Value, NetlifyError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        if record.provider != "netlify"
            || record.resource_kind != "netlify-site"
            || record.ownership != Ownership::Owned
        {
            return Err(invalid("native operations require an owned Netlify site"));
        }
        serde_json::from_str(&record.payload).map_err(|e| invalid(e.to_string()))
    }
    pub fn load(&self) -> Result<NativeState, NetlifyError> {
        let value = self.payload()?;
        match value.get("_netlify") {
            None | Some(Value::Null) => Ok(NativeState::default()),
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|e| invalid(e.to_string()))
            }
        }
    }
    pub fn save(&self, state: &NativeState) -> Result<(), NetlifyError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        let mut value = self.payload()?;
        value["_netlify"] = serde_json::to_value(state).map_err(|e| invalid(e.to_string()))?;
        self.store
            .resource_refresh_payload(
                &self.owner,
                &self.key,
                &record.resource_id,
                &value.to_string(),
            )
            .map_err(|e| invalid(e.to_string()))
    }
    pub fn site(&self, id: &str) -> Result<(), NetlifyError> {
        let mut state = self.load()?;
        if id.is_empty()
            || state
                .site_id
                .as_deref()
                .is_some_and(|previous| previous != id)
        {
            return Err(invalid("native site ID changed"));
        }
        state.site_id = Some(id.into());
        self.save(&state)
    }
    pub fn begin(
        &self,
        kind: &str,
        fingerprint: String,
    ) -> Result<(String, Request), NetlifyError> {
        let digest = stackless_core::engine::revision::digest(&(
            &self.owner,
            &self.key,
            &self.revision,
            kind,
        ))
        .map_err(|e| invalid(e.message))?;
        let receipt = format!("stackless:{digest}");
        let mut state = self.load()?;
        if let Some(request) = state.requests.get(&receipt) {
            if request.fingerprint != fingerprint {
                return Err(invalid(
                    "source changed during a recorded deployment attempt",
                ));
            }
            return Ok((receipt, request.clone()));
        }
        if state
            .requests
            .values()
            .any(|request| request.submitted && request.deploy_id.is_none())
        {
            return Err(invalid("an earlier native submission is unresolved"));
        }
        let request = Request {
            fingerprint,
            submitted: false,
            deploy_id: None,
            build_id: None,
        };
        state.requests.insert(receipt.clone(), request.clone());
        self.save(&state)?;
        Ok((receipt, request))
    }
    pub fn submit(&self, receipt: &str) -> Result<(), NetlifyError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("deployment intent missing"))?;
        if request.submitted {
            return Err(invalid("deployment was already submitted"));
        }
        request.submitted = true;
        self.save(&state)
    }
    pub fn response(
        &self,
        receipt: &str,
        deploy: Option<&str>,
        build: Option<&str>,
    ) -> Result<(), NetlifyError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("deployment intent missing"))?;
        for (slot, value) in [
            (&mut request.deploy_id, deploy),
            (&mut request.build_id, build),
        ] {
            if let Some(value) = value {
                if value.is_empty() || slot.as_deref().is_some_and(|old| old != value) {
                    return Err(invalid("native response changed its recorded ID"));
                }
                *slot = Some(value.into());
            }
        }
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netlify_api::NetlifyApi;
    use serde_json::json;
    use stackless_core::state::ResourceIntent;
    use std::{
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    const KEY: &str = "catalog:netlify/project:owned-site";
    fn open(db: &Path, owner: &str) -> Journal {
        Journal {
            store: Store::open(db).unwrap(),
            owner: owner.into(),
            key: KEY.into(),
            revision: "revision-one".into(),
        }
    }
    fn create(db: &Path) -> String {
        let store = Store::open(db).unwrap();
        let owner = store
            .create_instance("demo", "netlify", "definition", &BTreeMap::new(), "", false)
            .unwrap()
            .instance_id;
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                key: KEY,
                step_id: "start:web",
                provider: "netlify",
                ownership: Ownership::Owned,
                resource_kind: "netlify-site",
                resource_id: "owned-site",
                payload: "{}",
                dependencies: &[],
            })
            .unwrap();
        store
            .resource_created(&owner, KEY, "owned-site", "{}")
            .unwrap();
        owner
    }
    fn api(server: &MockServer, journal: Journal) -> NetlifyApi {
        NetlifyApi::with_base("token", server.uri())
            .with_journal(journal)
            .with_poll_interval(Duration::from_millis(1))
    }
    async fn build(api: &NetlifyApi, zip: bool) -> Result<(String, String), NetlifyError> {
        if zip {
            api.deploy_build_zip(
                "site_one",
                b"PK\x03\x04fixed source".to_vec(),
                "build",
                "web",
                Duration::from_secs(1),
            )
            .await
        } else {
            api.deploy_build_git(
                "site_one",
                "feature/receipt & recovery",
                "web",
                Duration::from_secs(1),
            )
            .await
        }
    }

    #[tokio::test]
    async fn git_and_zip_builds_recover_lost_responses_and_queued_build_ids() {
        for zip in [false, true] {
            for queued in [false, true] {
                let server = MockServer::start().await;
                let dir = tempfile::tempdir().unwrap();
                let db = dir.path().join("state.db");
                let owner = create(&db);
                let receipt = Arc::new(Mutex::new(String::new()));
                let db_copy = db.clone();
                let owner_copy = owner.clone();
                let receipt_copy = receipt.clone();
                Mock::given(method("POST"))
                    .and(path("/sites/site_one/builds"))
                    .respond_with(move |req: &wiremock::Request| {
                        let journal = open(&db_copy, &owner_copy);
                        let state = journal.load().unwrap();
                        assert_eq!(state.requests.len(), 1);
                        let (saved_receipt, request) = state.requests.iter().next().unwrap();
                        assert!(request.submitted);
                        assert!(request.build_id.is_none());
                        assert!(request.deploy_id.is_none());
                        let params: BTreeMap<_, _> = req.url.query_pairs().into_owned().collect();
                        assert_eq!(params["title"], *saved_receipt);
                        if zip {
                            assert!(String::from_utf8_lossy(&req.body).contains("name=\"zip\""));
                            assert!(req.body.windows(12).any(|bytes| bytes == b"fixed source"));
                        } else {
                            assert_eq!(params["branch"], "feature/receipt & recovery");
                            assert_eq!(params["clear_cache"], "true");
                        }
                        *receipt_copy.lock().unwrap() = saved_receipt.clone();
                        if queued {
                            ResponseTemplate::new(202).set_body_json(json!({"id":"build_one"}))
                        } else {
                            ResponseTemplate::new(500)
                        }
                    })
                    .expect(1)
                    .mount(&server)
                    .await;
                let initial = api(&server, open(&db, &owner));
                assert!(build(&initial, zip).await.is_err());
                drop(initial);
                let saved = open(&db, &owner).load().unwrap();
                let request = saved.requests.values().next().unwrap();
                assert!(request.submitted);
                assert_eq!(request.build_id.as_deref(), queued.then_some("build_one"));
                assert!(request.deploy_id.is_none());
                let deploy = json!({"id":"deploy_one", "site_id":"site_one", "build_id":"build_one",
                    "title":receipt.lock().unwrap().clone(), "state":"ready", "ssl_url":"https://actual.netlify.app"});
                Mock::given(method("GET"))
                    .and(path("/builds/build_one"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(json!({"id":"build_one", "deploy_id":"deploy_one"})),
                    )
                    .expect(if queued { 1 } else { 0 })
                    .mount(&server)
                    .await;
                Mock::given(method("GET"))
                    .and(path("/sites/site_one/deploys"))
                    .and(query_param("per_page", "100"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!([deploy.clone()])))
                    .expect(if queued { 0 } else { 1 })
                    .mount(&server)
                    .await;
                Mock::given(method("GET"))
                    .and(path("/sites/site_one/deploys/deploy_one"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(deploy))
                    .mount(&server)
                    .await;
                let recovered = api(&server, open(&db, &owner));
                assert_eq!(
                    build(&recovered, zip).await.unwrap(),
                    ("https://actual.netlify.app".into(), "deploy_one".into())
                );
                let saved = open(&db, &owner).load().unwrap();
                let request = saved.requests.values().next().unwrap();
                assert_eq!(request.deploy_id.as_deref(), Some("deploy_one"));
                assert_eq!(request.build_id.as_deref(), Some("build_one"));
                server.verify().await;
            }
        }
    }

    #[tokio::test]
    async fn native_site_creation_recovers_by_name_after_lost_response() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        let created = Arc::new(Mutex::new(false));
        let created_copy = created.clone();
        Mock::given(method("GET")).and(path("/sites")).and(query_param("name", "owned-site"))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(if *created_copy.lock().unwrap() {
                    json!([{"id":"site_one", "name":"owned-site", "ssl_url":"https://actual.netlify.app"}])
                } else { json!([]) })
            }).expect(2).mount(&server).await;
        let db_copy = db.clone();
        let owner_copy = owner.clone();
        Mock::given(method("POST"))
            .and(path("/sites"))
            .respond_with(move |_: &wiremock::Request| {
                let state = open(&db_copy, &owner_copy).load().unwrap();
                assert!(state.site_submitted);
                assert!(state.site_id.is_none());
                *created.lock().unwrap() = true;
                ResponseTemplate::new(500)
            })
            .expect(1)
            .mount(&server)
            .await;
        let initial = api(&server, open(&db, &owner));
        assert!(initial.create_site("owned-site").await.is_err());
        drop(initial);
        let recovered = api(&server, open(&db, &owner));
        assert_eq!(
            recovered.create_site("owned-site").await.unwrap().id,
            "site_one"
        );
        assert_eq!(
            open(&db, &owner).load().unwrap().site_id.as_deref(),
            Some("site_one")
        );
    }

    #[tokio::test]
    async fn preexisting_native_site_is_an_ownership_conflict() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        Mock::given(method("GET"))
            .and(path("/sites"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"id":"foreign", "name":"owned-site"}])),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        assert!(
            api(&server, open(&db, &owner))
                .create_site("owned-site")
                .await
                .is_err()
        );
        let state = open(&db, &owner).load().unwrap();
        assert!(state.site_conflict);
        assert!(!state.site_submitted);
        assert!(state.site_id.is_none());
    }

    #[tokio::test]
    async fn unresolved_and_ambiguous_deployment_inventory_never_authorizes_resubmission() {
        for case in [
            "absent",
            "duplicate",
            "foreign",
            "malformed",
            "unauthorized",
        ] {
            let server = MockServer::start().await;
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("state.db");
            let owner = create(&db);
            let journal = open(&db, &owner);
            let fingerprint = stackless_core::engine::revision::digest(&(
                "site_one",
                "feature/receipt & recovery",
            ))
            .unwrap();
            let (receipt, _) = journal.begin("build-git", fingerprint).unwrap();
            journal.submit(&receipt).unwrap();
            let own = json!({"id":"deploy_one", "site_id":"site_one", "title":receipt});
            let rows = match case {
                "absent" => json!([]),
                "duplicate" => {
                    json!([own, {"id":"deploy_two", "site_id":"site_one", "title":receipt}])
                }
                "foreign" => json!([{"id":"deploy_one", "site_id":"other_site", "title":receipt}]),
                "malformed" => json!([{"site_id":"site_one", "title":receipt}]),
                _ => json!({"error":"denied"}),
            };
            Mock::given(method("GET"))
                .and(path("/sites/site_one/deploys"))
                .respond_with(
                    ResponseTemplate::new(if case == "unauthorized" { 401 } else { 200 })
                        .set_body_json(rows),
                )
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
            assert!(
                build(&api(&server, journal), false).await.is_err(),
                "{case}"
            );
            let state = open(&db, &owner).load().unwrap();
            assert!(state.requests[&receipt].submitted);
            assert!(state.requests[&receipt].deploy_id.is_none());
            server.verify().await;
        }
    }

    #[test]
    fn changed_source_cannot_replace_an_unresolved_request() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db);
        let journal = open(&db, &owner);
        let (receipt, _) = journal
            .begin("deploy-files", "original-files".into())
            .unwrap();
        journal.submit(&receipt).unwrap();
        assert!(
            journal
                .begin("deploy-files", "changed-files".into())
                .is_err()
        );
        let mut next = open(&db, &owner);
        next.revision = "next-revision".into();
        assert!(next.begin("deploy-files", "changed-files".into()).is_err());
        assert_eq!(journal.load().unwrap().requests.len(), 1);
    }
}
