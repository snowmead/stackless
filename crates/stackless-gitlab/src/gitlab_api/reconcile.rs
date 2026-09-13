//! Replace the managed Pages tree while preserving unrelated repository files.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}
fn managed(path: &str) -> bool {
    path == "public" || path.starts_with("public/") || path == ".gitlab-ci.yml"
}
impl GitLabApi {
    pub(super) async fn branch_head(
        &self,
        project: &str,
        branch: &str,
    ) -> Result<Option<String>, GitLabError> {
        let path = format!(
            "/projects/{}/repository/branches/{}",
            encode_project_id(project),
            urlencoding::encode(branch)
        );
        let response = self
            .client()?
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            let value = self
                .send_json(
                    Method::GET,
                    &format!("/projects/{}", encode_project_id(project)),
                    None,
                )
                .await?;
            if value["id"].as_u64().map(|id| id.to_string()).as_deref() != Some(project)
                || value["empty_repo"].as_bool() != Some(true)
            {
                return Err(api_failed(
                    "GET",
                    &path,
                    "branch absence is not a verified empty repository",
                ));
            }
            return Ok(None);
        }
        let value: Value = response
            .error_for_status()
            .map_err(|e| api_failed("GET", &path, e))?
            .json()
            .await
            .map_err(|e| api_failed("GET", &path, e))?;
        if value["name"].as_str() != Some(branch) {
            return Err(api_failed("GET", &path, "branch name differs"));
        }
        commit_sha(&value["commit"], "GET", &path).map(Some)
    }
    async fn repository_tree(
        &self,
        project: &str,
        sha: &str,
    ) -> Result<BTreeMap<String, String>, GitLabError> {
        let mut rows = BTreeMap::new();
        for page in 1..=1000 {
            let path = format!(
                "/projects/{}/repository/tree?ref={}&recursive=true&per_page=100&page={page}",
                encode_project_id(project),
                urlencoding::encode(sha)
            );
            let value = self.send_json(Method::GET, &path, None).await?;
            let entries = value
                .as_array()
                .ok_or_else(|| api_failed("GET", &path, "tree response is not an array"))?;
            for node in entries {
                let name = node["path"]
                    .as_str()
                    .filter(|p| safe_path(p))
                    .ok_or_else(|| api_failed("GET", &path, "invalid tree path"))?;
                commit_sha(node, "GET", &path)?;
                let kind = node["type"]
                    .as_str()
                    .filter(|k| matches!(*k, "blob" | "tree" | "commit"))
                    .ok_or_else(|| api_failed("GET", &path, "invalid tree entry type"))?;
                if rows.insert(name.into(), kind.into()).is_some() {
                    return Err(api_failed("GET", &path, "duplicate tree path"));
                }
            }
            if entries.len() < 100 {
                return Ok(rows);
            }
        }
        Err(invalid(
            "repository tree exceeds 100000 entries; refusing a partial replacement",
        ))
    }
    async fn file_revision(
        &self,
        project: &str,
        path: &str,
        sha: &str,
    ) -> Result<String, GitLabError> {
        let route = format!(
            "/projects/{}/repository/files/{}?ref={}",
            encode_project_id(project),
            encode_file_path(path),
            urlencoding::encode(sha)
        );
        let value = self.send_json(Method::GET, &route, None).await?;
        if value["file_path"].as_str() != Some(path) || value["commit_id"].as_str() != Some(sha) {
            return Err(api_failed(
                "GET",
                &route,
                "file identity or revision differs",
            ));
        }
        commit_sha(&json!({"id":value["last_commit_id"]}), "GET", &route)
    }
    pub(super) async fn replacement_actions(
        &self,
        project: &str,
        branch: &str,
        files: &[RepoFile],
    ) -> Result<(Option<String>, Vec<Value>), GitLabError> {
        let mut desired = BTreeSet::new();
        for file in files {
            if !safe_path(&file.path) || !managed(&file.path) || !desired.insert(file.path.clone())
            {
                return Err(invalid("invalid or duplicate managed source path"));
            }
        }
        for path in &desired {
            for (index, _) in path.match_indices('/') {
                if desired.contains(&path[..index]) {
                    return Err(invalid(
                        "source file collides with another source directory",
                    ));
                }
            }
        }
        let base = self.branch_head(project, branch).await?;
        let tree = match base.as_deref() {
            Some(sha) => self.repository_tree(project, sha).await?,
            None => BTreeMap::new(),
        };
        let mut actions = Vec::new();
        for (path, kind) in &tree {
            if !managed(path) || kind == "tree" {
                continue;
            }
            if kind != "blob" {
                return Err(invalid("managed Pages tree contains a submodule"));
            }
            if !desired.contains(path) {
                let last = self
                    .file_revision(
                        project,
                        path,
                        base.as_deref()
                            .ok_or_else(|| invalid("tree has no base commit"))?,
                    )
                    .await?;
                actions.push(json!({"action":"delete","file_path":path,"last_commit_id":last}));
            }
        }
        for file in files {
            let existing = tree.get(&file.path);
            if existing.is_some_and(|kind| {
                kind == "commit" || (kind == "tree" && file.path == ".gitlab-ci.yml")
            }) {
                return Err(invalid(
                    "source file collides with a repository directory or submodule",
                ));
            }
            let mut action = json!({"action":if existing.is_some_and(|kind|kind=="blob"){"update"}else{"create"},"file_path":file.path,"content":base64::engine::general_purpose::STANDARD.encode(&file.content),"encoding":"base64"});
            if existing.is_some_and(|kind| kind == "blob") {
                action["last_commit_id"] = json!(
                    self.file_revision(
                        project,
                        &file.path,
                        base.as_deref()
                            .ok_or_else(|| invalid("tree has no base commit"))?
                    )
                    .await?
                );
            }
            actions.push(action);
        }
        Ok((base, actions))
    }
    pub(super) async fn verify_public_tree(
        &self,
        project: &str,
        sha: &str,
        files: &[RepoFile],
    ) -> Result<(), GitLabError> {
        let tree = self.repository_tree(project, sha).await?;
        let actual: BTreeSet<_> = tree
            .iter()
            .filter(|(path, kind)| managed(path) && kind.as_str() != "tree")
            .map(|(path, kind)| {
                if kind != "blob" {
                    return Err(invalid("managed Pages tree contains a submodule"));
                }
                Ok(path.clone())
            })
            .collect::<Result<_, _>>()?;
        let expected: BTreeSet<_> = files.iter().map(|f| f.path.clone()).collect();
        if actual != expected {
            return Err(invalid(
                "committed Pages tree differs from the requested file set",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, path_regex, query_param},
    };
    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    #[tokio::test]
    async fn malformed_file_identity_and_paginated_duplicates_cannot_build_delete_actions() {
        for duplicate in [false, true] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/projects/42/repository/branches/main"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!({"name":"main","commit":{"id":SHA}})),
                )
                .mount(&server)
                .await;
            if duplicate {
                let rows: Vec<_> = (0..100)
                    .map(|n| json!({"id":SHA,"path":format!("public/{n}.txt"),"type":"blob"}))
                    .collect();
                Mock::given(method("GET"))
                    .and(path("/projects/42/repository/tree"))
                    .and(query_param("page", "1"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(&rows))
                    .expect(1)
                    .mount(&server)
                    .await;
                Mock::given(method("GET"))
                    .and(path("/projects/42/repository/tree"))
                    .and(query_param("page", "2"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(json!([rows[0]])))
                    .expect(1)
                    .mount(&server)
                    .await;
            } else {
                Mock::given(method("GET"))
                    .and(path("/projects/42/repository/tree"))
                    .respond_with(
                        ResponseTemplate::new(200).set_body_json(
                            json!([{"id":SHA,"path":"public/stale.txt","type":"blob"}]),
                        ),
                    )
                    .mount(&server)
                    .await;
                Mock::given(method("GET"))
                    .and(path_regex("/repository/files/"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(
                        json!({"file_path":"README.md","commit_id":SHA,"last_commit_id":SHA}),
                    ))
                    .expect(1)
                    .mount(&server)
                    .await;
            }
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
            assert!(
                GitLabApi::with_base("test", server.uri())
                    .replacement_actions(
                        "42",
                        "main",
                        &[RepoFile {
                            path: "public/index.html".into(),
                            content: b"desired".to_vec()
                        }]
                    )
                    .await
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn invalid_credentials_do_not_fall_back_to_anonymous_requests() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        for token in ["", "\ninvalid"] {
            assert!(
                GitLabApi::with_base(token, server.uri())
                    .get_project("42")
                    .await
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn missing_branch_requires_an_explicit_empty_repository_read() {
        for empty in [false, true] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/projects/42/repository/branches/main"))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/projects/42"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"id":42,"empty_repo":empty})),
                )
                .mount(&server)
                .await;
            let result = GitLabApi::with_base("test", server.uri())
                .branch_head("42", "main")
                .await;
            if empty {
                assert_eq!(result.unwrap(), None);
            } else {
                assert!(result.is_err());
            }
        }
    }
}
