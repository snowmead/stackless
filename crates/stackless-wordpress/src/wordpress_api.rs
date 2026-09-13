//! WordPress.com REST client (`public-api.wordpress.com/rest/v1.1`): publish
//! static HTML as a page, set it as the site front page when the API allows,
//! and pull activity or deploy summaries for `stackless logs`.

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::{Client, Method};
use serde_json::{Value, json};

use crate::error::WordPressError;

const DEFAULT_BASE: &str = "https://public-api.wordpress.com/rest/v1.1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
pub struct DeployResult {
    pub page_id: String,
    pub page_url: Option<String>,
    pub status: String,
    pub homepage_set: bool,
}

pub struct WordPressApi {
    client: Result<Client, String>,
    journal: Option<crate::lifecycle::Journal>,
    serving_client: Result<Client, String>,
    base: String,
}

impl std::fmt::Debug for WordPressApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WordPressApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

fn authed_client(token: &str) -> Result<Client, String> {
    if token.trim().is_empty() {
        return Err("WordPress access token is empty".into());
    }
    let mut headers = HeaderMap::new();
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "WordPress token is not a valid header".to_owned())?;
    value.set_sensitive(true);
    headers.insert(AUTHORIZATION, value);
    Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

fn api_failed(method: &str, path: &str, err: impl std::fmt::Display) -> WordPressError {
    WordPressError::ApiFailed {
        method: method.to_owned(),
        path: path.to_owned(),
        detail: err.to_string(),
    }
}

fn truncate(text: &str) -> String {
    const MAX: usize = 400;
    if text.len() <= MAX {
        text.to_owned()
    } else {
        format!("{}…", &text[..text.floor_char_boundary(MAX)])
    }
}

/// Site identifier for `/sites/{site}/…` — host from an `https://…` URL or the
/// raw slug/domain Stripe returns.
pub fn site_identifier_from_url(site_url: &str) -> Result<String, WordPressError> {
    let origin = normalize_origin(site_url)?;
    let url = reqwest::Url::parse(&origin).map_err(|e| api_failed("GET", "site URL", e))?;
    url.host_str()
        .map(str::to_owned)
        .ok_or_else(|| api_failed("GET", "site URL", "missing host"))
}

pub(crate) fn normalize_origin(raw: &str) -> Result<String, WordPressError> {
    let raw = raw.trim();
    let value = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    let url = reqwest::Url::parse(&value).map_err(|e| api_failed("GET", "site URL", e))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(api_failed(
            "GET",
            "site URL",
            "expected an HTTP(S) origin without credentials or a path",
        ));
    }
    Ok(url.as_str().trim_end_matches('/').into())
}
fn numeric_id(value: &Value, key: &str) -> Result<u64, WordPressError> {
    value
        .get(key)
        .and_then(|id| id.as_u64().or_else(|| id.as_str()?.parse().ok()))
        .filter(|id| *id > 0)
        .ok_or_else(|| api_failed("GET", "identity", format!("missing or invalid {key}")))
}
fn fingerprint(title: &str, content: &str) -> Result<String, WordPressError> {
    stackless_core::engine::revision::digest(&(title, content))
        .map_err(|e| crate::lifecycle::invalid(e.message))
}

impl WordPressApi {
    pub fn new(token: impl AsRef<str>) -> Self {
        Self::with_base(token, DEFAULT_BASE)
    }

    pub fn with_base(token: impl AsRef<str>, base: impl Into<String>) -> Self {
        Self {
            client: authed_client(token.as_ref()),
            journal: None,
            serving_client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(REQUEST_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|e| e.to_string()),
            base: base.into(),
        }
    }

    pub(crate) fn with_journal(mut self, journal: crate::lifecycle::Journal) -> Self {
        self.journal = Some(journal);
        self
    }

