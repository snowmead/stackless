//! A deployment request belongs to an owned service and is durable before POST.

use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::substrate::SubstrateFault;

use crate::render_api::{RenderApi, RenderDeploy};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Deployment {
    pub revision: String,
    pub commit: String,
    pub before: BTreeSet<String>,
    pub submitted: bool,
    pub id: Option<String>,
}

pub(crate) fn invalid(detail: impl Into<String>) -> SubstrateFault {
    SubstrateFault::from_fault(&stackless_core::state::StateError::ResourceInvariant {
        detail: detail.into(),
    })
}

impl Deployment {
    pub fn new(
        revision: String,
        commit: String,
        before: Vec<RenderDeploy>,
    ) -> Result<Self, SubstrateFault> {
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(invalid(
                "Render deployment requires an immutable Git commit",
            ));
        }
        Ok(Self {
            revision,
            commit,
            before: before.into_iter().map(|d| d.id).collect(),
            submitted: false,
            id: None,
        })
    }

    /// Auto-deploy is disabled before the inventory snapshot. Only one new API
    /// deployment of the pinned commit can resolve an otherwise lost receipt.
    pub async fn recover(
        &self,
        api: &RenderApi,
        service: &str,
    ) -> Result<Option<RenderDeploy>, SubstrateFault> {
        let deploy = self.lookup(api, service).await?;
        if self.submitted && deploy.is_none() {
            return Err(invalid(
                "Render deployment submission is unresolved; refusing another POST",
            ));
        }
        Ok(deploy)
    }

    /// A 202 queues the request behind an existing deployment without returning an ID.
    /// Wait for its receipt without submitting another request or adopting a foreign deploy.
    pub async fn wait_for_receipt(
        &self,
        api: &RenderApi,
        service: &str,
        budget: Duration,
    ) -> Result<Option<RenderDeploy>, SubstrateFault> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let deploy = self.lookup(api, service).await?;
            if deploy.is_some() || !self.submitted {
                return Ok(deploy);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(invalid(
                    "Render deployment receipt did not appear before the deadline; refusing another POST",
                ));
            }
            tokio::time::sleep(api.poll_interval().min(remaining)).await;
        }
    }

    async fn lookup(
        &self,
        api: &RenderApi,
        service: &str,
    ) -> Result<Option<RenderDeploy>, SubstrateFault> {
        if let Some(id) = &self.id {
            let deploy = api.get_deploy(service, id).await.map_err(crate::fault)?;
            self.check(&deploy)?;
            return Ok(Some(deploy));
        }
        if !self.submitted {
            return Ok(None);
        }
        let new: Vec<_> = api
            .deployments(service)
            .await
            .map_err(crate::fault)?
            .into_iter()
            .filter(|d| !self.before.contains(&d.id))
            .collect();
        if new.is_empty() {
            return Ok(None);
        }
        if new.len() != 1 {
            return Err(invalid(
                "Render deployment submission is unresolved; refusing another POST",
            ));
        }
        let deploy = new
            .into_iter()
            .next()
            .ok_or_else(|| invalid("deployment disappeared"))?;
        self.check(&deploy)?;
        if deploy.trigger.as_deref() != Some("api") {
            return Err(invalid(
                "new Render deployment was not triggered by the API",
            ));
        }
        Ok(Some(deploy))
    }

    pub fn check(&self, deploy: &RenderDeploy) -> Result<(), SubstrateFault> {
        if deploy.commit.as_deref() != Some(self.commit.as_str())
            || self.id.as_ref().is_some_and(|id| id != &deploy.id)
        {
            return Err(invalid(
                "Render deployment does not match the submitted ID and commit",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[tokio::test]
    async fn unresolved_and_foreign_deployments_cannot_be_adopted() {
        let server = MockServer::start().await;
        let api = RenderApi::with_base("test", server.uri());
        let commit = "a".repeat(40);
        let mut attempt = Deployment::new("revision".into(), commit.clone(), vec![]).unwrap();
        attempt.submitted = true;
        for rows in [
            vec![],
            vec![
                json!({"deploy":{"id":"wrong_commit","status":"live","commit":{"id":"b".repeat(40)},"trigger":"api"}}),
            ],
            vec![
                json!({"deploy":{"id":"auto","status":"live","commit":{"id":commit},"trigger":"service_created"}}),
            ],
            vec![
                json!({"deploy":{"id":"one","status":"live","commit":{"id":commit},"trigger":"api"}}),
                json!({"deploy":{"id":"two","status":"live","commit":{"id":commit},"trigger":"api"}}),
            ],
        ] {
            Mock::given(method("GET"))
                .and(path("/services/srv_one/deploys"))
                .respond_with(ResponseTemplate::new(200).set_body_json(rows))
                .expect(1)
                .mount(&server)
                .await;
            assert!(attempt.recover(&api, "srv_one").await.is_err());
            assert!(attempt.submitted);
            assert!(attempt.id.is_none());
            server.verify().await;
            server.reset().await;
        }
    }

    #[tokio::test]
    async fn queued_receipt_timeout_retains_submission_and_rejects_ambiguous_inventory() {
        let server = MockServer::start().await;
        let api = RenderApi::with_base("test", server.uri());
        let commit = "a".repeat(40);
        let mut attempt = Deployment::new("revision".into(), commit.clone(), vec![]).unwrap();
        attempt.submitted = true;
        for rows in [
            vec![],
            vec![
                json!({"deploy":{"id":"one","status":"live","commit":{"id":commit},"trigger":"api"}}),
                json!({"deploy":{"id":"two","status":"live","commit":{"id":commit},"trigger":"api"}}),
            ],
        ] {
            Mock::given(method("GET"))
                .and(path("/services/srv_one/deploys"))
                .respond_with(ResponseTemplate::new(200).set_body_json(rows))
                .expect(1)
                .mount(&server)
                .await;
            assert!(
                attempt
                    .wait_for_receipt(&api, "srv_one", Duration::ZERO)
                    .await
                    .is_err()
            );
            assert!(attempt.submitted);
            assert!(attempt.id.is_none());
            server.verify().await;
            server.reset().await;
        }
    }
}
