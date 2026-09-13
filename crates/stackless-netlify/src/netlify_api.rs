//! The Netlify REST client (ARCHITECTURE.md §4): the post-provisioning steps
//! Stripe Projects can't express — create/resolve the site, run the file-digest
//! deploy (POST the per-file SHA1 map, PUT only the files Netlify still needs),
//! apply build settings + zip/git builds, and poll the deploy to `ready`.
//!
//! Hand-written over `reqwest`: the deploy lifecycle is a small set of endpoints
//! with flat JSON + raw-bytes / multipart uploads, and the served spec is
//! Swagger 2.0 (`netlify/open-api`). Responses are parsed leniently so additive
//! provider drift never breaks a deploy.

use std::collections::HashSet;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::multipart::{Form, Part};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::error::NetlifyError;

/// Build settings applied to a site before a build-path deploy.
#[derive(Debug, Clone)]
pub struct BuildSettings {
    pub cmd: String,
    /// Publish directory (Netlify `dir`).
    pub dir: Option<String>,
    /// Base directory within the repo / zip (Netlify `base`).
    pub base: Option<String>,
}

const DEFAULT_BASE: &str = "https://api.netlify.com/api/v1";

/// A static deploy (upload + Netlify-side processing) is fast, but a cold
/// upload + edge propagation can lag; the budget covers it without hanging `up`.
pub const NETLIFY_DEPLOY_BUDGET: Duration = Duration::from_secs(10 * 60);
/// The public-origin health wait budget (§7).
pub const HEALTH_BUDGET: Duration = Duration::from_secs(5 * 60);

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// One file to upload: a repo-relative path (no leading slash) + its bytes.
#[derive(Debug, Clone)]
pub struct UploadFile {
    pub path: String,
    pub data: Vec<u8>,
}

/// A Netlify site's identity (from create/get).
#[derive(Debug, Clone)]
pub struct SiteInfo {
    pub id: String,
    /// The production HTTPS URL (`https://<site>.netlify.app`).
    pub ssl_url: Option<String>,
}

pub struct NetlifyApi {
    client: Client,
    base: String,
    poll_interval: Duration,
    journal: Option<crate::lifecycle::Journal>,
}

impl std::fmt::Debug for NetlifyApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetlifyApi")
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

