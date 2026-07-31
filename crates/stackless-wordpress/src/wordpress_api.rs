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
    client: Client,
    base: String,
}

impl std::fmt::Debug for WordPressApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WordPressApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

fn authed_client(token: &str) -> Client {
    let mut headers = HeaderMap::new();
    if let Ok(mut value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }
    Client::builder()
        .default_headers(headers)
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| Client::new())
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
        format!("{}…", &text[..MAX])
    }
}

/// Site identifier for `/sites/{site}/…` — host from an `https://…` URL or the
/// raw slug/domain Stripe returns.
pub fn site_identifier_from_url(site_url: &str) -> Result<String, WordPressError> {
    let trimmed = site_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(WordPressError::DeployFailed {
            service: "wordpress".into(),
            detail: "empty site URL from Stripe provision".into(),
        });
    }
    if let Some(rest) = trimmed.strip_prefix("https://") {
        let host = rest.split('/').next().unwrap_or(rest);
        if host.is_empty() {
            return Err(WordPressError::DeployFailed {
                service: "wordpress".into(),
                detail: format!("could not parse site host from {site_url:?}"),
            });
        }
        return Ok(host.to_owned());
    }
    if let Some(rest) = trimmed.strip_prefix("http://") {
        let host = rest.split('/').next().unwrap_or(rest);
        return Ok(host.to_owned());
    }
    Ok(trimmed.to_owned())
}

impl WordPressApi {
    pub fn new(token: impl AsRef<str>) -> Self {
        Self::with_base(token, DEFAULT_BASE)
    }

    pub fn with_base(token: impl AsRef<str>, base: impl Into<String>) -> Self {
        Self {
            client: authed_client(token.as_ref()),
            base: base.into(),
        }
    }

    async fn send_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, WordPressError> {
        let url = format!("{}{path}", self.base);
        let mut req = self.client.request(method.clone(), &url);
        if let Some(body) = &body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| api_failed(method.as_str(), path, err))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
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
