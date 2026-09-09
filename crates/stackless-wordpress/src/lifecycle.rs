//! Page receipts and site identity belong to the catalog ownership record.
use crate::error::WordPressError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stackless_core::{
    state::{Ownership, Store},
    substrate::StepContext,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct NativeState {
    pub site_id: Option<u64>,
    pub origin: Option<String>,
    pub removal_submitted: bool,
    pub requests: BTreeMap<String, Request>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Request {
    pub fingerprint: String,
    pub submitted: bool,
    pub page_id: Option<u64>,
    pub homepage_submitted: bool,
    #[serde(default)]
    pub content_update_submitted: bool,
}
#[derive(Clone)]
pub(crate) struct Journal {
    store: Store,
    owner: String,
    key: String,
    revision: String,
}
pub(crate) fn invalid(detail: impl Into<String>) -> WordPressError {
    WordPressError::ConfigInvalid {
        location: "native resource journal".into(),
        detail: detail.into(),
    }
}
impl Journal {
    pub fn new(
        ctx: &StepContext<'_>,
        resource: &str,
        revision: String,
    ) -> Result<Self, WordPressError> {
        let journal = Self {
            store: ctx.store.clone(),
            owner: ctx.instance.id.into(),
            key: format!("catalog:wordpress.com/site:{resource}"),
            revision,
        };
        journal.load()?;
        Ok(journal)
    }
    fn payload(&self) -> Result<Value, WordPressError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        if record.provider != "wordpress"
            || record.resource_kind != "wordpress-site"
            || record.ownership != Ownership::Owned
        {
            return Err(invalid("native operations require an owned WordPress site"));
        }
        serde_json::from_str(&record.payload).map_err(|e| invalid(e.to_string()))
    }
    pub fn load(&self) -> Result<NativeState, WordPressError> {
        match self.payload()?.get("_wordpress") {
            None | Some(Value::Null) => Ok(NativeState::default()),
            Some(value) => {
                serde_json::from_value(value.clone()).map_err(|e| invalid(e.to_string()))
            }
        }
    }
    fn save(&self, state: &NativeState) -> Result<(), WordPressError> {
        let record = self
            .store
            .resource(&self.owner, &self.key)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| invalid("catalog ownership record is missing"))?;
        let mut value = self.payload()?;
        value["_wordpress"] = serde_json::to_value(state).map_err(|e| invalid(e.to_string()))?;
        self.store
            .resource_refresh_payload(
                &self.owner,
                &self.key,
                &record.resource_id,
                &value.to_string(),
            )
            .map_err(|e| invalid(e.to_string()))
    }
    pub fn bind(&self, site_id: u64, origin: &str) -> Result<(), WordPressError> {
        let mut state = self.load()?;
        if site_id == 0
            || state.site_id.is_some_and(|old| old != site_id)
            || state.origin.as_deref().is_some_and(|old| old != origin)
        {
            return Err(invalid("native site identity is invalid or changed"));
        }
        state.site_id = Some(site_id);
        state.origin = Some(origin.into());
        self.save(&state)
    }
    pub fn receipt(&self) -> Result<String, WordPressError> {
        let hash =
            stackless_core::engine::revision::digest(&(&self.owner, &self.key, &self.revision))
                .map_err(|e| invalid(e.message))?;
        Ok(format!("stackless-{hash}"))
    }
    pub fn begin(
        &self,
        site: &str,
        fingerprint: String,
    ) -> Result<(String, Request), WordPressError> {
        let mut state = self.load()?;
        if state.site_id.map(|id| id.to_string()).as_deref() != Some(site)
            || state.removal_submitted
        {
            return Err(invalid(
                "site differs from its ownership record or teardown has started",
            ));
        }
        let receipt = self.receipt()?;
        if let Some(request) = state.requests.get(&receipt) {
            if request.fingerprint != fingerprint {
                return Err(invalid("page contents changed during this revision"));
            }
            return Ok((receipt, request.clone()));
        }
        if state
            .requests
            .values()
            .any(|r| r.submitted && r.page_id.is_none())
        {
            return Err(invalid("an earlier page submission is unresolved"));
        }
        let request = Request {
            fingerprint,
            submitted: false,
            page_id: None,
            homepage_submitted: false,
            content_update_submitted: false,
        };
        state.requests.insert(receipt.clone(), request.clone());
        self.save(&state)?;
        Ok((receipt, request))
    }
    pub fn submitted(&self, receipt: &str) -> Result<(), WordPressError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("page intent missing"))?;
        if request.submitted {
            return Err(invalid("page was already submitted"));
        }
        request.submitted = true;
        self.save(&state)
    }
    pub fn page(&self, receipt: &str, id: u64) -> Result<(), WordPressError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("page intent missing"))?;
        if !request.submitted || id == 0 || request.page_id.is_some_and(|old| old != id) {
            return Err(invalid("page ID differs from the submitted request"));
        }
        request.page_id = Some(id);
        self.save(&state)
    }
    pub fn content_update(&self, receipt: &str) -> Result<(), WordPressError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("page intent missing"))?;
        if request.page_id.is_none() {
            return Err(invalid("content update has no recorded page"));
        }
        request.content_update_submitted = true;
        self.save(&state)
    }

    pub fn homepage(&self, receipt: &str) -> Result<(), WordPressError> {
        let mut state = self.load()?;
        let request = state
            .requests
            .get_mut(receipt)
            .ok_or_else(|| invalid("page intent missing"))?;
        if request.page_id.is_none() {
            return Err(invalid("homepage has no recorded page"));
        }
        request.homepage_submitted = true;
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wordpress_api::WordPressApi;
    use serde_json::json;
    use stackless_core::state::ResourceIntent;
    use std::path::Path;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, path_regex},
    };
    const KEY: &str = "catalog:wordpress.com/site:owned-site";
    fn open(db: &Path, owner: &str) -> Journal {
        Journal {
            store: Store::open(db).unwrap(),
            owner: owner.into(),
            key: KEY.into(),
            revision: "revision-one".into(),
        }
    }
    fn create(db: &Path, origin: &str) -> String {
        let store = Store::open(db).unwrap();
        let owner = store
            .create_instance(
                "demo",
                "wordpress",
                "definition",
                &BTreeMap::new(),
                "",
                false,
            )
            .unwrap()
            .instance_id;
        let payload = json!({"_catalog_creation":{"config":{"plan":"free"}}}).to_string();
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                key: KEY,
                step_id: "start:web",
                provider: "wordpress",
                ownership: Ownership::Owned,
                resource_kind: "wordpress-site",
                resource_id: "owned-site",
                payload: &payload,
                dependencies: &[],
            })
            .unwrap();
        store
            .resource_created(&owner, KEY, "owned-site", &payload)
            .unwrap();
        open(db, &owner).bind(99, origin).unwrap();
        owner
    }
    #[tokio::test]
    async fn missing_foreign_or_ambiguous_receipts_never_authorize_another_page() {
        for mode in 0..6 {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("state.db");
            let server = MockServer::start().await;
            let owner = create(&db, &server.uri());
            let journal = open(&db, &owner);
            let receipt = journal.receipt().unwrap();
            let content = format!("<p>original</p>\n<!-- {receipt} -->");
            let fingerprint =
                stackless_core::engine::revision::digest(&("title", &content)).unwrap();
            journal.begin("99", fingerprint).unwrap();
            if mode != 5 {
                journal.submitted(&receipt).unwrap();
            }
            drop(journal);
            Mock::given(method("GET")).and(path("/sites/99"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ID":99,"URL":server.uri(),"is_private":false,"is_coming_soon":false,"user_can_manage":true}))).mount(&server).await;
            let mut page = json!({"ID":42,"site_ID":99,"type":"page","status":"publish","title":"title","content":content,"has_password":false,"URL":format!("{}/page/",server.uri()),"slug":receipt,"metadata":[{"key":"stackless_deployment_receipt","value":receipt}]});
            match mode {
                1 => page["metadata"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"key":"stackless_deployment_receipt","value":receipt})),
                2 => page["site_ID"] = json!(100),
                3 => page["slug"] = json!("foreign-slug"),
                4 => page["content"] = json!("changed content"),
                _ => (),
            }
            Mock::given(method("GET"))
                .and(path_regex("^/sites/99/posts/slug:"))
                .respond_with(if mode == 0 {
                    ResponseTemplate::new(404)
                } else {
                    ResponseTemplate::new(200).set_body_json(page)
                })
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
            let api = WordPressApi::with_base("test", server.uri()).with_journal(open(&db, &owner));
            assert!(
                api.deploy_page("99", "web", "title", "<p>original</p>")
                    .await
                    .is_err()
            );
            assert!(
                open(&db, &owner).load().unwrap().requests[&receipt]
                    .page_id
                    .is_none()
            );
        }
    }
    #[tokio::test]
    async fn lost_content_repair_and_homepage_responses_resume_without_duplicate_updates() {
        use std::sync::{Arc, Mutex};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let server = MockServer::start().await;
        let owner = create(&db, &server.uri());
        let journal = open(&db, &owner);
        let receipt = journal.receipt().unwrap();
        let content = format!("<p>original</p>\n<!-- {receipt} -->");
        let expected = stackless_core::engine::revision::digest(&("title", &content)).unwrap();
        journal.begin("99", expected).unwrap();
        journal.submitted(&receipt).unwrap();
        journal.page(&receipt, 42).unwrap();
        drop(journal);
        let page = Arc::new(Mutex::new(
            json!({"ID":42,"site_ID":99,"type":"page","status":"draft","title":"title","content":"externally changed","has_password":false,"URL":format!("{}/page/",server.uri()),"slug":receipt,"metadata":[{"key":"stackless_deployment_receipt","value":receipt}]}),
        ));
        Mock::given(method("GET")).and(path("/sites/99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ID":99,"URL":server.uri(),"is_private":false,"is_coming_soon":false,"user_can_manage":true}))).mount(&server).await;
        let saved = page.clone();
        Mock::given(method("GET"))
            .and(path("/sites/99/posts/42"))
            .respond_with(move |_: &wiremock::Request| {
                ResponseTemplate::new(200).set_body_json(saved.lock().unwrap().clone())
            })
            .mount(&server)
            .await;
        let saved = page.clone();
        let update_db = db.clone();
        let update_owner = owner.clone();
        let update_receipt = receipt.clone();
        Mock::given(method("POST"))
            .and(path("/sites/99/posts/42"))
            .respond_with(move |request: &wiremock::Request| {
                let journal = open(&update_db, &update_owner);
                assert!(journal.load().unwrap().requests[&update_receipt].content_update_submitted);
                let body: Value = request.body_json().unwrap();
                assert_eq!(body["content"], content);
                let mut saved = saved.lock().unwrap();
                saved["content"] = body["content"].clone();
                saved["status"] = body["status"].clone();
                ResponseTemplate::new(503)
            })
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sites/99/posts/new"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;
        let homepage = Arc::new(Mutex::new(false));
        let state = homepage.clone();
        Mock::given(method("GET")).and(path("/sites/99/settings"))
            .respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(json!({"settings":{"show_on_front":if *state.lock().unwrap() {"page"} else {"posts"},"page_on_front":42}}))).mount(&server).await;
        let setting_db = db.clone();
        let setting_owner = owner.clone();
        Mock::given(method("POST"))
            .and(path("/sites/99/settings"))
            .respond_with(move |_: &wiremock::Request| {
                assert!(
                    open(&setting_db, &setting_owner).load().unwrap().requests[&receipt]
                        .homepage_submitted
                );
                *homepage.lock().unwrap() = true;
                ResponseTemplate::new(503)
            })
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(move |request: &wiremock::Request| {
                assert!(!request.headers.contains_key("authorization"));
                ResponseTemplate::new(200)
                    .set_body_string(page.lock().unwrap()["content"].as_str().unwrap())
            })
            .mount(&server)
            .await;
        for attempt in 0..3 {
            let api = WordPressApi::with_base("test", server.uri()).with_journal(open(&db, &owner));
            let result = api
                .deploy_page("99", "web", "title", "<p>original</p>")
                .await;
            if attempt < 2 {
                assert!(result.unwrap_err().to_string().contains("status 503"));
            } else {
                assert_eq!(result.unwrap().page_id, "42");
            }
        }
    }

    #[test]
    fn journal_rejects_changed_site_source_and_unresolved_prior_revision() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let owner = create(&db, "https://owned.wordpress.com");
        let journal = open(&db, &owner);
        assert!(journal.bind(100, "https://owned.wordpress.com").is_err());
        assert!(journal.bind(99, "https://foreign.wordpress.com").is_err());
        let (receipt, _) = journal.begin("99", "source".into()).unwrap();
        journal.submitted(&receipt).unwrap();
        drop(journal);
        let mut journal = open(&db, &owner);
        assert!(journal.begin("100", "source".into()).is_err());
        assert!(journal.begin("99", "changed".into()).is_err());
        journal.revision = "revision-two".into();
        assert!(journal.begin("99", "source".into()).is_err());
        journal.page(&receipt, 42).unwrap();
        assert!(journal.page(&receipt, 43).is_err());
        journal.begin("99", "source-two".into()).unwrap();
    }
}