fn api_failed(method: &str, path: &str, err: impl std::fmt::Display) -> NetlifyError {
    NetlifyError::ApiFailed {
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

fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl NetlifyApi {
    pub fn new(token: impl AsRef<str>) -> Self {
        Self::with_base(token, DEFAULT_BASE)
    }

    pub fn with_base(token: impl AsRef<str>, base: impl Into<String>) -> Self {
        Self {
            client: authed_client(token.as_ref()),
            base: base.into(),
            poll_interval: POLL_INTERVAL,
            journal: None,
        }
    }

    fn endpoint(&self, path: &str, params: &[(&str, &str)]) -> Result<reqwest::Url, NetlifyError> {
        let mut url = reqwest::Url::parse(&format!("{}{path}", self.base))
            .map_err(|e| api_failed("GET", path, e))?;
        url.query_pairs_mut().extend_pairs(params.iter().copied());
        Ok(url)
    }

    pub(crate) fn with_journal(mut self, journal: crate::lifecycle::Journal) -> Self {
        self.journal = Some(journal);
        self
    }

    pub async fn owned_site(&self, id: &str, name: &str) -> Result<Option<SiteInfo>, NetlifyError> {
        valid_id(id)?;
        let path = format!("/sites/{id}");
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(api_failed("GET", &path, response.status()));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if value["id"].as_str() != Some(id) || value["name"].as_str() != Some(name) {
            return Err(api_failed(
                "GET",
                &path,
                "site identity differs from its ownership record",
            ));
        }
        site_info(&value)
            .map(Some)
            .ok_or_else(|| api_failed("GET", &path, "missing site ID"))
    }

    pub(crate) async fn site_by_name(&self, name: &str) -> Result<Option<SiteInfo>, NetlifyError> {
        let mut found = None;
        let mut ids = HashSet::new();
        for page in 1..=1000 {
            let response = self
                .client
                .get(self.endpoint(
                    "/sites",
                    &[
                        ("name", name),
                        ("page", &page.to_string()),
                        ("per_page", "100"),
                    ],
                )?)
                .send()
                .await
                .map_err(|e| api_failed("GET", "/sites", e))?;
            if !response.status().is_success() {
                return Err(api_failed("GET", "/sites", response.status()));
            }
            let rows: Vec<Value> = response
                .json()
                .await
                .map_err(|e| api_failed("GET", "/sites", e))?;
            let full = rows.len() == 100;
            for row in rows {
                let site = site_info(&row)
                    .ok_or_else(|| api_failed("GET", "/sites", "missing site ID"))?;
                if !ids.insert(site.id.clone()) {
                    return Err(api_failed("GET", "/sites", "repeated site ID"));
                }
                let row_name = row["name"]
                    .as_str()
                    .ok_or_else(|| api_failed("GET", "/sites", "missing site name"))?;
                if row_name == name {
                    if found.is_some() {
                        return Err(api_failed("GET", "/sites", "ambiguous site name"));
                    }
                    found = Some(site);
                }
            }
            if !full {
                return Ok(found);
            }
        }
        Err(api_failed("GET", "/sites", "pagination limit exceeded"))
    }

    pub(crate) async fn owned_deploy(
        &self,
        site: &str,
        id: &str,
        receipt: &str,
    ) -> Result<Option<Value>, NetlifyError> {
        valid_id(site)?;
        valid_id(id)?;
        let path = format!("/sites/{site}/deploys/{id}");
        let response = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(api_failed("GET", &path, response.status()));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if value["id"].as_str() != Some(id)
            || value["site_id"].as_str() != Some(site)
            || value["title"].as_str() != Some(receipt)
            || value["state"].as_str().is_none()
        {
            return Err(api_failed(
                "GET",
                &path,
                "deployment identity or receipt does not match",
            ));
        }
        Ok(Some(value))
    }

    async fn find_receipt(&self, site: &str, receipt: &str) -> Result<Option<Value>, NetlifyError> {
        valid_id(site)?;
        let mut found = None;
        let mut ids = HashSet::new();
        for page in 1..=1000 {
            let path = format!("/sites/{site}/deploys?page={page}&per_page=100");
            let rows = self.send_json(Method::GET, &path, None).await?;
            let rows = rows
                .as_array()
                .ok_or_else(|| api_failed("GET", &path, "invalid deployment inventory"))?;
            for row in rows {
                let id = row["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| api_failed("GET", &path, "missing deployment ID"))?;
                if row["site_id"].as_str() != Some(site) || !ids.insert(id.to_owned()) {
                    return Err(api_failed("GET", &path, "foreign or repeated deployment"));
                }
                if row["title"].as_str() == Some(receipt) {
                    if found.is_some() {
                        return Err(api_failed("GET", &path, "ambiguous deployment receipt"));
                    }
                    found = Some(id.to_owned());
                }
            }
            if rows.len() < 100 {
                return match found {
                    Some(id) => self.owned_deploy(site, &id, receipt).await,
                    None => Ok(None),
                };
            }
        }
        Err(api_failed("GET", "/deploys", "pagination limit exceeded"))
    }

    async fn prepare_request(
        &self,
        site: &str,
        kind: &str,
        fingerprint: String,
    ) -> Result<(Option<String>, Option<Value>), NetlifyError> {
        let Some(journal) = &self.journal else {
            return Ok((None, None));
        };
        let (receipt, request) = journal.begin(kind, fingerprint)?;
        let recovered = if let Some(id) = request.deploy_id {
            Some(
                self.owned_deploy(site, &id, &receipt)
                    .await?
                    .ok_or_else(|| crate::lifecycle::invalid("recorded deployment disappeared"))?,
            )
        } else if request.submitted {
            let mut known_deploy = None;
            if let Some(id) = request.build_id {
                valid_id(&id)?;
                let build = self
                    .send_json(Method::GET, &format!("/builds/{id}"), None)
                    .await?;
                if build["id"].as_str() != Some(&id) {
                    return Err(crate::lifecycle::invalid("build ID changed"));
                }
                if let Some(deploy) = deploy_id_from_build(&build) {
                    journal.response(&receipt, Some(&deploy), Some(&id))?;
                    known_deploy = Some(deploy);
                }
            }
            let recovered = match known_deploy {
                Some(id) => self.owned_deploy(site, &id, &receipt).await?,
                None => self.find_receipt(site, &receipt).await?,
            };
            Some(recovered.ok_or_else(|| {
                crate::lifecycle::invalid(
                    "deployment submission is unresolved; refusing another POST",
                )
            })?)
        } else {
            None
        };
        if let Some(value) = &recovered {
            journal.response(&receipt, value["id"].as_str(), value["build_id"].as_str())?;
        }
        Ok((Some(receipt), recovered))
    }

    fn submit(&self, receipt: Option<&str>) -> Result<(), NetlifyError> {
        if let (Some(journal), Some(receipt)) = (&self.journal, receipt) {
            journal.submit(receipt)?;
        }
        Ok(())
    }
    fn record_response(
        &self,
        receipt: Option<&str>,
        value: &Value,
        build: bool,
    ) -> Result<(), NetlifyError> {
        if let (Some(journal), Some(receipt)) = (&self.journal, receipt) {
            let deploy = if build {
                deploy_id_from_build(value)
            } else {
                value["id"].as_str().map(str::to_owned)
            };
            journal.response(
                receipt,
                deploy.as_deref(),
                if build { value["id"].as_str() } else { None },
            )?;
        }
        Ok(())
    }

    /// Tests set a tiny interval so the poll/timeout paths run instantly.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Send a JSON request and require a 2xx, returning the parsed body (or
    /// `Null` for an empty body).
    async fn send_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, NetlifyError> {
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

    /// Create a Netlify site with the given name (used when provisioning did not
    /// already hand back a site id).
    pub async fn create_site(&self, name: &str) -> Result<SiteInfo, NetlifyError> {
        if let Some(journal) = &self.journal {
            let mut state = journal.load()?;
            if state.site_conflict {
                return Err(crate::lifecycle::invalid(
                    "native site ownership conflict requires an audit",
                ));
            }
            if let Some(id) = &state.site_id {
                return self
                    .owned_site(id, name)
                    .await?
                    .ok_or_else(|| crate::lifecycle::invalid("recorded native site disappeared"));
            }
            let existing = self.site_by_name(name).await?;
            if state.site_submitted {
                let site = existing.ok_or_else(|| {
                    crate::lifecycle::invalid("site creation is unresolved; refusing another POST")
                })?;
                journal.site(&site.id)?;
                return Ok(site);
            }
            if existing.is_some() {
                state.site_conflict = true;
                journal.save(&state)?;
                return Err(crate::lifecycle::invalid(
                    "native site name existed before submission",
                ));
            }
            state.site_submitted = true;
            journal.save(&state)?;
        }
        let value = self
            .send_json(Method::POST, "/sites", Some(json!({"name":name})))
            .await?;
        let site = site_info(&value)
            .ok_or_else(|| api_failed("POST", "/sites", "create returned no site ID"))?;
        if let Some(journal) = &self.journal {
            journal.site(&site.id)?;
        }
        if value["name"].as_str().is_some_and(|actual| actual != name) {
            return Err(api_failed(
                "POST",
                "/sites",
                "created site has another name",
            ));
        }
        Ok(site)
    }

    /// Whether a site still exists (best-effort teardown verification).
    pub async fn site_exists(&self, site_id: &str) -> Result<bool, NetlifyError> {
        let path = format!("/sites/{site_id}");
        let url = format!("{}{path}", self.base);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|err| api_failed("GET", &path, err))?;
        match resp.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            other => Err(api_failed("GET", &path, format!("status {other}"))),
        }
    }

    /// Best-effort site delete (Stripe is the authoritative deprovision).
    pub async fn delete_site(&self, site_id: &str) -> Result<(), NetlifyError> {
        self.send_json(Method::DELETE, &format!("/sites/{site_id}"), None)
            .await
            .map(|_| ())
    }

    /// Apply build settings (cmd / publish dir / base) before a build deploy.
    pub async fn update_build_settings(
        &self,
        site_id: &str,
        settings: &BuildSettings,
    ) -> Result<(), NetlifyError> {
        let path = format!("/sites/{site_id}");
        let mut build_settings = json!({ "cmd": settings.cmd });
        if let Some(dir) = &settings.dir {
            build_settings["dir"] = json!(dir);
        }
        if let Some(base) = &settings.base {
            build_settings["base"] = json!(base);
        }
        self.send_json(
            Method::PATCH,
            &path,
            Some(json!({ "build_settings": build_settings })),
        )
        .await?;
        Ok(())
    }

    /// Link a public GitHub repo for continuous / git-triggered builds.
    pub async fn link_github_repo(
        &self,
        site_id: &str,
        org: &str,
        repo: &str,
        branch: &str,
        settings: &BuildSettings,
    ) -> Result<(), NetlifyError> {
        let path = format!("/sites/{site_id}");
        let repo_path = format!("{org}/{repo}");
        let mut repo_body = json!({
            "provider": "github",
            "repo": repo_path,
            "repo_path": repo_path,
            "repo_url": format!("https://github.com/{org}/{repo}"),
            "repo_branch": branch,
            "cmd": settings.cmd,
            "public_repo": true,
        });
        if let Some(dir) = &settings.dir {
            repo_body["dir"] = json!(dir);
        }
        if let Some(base) = &settings.base {
            repo_body["base"] = json!(base);
        }
        self.send_json(Method::PATCH, &path, Some(json!({ "repo": repo_body })))
            .await?;
        Ok(())
    }

    /// Trigger a build from a zip of the source tree (build-path, no GitHub link).
    pub async fn deploy_build_zip(
        &self,
        site_id: &str,
        zip_bytes: Vec<u8>,
        title: &str,
        service: &str,
        budget: Duration,
    ) -> Result<(String, String), NetlifyError> {
        let fingerprint = stackless_core::engine::revision::digest(&(site_id, &zip_bytes))
            .map_err(|e| crate::lifecycle::invalid(e.message))?;
        let (receipt, recovered) = self
            .prepare_request(site_id, "build-zip", fingerprint)
            .await?;
        if let Some(deploy) = recovered {
            let id = deploy["id"]
                .as_str()
                .ok_or_else(|| crate::lifecycle::invalid("recovered deploy has no ID"))?
                .to_owned();
            let origin = self
                .wait_for_ready(
                    &id,
                    service,
                    budget,
                    receipt.as_deref().map(|r| (site_id, r)),
                )
                .await?;
            return Ok((origin, id));
        }
        let path = format!("/sites/{site_id}/builds");
        let part = Part::bytes(zip_bytes)
            .file_name("site.zip")
            .mime_str("application/zip")
            .map_err(|err| api_failed("POST", &path, err))?;
        let form = Form::new().part("zip", part);
        self.submit(receipt.as_deref())?;
        let resp = self
            .client
            .post(self.endpoint(&path, &[("title", receipt.as_deref().unwrap_or(title))])?)
            .multipart(form)
            .send()
            .await
            .map_err(|err| api_failed("POST", &path, err))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(api_failed(
                "POST",
                &path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        let created: Value = serde_json::from_str(&text)
            .map_err(|err| api_failed("POST", &path, format!("bad json: {err}")))?;
        self.record_response(receipt.as_deref(), &created, true)?;
        let deploy_id = deploy_id_from_build(&created)
            .ok_or_else(|| api_failed("POST", &path, "build create returned no deploy id"))?;
        let origin = self
            .wait_for_ready(
                &deploy_id,
                service,
                budget,
                receipt.as_deref().map(|r| (site_id, r)),
            )
            .await?;
        Ok((origin, deploy_id))
    }

    /// Trigger a git-linked site build (Netlify clones the linked repo).
    pub async fn deploy_build_git(
        &self,
        site_id: &str,
        branch: &str,
        service: &str,
        budget: Duration,
    ) -> Result<(String, String), NetlifyError> {
        let fingerprint = stackless_core::engine::revision::digest(&(site_id, branch))
            .map_err(|e| crate::lifecycle::invalid(e.message))?;
        let (receipt, recovered) = self
            .prepare_request(site_id, "build-git", fingerprint)
            .await?;
        let deploy_id = if let Some(deploy) = recovered {
            deploy["id"]
                .as_str()
                .ok_or_else(|| crate::lifecycle::invalid("recovered deploy has no ID"))?
                .to_owned()
        } else {
            let path = format!("/sites/{site_id}/builds");
            self.submit(receipt.as_deref())?;
            let response = self
                .client
                .post(self.endpoint(
                    &path,
                    &[
                        ("branch", branch),
                        ("clear_cache", "true"),
                        ("title", receipt.as_deref().unwrap_or("stackless")),
                    ],
                )?)
                .send()
                .await
                .map_err(|e| api_failed("POST", &path, e))?;
            if !response.status().is_success() {
                return Err(api_failed("POST", &path, response.status()));
            }
            let created: Value = response
                .json()
                .await
                .map_err(|e| api_failed("POST", &path, e))?;
            self.record_response(receipt.as_deref(), &created, true)?;
            deploy_id_from_build(&created)
                .ok_or_else(|| api_failed("POST", &path, "build create returned no deploy ID"))?
        };
        let origin = self
            .wait_for_ready(
                &deploy_id,
                service,
                budget,
                receipt.as_deref().map(|r| (site_id, r)),
            )
            .await?;
        Ok((origin, deploy_id))
    }

    /// Run the full file-digest deploy and return the live HTTPS URL: POST the
    /// per-file SHA1 map, PUT each file Netlify reports as `required`, then poll
    /// the deploy to `ready`.
    pub async fn deploy(
        &self,
        site_id: &str,
        files: &[UploadFile],
        service: &str,
        budget: Duration,
    ) -> Result<(String, String), NetlifyError> {
        // Per-file SHA1, keyed by leading-slash path (the Netlify file map shape).
        let mut digests = serde_json::Map::new();
        for file in files {
            digests.insert(
                format!("/{}", file.path),
                Value::String(sha1_hex(&file.data)),
            );
        }
        let fingerprint = stackless_core::engine::revision::digest(&(
            site_id,
            files.iter().map(|f| (&f.path, &f.data)).collect::<Vec<_>>(),
        ))
        .map_err(|e| crate::lifecycle::invalid(e.message))?;
        let (receipt, recovered) = self
            .prepare_request(site_id, "deploy-files", fingerprint)
            .await?;
        let deploys_path = format!("/sites/{site_id}/deploys");
        let created = match recovered {
            Some(value) => value,
            None => {
                self.submit(receipt.as_deref())?;
                let path = receipt
                    .as_ref()
                    .map(|r| format!("{deploys_path}?title={r}"))
                    .unwrap_or_else(|| deploys_path.clone());
                let value = self
                    .send_json(
                        Method::POST,
                        &path,
                        Some(json!({"files": Value::Object(digests)})),
                    )
                    .await?;
                self.record_response(receipt.as_deref(), &value, false)?;
                value
            }
        };
        let deploy_id = created["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| api_failed("POST", &deploys_path, "deploy create returned no ID"))?
            .to_owned();
        if let Some(receipt) = &receipt {
            self.owned_deploy(site_id, &deploy_id, receipt)
                .await?
                .ok_or_else(|| crate::lifecycle::invalid("deployment disappeared before upload"))?;
        }
        let required: HashSet<String> = match created.get("required") {
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| crate::lifecycle::invalid("invalid required digest"))
                })
                .collect::<Result<_, _>>()?,
            None if matches!(created["state"].as_str(), Some("ready" | "processing")) => {
                HashSet::new()
            }
            _ => {
                return Err(crate::lifecycle::invalid(
                    "deployment returned no required digest list",
                ));
            }
        };
        let available: HashSet<_> = files.iter().map(|f| sha1_hex(&f.data)).collect();
        if !required.is_subset(&available) {
            return Err(crate::lifecycle::invalid(
                "deployment requires content outside its recorded upload",
            ));
        }

        // Upload each required digest once (Netlify dedups by SHA1).
        let mut uploaded: HashSet<String> = HashSet::new();
        for file in files {
            let sha = sha1_hex(&file.data);
            if required.contains(&sha) && uploaded.insert(sha) {
                self.upload_file(&deploy_id, &file.path, &file.data).await?;
            }
        }

        let origin = self
            .wait_for_ready(
                &deploy_id,
                service,
                budget,
                receipt.as_deref().map(|r| (site_id, r)),
            )
            .await?;
        Ok((origin, deploy_id))
    }

    async fn upload_file(
        &self,
        deploy_id: &str,
        rel_path: &str,
        bytes: &[u8],
    ) -> Result<(), NetlifyError> {
        valid_id(deploy_id)?;
        if rel_path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(api_failed("PUT", "/deploys/files", "invalid upload path"));
        }
        let path = format!("/deploys/{deploy_id}/files/{rel_path}");
        let mut url = reqwest::Url::parse(&self.base).map_err(|e| api_failed("PUT", &path, e))?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| api_failed("PUT", &path, "invalid API base"))?;
            segments
                .pop_if_empty()
                .push("deploys")
                .push(deploy_id)
                .push("files");
            for component in rel_path.split('/') {
                segments.push(component);
            }
        }
        let resp = self
            .client
            .put(url)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|err| api_failed("PUT", &path, err))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(api_failed(
                "PUT",
                &path,
                format!("status {}: {}", status.as_u16(), truncate(&text)),
            ));
        }
        Ok(())
    }

    /// Poll the deploy until `ready`, returning its live HTTPS URL.
    async fn wait_for_ready(
        &self,
        deploy_id: &str,
        service: &str,
        budget: Duration,
        authority: Option<(&str, &str)>,
    ) -> Result<String, NetlifyError> {
        let path = format!("/deploys/{deploy_id}");
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let deploy = match authority {
                Some((site, receipt)) => self
                    .owned_deploy(site, deploy_id, receipt)
                    .await?
                    .ok_or_else(|| {
                        crate::lifecycle::invalid("deployment disappeared while waiting")
                    })?,
                None => self.send_json(Method::GET, &path, None).await?,
            };
            let state = DeployState::from_api(
                deploy
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
            );
            if state.is_ready() {
                let origin = deploy
                    .get("ssl_url")
                    .or_else(|| deploy.get("deploy_ssl_url"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        api_failed("GET", &path, "ready deployment returned no endpoint")
                    })?;
                return Ok(origin.to_owned());
            }
            if state.is_failed() {
                return Err(NetlifyError::DeployFailed {
                    service: service.to_owned(),
                    state: state.as_str().to_owned(),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(NetlifyError::DeployTimeout {
                    service: service.to_owned(),
                    budget_secs: budget.as_secs(),
                    last_state: state.as_str().to_owned(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

fn valid_id(id: &str) -> Result<(), NetlifyError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err(api_failed("GET", "/sites", "invalid native ID"));
    }
    Ok(())
}

fn site_info(value: &Value) -> Option<SiteInfo> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?
        .to_owned();
    Some(SiteInfo {
        id,
        ssl_url: value
            .get("ssl_url")
            .or_else(|| value.get("url"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn deploy_id_from_build(value: &Value) -> Option<String> {
    // Prefer deploy_id — top-level `id` on a build record is the build id.
    value
        .get("deploy_id")
        .or_else(|| value.pointer("/deploy/id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A Netlify deploy state. Modeled as an enum so the polling logic is
/// exhaustive; `Unknown` preserves any state not in Netlify's documented set so
/// drift is visible instead of silently misclassified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployState {
    New,
    PendingReview,
    Accepted,
    Rejected,
    Enqueued,
    Building,
    Uploading,
    Uploaded,
    Preparing,
    Prepared,
    Processing,
    Processed,
    Ready,
    Error,
    Retrying,
    Unknown(String),
}

impl DeployState {
    pub const CANONICAL: &'static [&'static str] = &[
        "new",
        "pending_review",
        "accepted",
        "rejected",
        "enqueued",
        "building",
        "uploading",
        "uploaded",
        "preparing",
        "prepared",
        "processing",
        "processed",
        "ready",
        "error",
        "retrying",
    ];

    pub fn from_api(state: &str) -> Self {
        match state {
            "new" => Self::New,
            "pending_review" => Self::PendingReview,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            "enqueued" => Self::Enqueued,
            "building" => Self::Building,
            "uploading" => Self::Uploading,
            "uploaded" => Self::Uploaded,
            "preparing" => Self::Preparing,
            "prepared" => Self::Prepared,
            "processing" => Self::Processing,
            "processed" => Self::Processed,
            "ready" => Self::Ready,
            "error" => Self::Error,
            "retrying" => Self::Retrying,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::New => "new",
            Self::PendingReview => "pending_review",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Enqueued => "enqueued",
            Self::Building => "building",
            Self::Uploading => "uploading",
            Self::Uploaded => "uploaded",
            Self::Preparing => "preparing",
            Self::Prepared => "prepared",
            Self::Processing => "processing",
            Self::Processed => "processed",
            Self::Ready => "ready",
            Self::Error => "error",
            Self::Retrying => "retrying",
            Self::Unknown(raw) => raw,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// A terminal failure. A new Netlify state containing `error`/`reject` still
    /// fails fast; a new in-progress state never false-fails.
    pub fn is_failed(&self) -> bool {
        match self {
            Self::Error | Self::Rejected => true,
            Self::Unknown(raw) => raw.contains("error") || raw.contains("reject"),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn canonical_states_are_modeled() {
        for state in DeployState::CANONICAL {
            let parsed = DeployState::from_api(state);
            assert!(
                !matches!(parsed, DeployState::Unknown(_)),
                "canonical Netlify state {state:?} fell through to Unknown",
            );
            assert_eq!(parsed.as_str(), *state);
        }
        assert!(DeployState::from_api("ready").is_ready());
        assert!(DeployState::from_api("error").is_failed());
        assert!(DeployState::from_api("rejected").is_failed());
        assert!(!DeployState::from_api("processing").is_failed());
        assert_eq!(DeployState::from_api("warp").as_str(), "warp");
    }

    #[test]
    fn sha1_matches_known_vector() {
        // SHA1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[tokio::test]
    async fn deploy_uploads_only_required_then_polls_ready() {
        let server = MockServer::start().await;
        let sha = sha1_hex(b"<html>ok</html>");
        // create deploy → reports our one file as required
        Mock::given(method("POST"))
            .and(path("/sites/site_1/deploys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "dep_1",
                "state": "uploading",
                "required": [sha],
            })))
            .mount(&server)
            .await;
        // upload the required file
        Mock::given(method("PUT"))
            .and(path("/deploys/dep_1/files/index.html"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "f1" })))
            .mount(&server)
            .await;
        // poll → ready
        Mock::given(method("GET"))
            .and(path("/deploys/dep_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "ready",
                "ssl_url": "https://site-1.netlify.app",
            })))
            .mount(&server)
            .await;

        let api =
            NetlifyApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        let files = vec![UploadFile {
            path: "index.html".into(),
            data: b"<html>ok</html>".to_vec(),
        }];
        let url = api
            .deploy("site_1", &files, "web", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(url.0, "https://site-1.netlify.app");
        assert_eq!(url.1, "dep_1");
    }

    #[tokio::test]
    async fn deploy_build_zip_posts_multipart_and_polls_deploy() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/sites/site_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "site_1" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/sites/site_1/builds"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "build_1",
                "deploy_id": "dep_build",
                "state": "building",
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/deploys/dep_build"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "ready",
                "ssl_url": "https://site-1.netlify.app",
            })))
            .mount(&server)
            .await;

        let api =
            NetlifyApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        api.update_build_settings(
            "site_1",
            &BuildSettings {
                cmd: "mkdir -p dist && cp static/index.html dist/index.html".into(),
                dir: Some("dist".into()),
                base: None,
            },
        )
        .await
        .unwrap();
        let (url, deploy_id) = api
            .deploy_build_zip(
                "site_1",
                b"PK\x03\x04fake".to_vec(),
                "stackless test",
                "web",
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(url, "https://site-1.netlify.app");
        assert_eq!(deploy_id, "dep_build");
    }

    #[tokio::test]
    async fn deploy_fails_fast_on_error_state() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sites/site_1/deploys"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": "dep_1", "state": "new", "required": [] })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/deploys/dep_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "state": "error" })))
            .mount(&server)
            .await;
        let api =
            NetlifyApi::with_base("tok", server.uri()).with_poll_interval(Duration::from_millis(1));
        let err = api
            .deploy("site_1", &[], "web", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(matches!(err, NetlifyError::DeployFailed { .. }));
    }
}