    async fn send_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, WordPressError> {
        let url = format!("{}{path}", self.base);
        let mut req = self
            .client
            .as_ref()
            .map_err(|e| api_failed(method.as_str(), path, e))?
            .request(method.clone(), &url);
        if let Some(body) = &body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| api_failed(method.as_str(), path, e))?;
        if !status.is_success() {
            return Err(api_failed(
                method.as_str(),
                path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|err| api_failed(method.as_str(), path, format!("bad json: {err}")))
    }

    /// Create or update a published page with `html` as its body.
    pub async fn deploy_page(
        &self,
        site: &str,
        _service: &str,
        title: &str,
        html: &str,
    ) -> Result<DeployResult, WordPressError> {
        if let Some(journal) = &self.journal {
            return self.deploy_recorded_page(journal, site, title, html).await;
        }
        let path = format!("/sites/{site}/posts/new");
        let created = self
            .send_json(
                Method::POST,
                &path,
                Some(json!({
                    "title": title,
                    "content": html,
                    "status": "publish",
                    "type": "page",
                })),
            )
            .await?;
        let page_id = created
            .get("ID")
            .or_else(|| created.get("id"))
            .and_then(Value::as_i64)
            .or_else(|| {
                created
                    .get("ID")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok())
            })
            .map(|id| id.to_string())
            .ok_or_else(|| api_failed("POST", &path, "posts/new returned no page id"))?;
        let status = created
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("publish")
            .to_owned();
        let page_url = created
            .get("URL")
            .or_else(|| created.get("link"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let homepage_set = self.try_set_homepage(site, &page_id).await;
        Ok(DeployResult {
            page_id,
            page_url,
            status,
            homepage_set,
        })
    }

    async fn optional_get(&self, path: &str) -> Result<Option<Value>, WordPressError> {
        let response = self
            .client
            .as_ref()
            .map_err(|e| api_failed("GET", path, e))?
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", path, e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| api_failed("GET", path, e))?;
        if !status.is_success() {
            return Err(api_failed(
                "GET",
                path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| api_failed("GET", path, e))
    }

    pub(crate) async fn owned_site(
        &self,
        site: u64,
        origin: &str,
    ) -> Result<Option<Value>, WordPressError> {
        if site == 0 {
            return Err(crate::lifecycle::invalid("site ID is zero"));
        }
        let value = self.optional_get(&format!("/sites/{site}")).await?;
        if let Some(value) = &value {
            let url = value
                .get("URL")
                .and_then(Value::as_str)
                .ok_or_else(|| crate::lifecycle::invalid("site URL missing"))?;
            if numeric_id(value, "ID")? != site
                || normalize_origin(url)? != normalize_origin(origin)?
            {
                return Err(crate::lifecycle::invalid(
                    "native site differs from catalog outputs",
                ));
            }
        }
        Ok(value)
    }

    fn page_matches(
        value: &Value,
        site: u64,
        page: Option<u64>,
        receipt: &str,
        expected: &str,
    ) -> Result<bool, WordPressError> {
        let id = numeric_id(value, "ID")?;
        if numeric_id(value, "site_ID")? != site
            || page.is_some_and(|page| page != id)
            || value.get("type").and_then(Value::as_str) != Some("page")
            || value.get("slug").and_then(Value::as_str) != Some(receipt)
        {
            return Err(crate::lifecycle::invalid(
                "page identity differs from the submitted receipt",
            ));
        }
        let metadata = value
            .get("metadata")
            .and_then(Value::as_array)
            .ok_or_else(|| crate::lifecycle::invalid("page metadata missing"))?;
        let receipts: Vec<_> = metadata
            .iter()
            .filter(|m| {
                m.get("key").and_then(Value::as_str) == Some("stackless_deployment_receipt")
            })
            .collect();
        if receipts.len() != 1 || receipts[0].get("value").and_then(Value::as_str) != Some(receipt)
        {
            return Err(crate::lifecycle::invalid(
                "page has no unique deployment receipt",
            ));
        }
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| crate::lifecycle::invalid("page title missing"))?;
        let content = value
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| crate::lifecycle::invalid("page content missing"))?;
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| crate::lifecycle::invalid("page status missing"))?;
        if !matches!(
            status,
            "publish" | "draft" | "pending" | "private" | "future" | "trash" | "auto-draft"
        ) {
            return Err(crate::lifecycle::invalid("unknown page status"));
        }
        let protected = value
            .get("has_password")
            .and_then(Value::as_bool)
            .ok_or_else(|| crate::lifecycle::invalid("page password status missing"))?;
        Ok(status == "publish" && !protected && fingerprint(title, content)? == expected)
    }

    async fn homepage_is(&self, site: u64, page: u64) -> Result<bool, WordPressError> {
        let value = self
            .send_json(Method::GET, &format!("/sites/{site}/settings"), None)
            .await?;
        let settings = value
            .get("settings")
            .filter(|v| v.is_object())
            .ok_or_else(|| crate::lifecycle::invalid("site settings missing"))?;
        let front = settings
            .get("show_on_front")
            .and_then(Value::as_str)
            .ok_or_else(|| crate::lifecycle::invalid("show_on_front missing"))?;
        if !matches!(front, "posts" | "page") {
            return Err(crate::lifecycle::invalid("unknown show_on_front"));
        }
        let current = settings
            .get("page_on_front")
            .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
            .ok_or_else(|| crate::lifecycle::invalid("page_on_front missing"))?;
        Ok(front == "page" && current == page)
    }

    async fn deploy_recorded_page(
        &self,
        journal: &crate::lifecycle::Journal,
        site: &str,
        title: &str,
        html: &str,
    ) -> Result<DeployResult, WordPressError> {
        let receipt = journal.receipt()?;
        let html = format!("{html}\n<!-- {receipt} -->");
        let expected = fingerprint(title, &html)?;
        let (receipt, request) = journal.begin(site, expected.clone())?;
        let native = journal.load()?;
        let id = native
            .site_id
            .ok_or_else(|| crate::lifecycle::invalid("site ID missing"))?;
        let origin = native
            .origin
            .as_deref()
            .ok_or_else(|| crate::lifecycle::invalid("site origin missing"))?;
        let site_info = self
            .owned_site(id, origin)
            .await?
            .ok_or_else(|| crate::lifecycle::invalid("owned site is absent"))?;
        for field in ["is_private", "is_coming_soon"] {
            if site_info.get(field).and_then(Value::as_bool) != Some(false) {
                return Err(crate::lifecycle::invalid(format!(
                    "site {field} is not false"
                )));
            }
        }
        if site_info.get("user_can_manage").and_then(Value::as_bool) != Some(true) {
            return Err(crate::lifecycle::invalid(
                "token cannot manage the owned site",
            ));
        }
        let read_path = format!("/sites/{id}/posts/slug:{receipt}?context=edit");
        let value = if let Some(page) = request.page_id {
            self.optional_get(&format!("/sites/{id}/posts/{page}?context=edit"))
                .await?
                .ok_or_else(|| crate::lifecycle::invalid("recorded page is absent"))?
        } else if request.submitted {
            self.optional_get(&read_path).await?.ok_or_else(|| {
                crate::lifecycle::invalid(
                    "page submission outcome is unknown; refusing another POST",
                )
            })?
        } else {
            if self.optional_get(&read_path).await?.is_some() {
                return Err(crate::lifecycle::invalid(
                    "receipt slug was occupied before submission",
                ));
            }
            journal.submitted(&receipt)?;
            let value = self.send_json(Method::POST, &format!("/sites/{id}/posts/new?context=edit"), Some(json!({
                "title":title, "content":html, "slug":receipt, "status":"publish", "type":"page", "password":"",
                "publicize":false, "discussion":{"comments_open":false,"pings_open":false},
                "metadata":[{"key":"stackless_deployment_receipt","value":receipt}]
            }))).await?;
            journal.page(&receipt, numeric_id(&value, "ID")?)?;
            value
        };
        if !Self::page_matches(&value, id, request.page_id, &receipt, &expected)? {
            let page = request.page_id.ok_or_else(|| {
                crate::lifecycle::invalid("page is not publishing the recorded content")
            })?;
            journal.content_update(&receipt)?;
            self.send_json(Method::POST, &format!("/sites/{id}/posts/{page}?context=edit"), Some(json!({
                "title":title, "content":html, "status":"publish", "password":"", "publicize":false,
                "discussion":{"comments_open":false,"pings_open":false}
            }))).await?;
            let repaired = self
                .optional_get(&format!("/sites/{id}/posts/{page}?context=edit"))
                .await?
                .ok_or_else(|| crate::lifecycle::invalid("updated page is absent"))?;
            if !Self::page_matches(&repaired, id, Some(page), &receipt, &expected)? {
                return Err(crate::lifecycle::invalid(
                    "page content update is unconfirmed",
                ));
            }
        }
        let page = numeric_id(&value, "ID")?;
        journal.page(&receipt, page)?;
        if !self.homepage_is(id, page).await? {
            journal.homepage(&receipt)?;
            self.send_json(
                Method::POST,
                &format!("/sites/{id}/settings"),
                Some(json!({"show_on_front":"page","page_on_front":page})),
            )
            .await?;
            if !self.homepage_is(id, page).await? {
                return Err(crate::lifecycle::invalid(
                    "homepage update has not taken effect",
                ));
            }
        }
        let observed = self
            .optional_get(&format!("/sites/{id}/posts/{page}?context=edit"))
            .await?
            .ok_or_else(|| crate::lifecycle::invalid("published page is absent on readback"))?;
        if !Self::page_matches(&observed, id, Some(page), &receipt, &expected)? {
            return Err(crate::lifecycle::invalid(
                "published page differs on readback",
            ));
        }
        tokio::time::timeout(HEALTH_BUDGET, async {
            loop {
                if self.serving_receipt(origin, &receipt).await? {
                    return Ok::<(), WordPressError>(());
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await
        .map_err(|_| {
            crate::lifecycle::invalid("site is not serving the recorded page within 300 seconds")
        })??;
        let url = observed
            .get("URL")
            .and_then(Value::as_str)
            .ok_or_else(|| crate::lifecycle::invalid("page URL missing"))?;
        let parsed =
            reqwest::Url::parse(url).map_err(|e| crate::lifecycle::invalid(e.to_string()))?;
        if parsed.origin().ascii_serialization() != origin
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(crate::lifecycle::invalid(
                "page URL differs from the owned site",
            ));
        }
        Ok(DeployResult {
            page_id: page.to_string(),
            page_url: Some(url.into()),
            status: "publish".into(),
            homepage_set: true,
        })
    }

    async fn serving_receipt(&self, origin: &str, receipt: &str) -> Result<bool, WordPressError> {
        let origin = normalize_origin(origin)?;
        let mut response = self
            .serving_client
            .as_ref()
            .map_err(|e| api_failed("GET", "site origin", e))?
            .get(&origin)
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .await
            .map_err(|e| api_failed("GET", "site origin", e))?;
        if !response.status().is_success() {
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(false);
            }
            return Err(api_failed(
                "GET",
                "site origin",
                format!("status {}", response.status()),
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| api_failed("GET", "site origin", e))?
        {
            if body.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(api_failed("GET", "site origin", "homepage exceeds 2 MiB"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(String::from_utf8_lossy(&body).contains(&format!("<!-- {receipt} -->")))
    }

    pub(crate) async fn deployment_ready(
        &self,
        native: &crate::lifecycle::NativeState,
        receipt: &str,
        request: &crate::lifecycle::Request,
    ) -> Result<bool, WordPressError> {
        let site = native
            .site_id
            .ok_or_else(|| crate::lifecycle::invalid("site ID missing"))?;
        let page = request
            .page_id
            .ok_or_else(|| crate::lifecycle::invalid("page ID missing"))?;
        let Some(value) = self
            .optional_get(&format!("/sites/{site}/posts/{page}?context=edit"))
            .await?
        else {
            return Ok(false);
        };
        Ok(
            Self::page_matches(&value, site, Some(page), receipt, &request.fingerprint)?
                && self.homepage_is(site, page).await?
                && self
                    .serving_receipt(
                        native
                            .origin
                            .as_deref()
                            .ok_or_else(|| crate::lifecycle::invalid("site origin missing"))?,
                        receipt,
                    )
                    .await?,
        )
    }

    pub(crate) async fn delete_site(&self, site: u64) -> Result<(), WordPressError> {
        if site == 0 {
            return Err(crate::lifecycle::invalid("site ID is zero"));
        }
        self.send_json(Method::POST, &format!("/sites/{site}/delete"), None)
            .await?;
        Ok(())
    }

    async fn try_set_homepage(&self, site: &str, page_id: &str) -> bool {
        let path = format!("/sites/{site}/settings");
        let page_num: i64 = page_id.parse().unwrap_or(0);
        let body = json!({
            "show_on_front": "page",
            "page_on_front": page_num,
        });
        self.send_json(Method::POST, &path, Some(body))
            .await
            .is_ok()
    }

    /// Recent activity lines, or a deploy summary when the activity endpoint is
    /// empty/unavailable.
    pub async fn recent_logs(
        &self,
        site: &str,
        deploy: &DeployResult,
        tail: usize,
    ) -> Result<Vec<String>, WordPressError> {
        let mut lines: Vec<String> = self.fetch_activity(site).await.unwrap_or_default();
        if lines.is_empty() {
            lines.push(format!("deploy page_id={}", deploy.page_id));
            lines.push(format!("deploy status={}", deploy.status));
            if let Some(url) = &deploy.page_url {
                lines.push(format!("deploy page_url={url}"));
            }
            lines.push(format!("deploy homepage_set={}", deploy.homepage_set));
        }
        let keep = tail.max(1);
        if lines.len() > keep {
            lines = lines.split_off(lines.len() - keep);
        }
        Ok(lines)
    }

    async fn fetch_activity(&self, site: &str) -> Result<Vec<String>, WordPressError> {
        let path = format!("/sites/{site}/activity?num=50");
        let value = self.send_json(Method::GET, &path, None).await?;
        let items = value
            .get("current")
            .or_else(|| value.get("activities"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut lines = Vec::new();
        for item in items {
            if let Some(summary) = format_activity_entry(&item) {
                lines.push(summary);
            }
        }
        Ok(lines)
    }
}

fn format_activity_entry(item: &Value) -> Option<String> {
    if let Some(summary) = item.get("summary").and_then(Value::as_str) {
        return Some(summary.to_owned());
    }
    if let Some(content) = item.get("content").and_then(Value::as_str) {
        return Some(content.to_owned());
    }
    if let Some(title) = item.get("title").and_then(Value::as_str) {
        return Some(title.to_owned());
    }
    item.get("type")
        .and_then(Value::as_str)
        .map(|kind| format!("activity: {kind}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn public_receipt_reads_are_bounded_and_never_send_the_oauth_token() {
        for mode in 0..3 {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(move |request: &wiremock::Request| {
                    assert!(!request.headers.contains_key("authorization"));
                    ResponseTemplate::new(200).set_body_string(match mode {
                        0 => "<!-- wrong-receipt -->".into(),
                        1 => "<!-- stackless-receipt -->".into(),
                        _ => "x".repeat(2 * 1024 * 1024 + 1),
                    })
                })
                .expect(1)
                .mount(&server)
                .await;
            let api = WordPressApi::with_base("private-token", server.uri());
            let result = api
                .serving_receipt(&server.uri(), "stackless-receipt")
                .await;
            if mode == 2 {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), mode == 1);
            }
        }
    }
    #[tokio::test]
    async fn invalid_tokens_and_site_origins_fail_before_sending_requests() {
        for token in ["", "bad\ntoken"] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
            assert!(
                WordPressApi::with_base(token, server.uri())
                    .deploy_page("99", "web", "title", "html")
                    .await
                    .is_err()
            );
        }
        for origin in [
            "",
            "https://user:password@example.com",
            "https://example.com/path",
            "https://example.com?query",
            "https://example.com#fragment",
            "file:///tmp/site",
            "/sites/99",
        ] {
            assert!(normalize_origin(origin).is_err(), "{origin}");
        }
        let text = "é".repeat(201);
        assert!(truncate(&text).ends_with('…'));
    }

    #[test]
    fn site_identifier_parses_https_url() {
        assert_eq!(
            site_identifier_from_url("https://atto-demo-web.wordpress.com/").unwrap(),
            "atto-demo-web.wordpress.com"
        );
    }

    #[tokio::test]
    async fn deploy_page_posts_and_sets_homepage() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/sites/atto\.wordpress\.com/posts/new$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ID": 42,
                "status": "publish",
                "URL": "https://atto.wordpress.com/stackless-web/",
                "link": "https://atto.wordpress.com/stackless-web/",
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path_regex(r"/sites/atto\.wordpress\.com/settings$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "updated": true })))
            .mount(&server)
            .await;

        let api = WordPressApi::with_base("tok", server.uri());
        let result = api
            .deploy_page(
                "atto.wordpress.com",
                "web",
                "Stackless Web",
                "<p>stackless-smoke-ok</p>",
            )
            .await
            .unwrap();
        assert_eq!(result.page_id, "42");
        assert!(result.homepage_set);
    }

    #[tokio::test]
    async fn recent_logs_falls_back_to_deploy_summary() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/sites/atto\.wordpress\.com/activity"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let api = WordPressApi::with_base("tok", server.uri());
        let deploy = DeployResult {
            page_id: "7".into(),
            page_url: Some("https://atto.wordpress.com/".into()),
            status: "publish".into(),
            homepage_set: true,
        };
        let lines = api
            .recent_logs("atto.wordpress.com", &deploy, 10)
            .await
            .unwrap();
        assert!(lines.iter().any(|l| l.contains("page_id=7")));
    }

    #[tokio::test]
    async fn recent_logs_uses_activity_when_present() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"/sites/atto\.wordpress\.com/activity"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "current": [{ "summary": "Published page Stackless Web" }],
            })))
            .mount(&server)
            .await;

        let api = WordPressApi::with_base("tok", server.uri());
        let deploy = DeployResult {
            page_id: "7".into(),
            page_url: None,
            status: "publish".into(),
            homepage_set: false,
        };
        let lines = api
            .recent_logs("atto.wordpress.com", &deploy, 10)
            .await
            .unwrap();
        assert!(lines.iter().any(|l| l.contains("Published page")));
    }
}
