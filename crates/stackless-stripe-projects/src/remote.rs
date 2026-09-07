//! Stripe Projects 0.35 provisioning contract. A missing row or HTTP error is unknown.
//! Only the remote `removed` status proves deprovisioning completed.

use crate::{CommandRunner, ProjectsError, StripeProjects};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const PREFIX: &str = "/v2/provisioning/internal/resources";

pub(crate) fn invalid(detail: impl Into<String>) -> ProjectsError {
    ProjectsError::Journal {
        detail: detail.into(),
    }
}

fn identifier(id: &str) -> Result<&str, ProjectsError> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid("invalid remote provisioning identifier"));
    }
    Ok(id)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Resource {
    pub id: String,
    pub name: Option<String>,
    pub provider: String,
    pub service_ref: String,
    pub status: String,
}

impl Resource {
    fn validate(&self) -> Result<(), ProjectsError> {
        identifier(&self.id)?;
        if self.provider.is_empty() || self.service_ref.is_empty() || self.status.is_empty() {
            return Err(invalid("remote resource lacks identity or status"));
        }
        Ok(())
    }

    pub fn observation(&self) -> Result<stackless_core::substrate::Observation, ProjectsError> {
        use stackless_core::substrate::Observation;
        match self.status.as_str() {
            "removed" => Ok(Observation::Gone),
            "complete" => Ok(Observation::Present),
            _ => Err(invalid(format!(
                "remote resource {} has unresolved status {:?}",
                self.id, self.status
            ))),
        }
    }
}

pub async fn get<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    id: &str,
) -> Result<Resource, ProjectsError> {
    let value = stripe
        .request("GET", &format!("{PREFIX}/{}", identifier(id)?))
        .await?;
    let resource: Resource =
        serde_json::from_value(value).map_err(|_| invalid("invalid remote resource response"))?;
    resource.validate()?;
    if resource.id != id {
        return Err(invalid("remote response changed resource identity"));
    }
    Ok(resource)
}

pub async fn list<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    project: &str,
) -> Result<Vec<Resource>, ProjectsError> {
    #[derive(Deserialize)]
    struct Page {
        data: Vec<Resource>,
        next_page_url: Option<String>,
    }
    let mut next = Some(format!(
        "{PREFIX}?project={}&limit=100",
        identifier(project)?
    ));
    let mut visited = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut resources = Vec::new();
    while let Some(path) = next {
        if !visited.insert(path.clone()) || visited.len() > 1000 {
            return Err(invalid(
                "remote resource pagination repeated or exceeded 1000 pages",
            ));
        }
        let page: Page = serde_json::from_value(stripe.request("GET", &path).await?)
            .map_err(|_| invalid("invalid remote resource page"))?;
        for resource in page.data {
            resource.validate()?;
            if !ids.insert(resource.id.clone()) {
                return Err(invalid("remote inventory repeated a resource ID"));
            }
            resources.push(resource);
        }
        next = page
            .next_page_url
            .map(|url| {
                let path = url.strip_prefix("https://api.stripe.com").unwrap_or(&url);
                if !path.starts_with(&format!("{PREFIX}?")) || path.contains('#') {
                    return Err(invalid("remote pagination changed its API route"));
                }
                Ok(path.to_owned())
            })
            .transpose()?;
    }
    Ok(resources)
}

pub async fn remove<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    id: &str,
) -> Result<(), ProjectsError> {
    let value = stripe
        .request("POST", &format!("{PREFIX}/{}/remove", identifier(id)?))
        .await?;
    match value.get("status").and_then(serde_json::Value::as_str) {
        Some("removed" | "pending") => Ok(()),
        _ => Err(invalid(
            "remote removal returned no accepted deletion status",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandOutput;
    use serde_json::{Value, json};
    use std::{collections::VecDeque, path::Path, sync::Mutex};

    struct Runner {
        replies: Mutex<VecDeque<Value>>,
        calls: Mutex<Vec<String>>,
    }
    #[async_trait::async_trait]
    impl CommandRunner for Runner {
        async fn run(&self, _args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
            panic!("must use direct API");
        }
        async fn request(
            &self,
            method: &str,
            path: &str,
            _cwd: &Path,
        ) -> Result<CommandOutput, ProjectsError> {
            self.calls.lock().unwrap().push(format!("{method} {path}"));
            Ok(CommandOutput {
                status: 0,
                stdout: self
                    .replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected API request")
                    .to_string(),
                stderr: String::new(),
            })
        }
    }
    fn driver(replies: Vec<Value>) -> StripeProjects<Runner> {
        StripeProjects::new(
            Runner {
                replies: Mutex::new(replies.into()),
                calls: Mutex::new(vec![]),
            },
            ".",
        )
    }
    fn resource(id: &str) -> Value {
        json!({"id": id, "name": "owned-db", "provider": "provider_1", "service_ref": "database", "status": "complete"})
    }

    #[tokio::test]
    async fn inventory_reads_every_page_and_rejects_incomplete_or_ambiguous_results() {
        let next = format!("{PREFIX}?page=two");
        let stripe = driver(vec![
            json!({"data": [resource("one")], "next_page_url": format!("https://api.stripe.com{next}")}),
            json!({"data": [resource("two")], "next_page_url": null}),
        ]);
        assert_eq!(list(&stripe, "project_1").await.unwrap().len(), 2);
        for replies in [
            vec![json!({})],
            vec![json!({"data": [resource("one"), resource("one")], "next_page_url": null})],
            vec![json!({"data": [], "next_page_url": "https://other.example/resources"})],
            vec![
                json!({"data": [], "next_page_url": format!("{PREFIX}?project=project_1&limit=100")}),
            ],
            vec![json!({"error": {"message": "request denied"}})],
        ] {
            assert!(list(&driver(replies), "project_1").await.is_err());
        }
    }

    #[tokio::test]
    async fn resource_read_rejects_missing_or_changed_identity() {
        for reply in [
            json!({}),
            json!({"status": "removed"}),
            resource("different"),
            json!({"error": {"code": "not_found"}}),
        ] {
            assert!(get(&driver(vec![reply]), "one").await.is_err());
        }
        assert!(get(&driver(vec![]), "../other").await.is_err());
        assert!(list(&driver(vec![]), "project&other=1").await.is_err());
    }
}
