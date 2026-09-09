//! Provider support is validated before admission or external effects.

use serde::{Deserialize, Serialize};

use crate::def::{StackDef, WorkloadKind};
use crate::substrate::SubstrateFault;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub early_origins: bool,
    #[serde(default)]
    pub tcp_services: bool,
    pub workers: bool,
    pub jobs: bool,
    pub local_sources: bool,
    pub empty_sources: bool,
    pub environment: bool,
    pub sandbox: bool,
    pub containers: bool,
    pub logs: bool,
}

impl Capabilities {
    pub fn local() -> Self {
        Self {
            early_origins: true,
            tcp_services: true,
            workers: true,
            jobs: true,
            local_sources: true,
            empty_sources: true,
            environment: true,
            sandbox: true,
            containers: true,
            logs: true,
        }
    }

    pub fn cloud(environment: bool, logs: bool) -> Self {
        Self {
            early_origins: false,
            tcp_services: false,
            workers: false,
            jobs: false,
            local_sources: false,
            empty_sources: false,
            environment,
            sandbox: false,
            containers: false,
            logs,
        }
    }

    pub fn validate(&self, provider: &str, def: &StackDef) -> Result<(), SubstrateFault> {
        for (name, workload) in &def.services {
            if workload.on.as_ref().is_some_and(|on| on != provider) {
                continue;
            }
            let unsupported = match workload.kind {
                _ if workload
                    .health
                    .as_ref()
                    .is_some_and(|health| health.is_tcp())
                    && !self.tcp_services =>
                {
                    Some("TCP services")
                }
                _ if workload.image.is_some() && !self.containers => Some("container images"),
                WorkloadKind::Worker if !self.workers => Some("workers"),
                WorkloadKind::Job if !self.jobs => Some("jobs"),
                _ if workload.source.path.is_some() && !self.local_sources => {
                    Some("local source paths")
                }
                _ if workload.source.repo.is_empty()
                    && workload.source.path.is_none()
                    && !self.empty_sources =>
                {
                    Some("workloads without a git source")
                }
                _ if !self.environment
                    && (!workload.env.is_empty()
                        || !workload.secrets.is_empty()
                        || !workload
                            .substrate_env(name, provider)
                            .map_err(|error| SubstrateFault::from_fault(&error))?
                            .is_empty()) =>
                {
                    Some("environment injection")
                }
                _ => None,
            };
            if let Some(feature) = unsupported {
                return Err(unsupported_feature(provider, name, feature));
            }
        }
        Ok(())
    }
}

pub fn unsupported_feature(provider: &str, workload: &str, feature: &str) -> SubstrateFault {
    SubstrateFault {
        code: "provider.unsupported".into(),
        message: format!("{provider} does not support {feature} for workload {workload:?}"),
        remediation:
            "choose a provider that advertises this capability or change the workload definition"
                .into(),
        context: Box::default(),
    }
}
