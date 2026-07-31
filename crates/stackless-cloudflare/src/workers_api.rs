//! Cloudflare Workers REST client: upload a module worker, enable the
//! `*.workers.dev` route, and read script metadata for deploy summaries.

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method};
use serde_json::{Value, json};

use crate::error::CloudflareHostError;

const DEFAULT_BASE: &str = "https://api.cloudflare.com/client/v4";

/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const COMPATIBILITY_DATE: &str = "2024-11-01";

/// Metadata returned after a successful script upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptDeployInfo {
    pub id: String,
    pub etag: String,
    pub modified_on: String,
}

pub struct WorkersApi {
    client: Client,
    base: String,
}

impl std::fmt::Debug for WorkersApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkersApi")
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

fn api_failed(method: &str, path: &str, detail: impl std::fmt::Display) -> CloudflareHostError {
    CloudflareHostError::ApiFailed {
        method: method.to_owned(),
        path: path.to_owned(),
        detail: detail.to_string(),
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

fn require_success(method: &str, path: &str, body: &Value) -> Result<Value, CloudflareHostError> {
    if body.get("success").and_then(Value::as_bool) == Some(true) {
        return Ok(body.get("result").cloned().unwrap_or_else(|| body.clone()));
    }
    let errors = body
        .get("errors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| truncate(&body.to_string()));
    Err(api_failed(method, path, errors))
}

impl WorkersApi {
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
    ) -> Result<Value, CloudflareHostError> {
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
                format!("HTTP {status}: {}", truncate(&text)),
            ));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|err| api_failed(method.as_str(), path, format!("invalid JSON: {err}")))
    }

    /// Upload (or replace) a module worker script.
    pub async fn put_script(
        &self,
        account_id: &str,
        script_name: &str,
        main_module: &str,
        script: &[u8],
    ) -> Result<ScriptDeployInfo, CloudflareHostError> {
        let path = format!("/accounts/{account_id}/workers/scripts/{script_name}");
        let metadata = json!({
            "main_module": main_module,
            "compatibility_date": COMPATIBILITY_DATE,
            "observability": { "enabled": true },
        });
        let metadata_part = Part::text(metadata.to_string())
            .mime_str("application/json")
            .map_err(|err| api_failed("PUT", &path, err))?;
        let module_part = Part::bytes(script.to_vec())
            .file_name(main_module.to_owned())
            .mime_str("application/javascript+module")
            .map_err(|err| api_failed("PUT", &path, err))?;
        let form = Form::new()
            .part("metadata", metadata_part)
            .part(main_module.to_owned(), module_part);

        let url = format!("{}{path}", self.base);
        let resp = self
            .client
            .put(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|err| api_failed("PUT", &path, err))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(api_failed(
                "PUT",
                &path,
                format!("HTTP {status}: {}", truncate(&text)),
            ));
        }
        let body: Value = if text.trim().is_empty() {
            json!({ "success": true, "result": {} })
        } else {
            serde_json::from_str(&text)
                .map_err(|err| api_failed("PUT", &path, format!("invalid JSON: {err}")))?
        };
        let result = require_success("PUT", &path, &body)?;
        Ok(parse_script_info(&result, script_name))
    }

    /// Enable the script on the account's `workers.dev` subdomain.
    pub async fn enable_workers_dev(
        &self,
        account_id: &str,
        script_name: &str,
    ) -> Result<(), CloudflareHostError> {
        let path = format!("/accounts/{account_id}/workers/scripts/{script_name}/subdomain");
        let body = self
            .send_json(Method::POST, &path, Some(json!({ "enabled": true })))
            .await?;
        require_success("POST", &path, &body)?;
        Ok(())
    }

    /// Fetch script metadata (for log summaries).
    pub async fn get_script(
        &self,
        account_id: &str,
        script_name: &str,
    ) -> Result<ScriptDeployInfo, CloudflareHostError> {
        let path = format!("/accounts/{account_id}/workers/scripts/{script_name}");
        let body = self.send_json(Method::GET, &path, None).await?;
        let result = require_success("GET", &path, &body)?;
        Ok(parse_script_info(&result, script_name))
    }

    /// Human-readable deploy / script summary lines for `stackless logs`.
    pub fn deploy_summary_lines(
        script_name: &str,
        account_id: &str,
        origin: &str,
        info: &ScriptDeployInfo,
    ) -> Vec<String> {
        vec![
            format!("cloudflare worker script: {script_name}"),
            format!("account_id: {account_id}"),
            format!("origin: {origin}"),
            format!("script id: {}", info.id),
            format!("etag: {}", info.etag),
            format!("modified_on: {}", info.modified_on),
            "deploy: Workers script upload confirmed".into(),
        ]
    }
}

fn parse_script_info(result: &Value, fallback_id: &str) -> ScriptDeployInfo {
    let id = result
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(fallback_id)
        .to_owned();
    let etag = result
        .get("etag")
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_owned();
    let modified_on = result
        .get("modified_on")
        .or_else(|| result.get("created_on"))
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_owned();
    ScriptDeployInfo {
        id,
        etag,
        modified_on,
    }
}

/// Build a module worker that serves static HTML (smoke / static-site path).
pub fn module_worker_for_html(html: &str) -> (String, Vec<u8>) {
    let main_module = "stackless_worker.mjs".to_owned();
    let html_json = serde_json::to_string(html).unwrap_or_else(|_| "\"\"".to_owned());
    let source = format!(
        "const HTML = {html_json};\n\
         export default {{\n\
           async fetch() {{\n\
             return new Response(HTML, {{\n\
               headers: {{ \"content-type\": \"text/html;charset=utf-8\" }},\n\
             }});\n\
           }},\n\
         }};\n"
    );
    (main_module, source.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn module_worker_for_html_embeds_body() {
        let (_name, bytes) = module_worker_for_html("<p>stackless-smoke-ok</p>");
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("stackless-smoke-ok"));
        assert!(text.contains("export default"));
    }

    #[tokio::test]
    async fn put_script_uploads_multipart_and_parses_result() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/accounts/acc_1/workers/scripts/demo-web"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "result": {
                    "id": "demo-web",
                    "etag": "etag_1",
                    "modified_on": "2026-01-01T00:00:00Z",
                },
            })))
            .mount(&server)
            .await;

        let api = WorkersApi::with_base("tok", server.uri());
        let (module, script) = module_worker_for_html("ok");
        let info = api
            .put_script("acc_1", "demo-web", &module, &script)
            .await
            .unwrap();
        assert_eq!(info.id, "demo-web");
        assert_eq!(info.etag, "etag_1");
    }

    #[tokio::test]
    async fn enable_workers_dev_posts_subdomain() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts/acc_1/workers/scripts/demo-web/subdomain"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "success": true,
                "result": { "enabled": true },
            })))
            .mount(&server)
            .await;

        let api = WorkersApi::with_base("tok", server.uri());
        api.enable_workers_dev("acc_1", "demo-web").await.unwrap();
    }

    #[tokio::test]
    async fn deploy_summary_lines_are_non_empty() {
        let info = ScriptDeployInfo {
            id: "demo-web".into(),
            etag: "e1".into(),
            modified_on: "2026-01-01".into(),
        };
        let lines = WorkersApi::deploy_summary_lines(
            "demo-web",
            "acc_1",
            "https://demo-web.sub.workers.dev",
            &info,
        );
        assert!(lines.len() >= 5);
        assert!(lines.iter().any(|l| l.contains("etag")));
    }
}
