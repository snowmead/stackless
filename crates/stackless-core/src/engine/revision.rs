//! Stable digests of the inputs each lifecycle step consumes.

use serde_json::json;
use sha2::{Digest, Sha256};

use super::StepKind;
use crate::substrate::{StepContext, SubstrateFault};

pub fn digest(value: &impl serde::Serialize) -> Result<String, SubstrateFault> {
    let bytes = serde_json::to_vec(value).map_err(|_| SubstrateFault {
        code: "engine.revision_invalid".into(),
        message: "cannot serialize desired inputs".into(),
        remediation: "repair the definition before retrying".into(),
        context: Box::default(),
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn step_revision(
    ctx: &StepContext<'_>,
    substrate: &(impl crate::substrate::Substrate + ?Sized),
) -> Result<String, SubstrateFault> {
    let provider = substrate.name();
    let name = &ctx.step.node;
    let value = if ctx.step.kind == StepKind::ProvisionIntegration {
        json!({"integration":ctx.def.integrations.get(name), "project":ctx.def.stack.projects})
    } else if let Some(service) = ctx.def.services.get(name) {
        match ctx.step.kind {
            StepKind::Materialize => {
                json!({"source":service.source,"override":ctx.source_overrides.get(name),"dirty":ctx.dirty,"image":service.image,"setup":service.setup,"prepare":service.prepare,"runtime":service.substrates.get(provider)})
            }
            StepKind::Setup => {
                json!({"source":service.source,"setup":service.setup,"env":service.env,"secrets":service.secrets,"timeout_secs":service.timeout_secs,"runtime":service.substrates.get(provider)})
            }
            StepKind::HealthGate => json!({"health":service.health}),
            _ => {
                json!({"source":service.source,"prepare":service.prepare,"env":service.env,"secrets":service.secrets,"runtime":service.substrates.get(provider),"stack":ctx.def.stack.substrates.get(provider),"root_origin":service.root_origin,"run":service.run,"kind":service.kind,"image":service.image,"timeout_secs":service.timeout_secs})
            }
        }
    } else {
        serde_json::Value::Null
    };
    let mut origin_inputs = std::collections::BTreeSet::new();
    let mut endpoint_inputs = std::collections::BTreeMap::new();
    let mut integration_steps = std::collections::BTreeSet::new();
    if let Some(service) = ctx.def.services.get(name) {
        for value in service
            .effective_env(name, provider)
            .map_err(|error| SubstrateFault::from_fault(&error))?
            .values()
        {
            for reference in crate::def::interp::references(value, "env")
                .map_err(|error| SubstrateFault::from_fault(&error))?
            {
                match reference {
                    crate::def::Reference::IntegrationOutput { integration, .. } => {
                        integration_steps.insert(format!("integration:{integration}"));
                    }
                    crate::def::Reference::ServiceOrigin(service) => {
                        origin_inputs.insert(service);
                    }
                    crate::def::Reference::EndpointUrl(endpoint) => {
                        if let Some(spec) = ctx.def.endpoints.get(&endpoint) {
                            endpoint_inputs.insert(endpoint, spec);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let upstream: Vec<_> = if matches!(
        ctx.step.kind,
        StepKind::Setup | StepKind::Prepare | StepKind::Start | StepKind::RunJob
    ) {
        ctx.prior
            .iter()
            .filter(|cp| {
                cp.step_id == format!("materialize:{name}")
                    || integration_steps.contains(&cp.step_id)
            })
            .map(|cp| {
                let payload = if cp.resource_kind == "source-snapshot" {
                    let value: serde_json::Value =
                        serde_json::from_str(&cp.payload).map_err(|_| SubstrateFault {
                            code: "engine.revision_invalid".into(),
                            message: "invalid source snapshot checkpoint".into(),
                            remediation: "inspect the recorded source snapshot".into(),
                            context: Box::default(),
                        })?;
                    if matches!(ctx.step.kind, StepKind::Setup | StepKind::Prepare) {
                        // Hooks consume the working copy, including setup output. A new
                        // snapshot needs its own execution even when the commit is unchanged.
                        json!({"repo":value["repo"],"commit":value["commit"],"digest":value["digest"],"snapshot":value["key"]})
                    } else {
                        json!({"repo":value["repo"],"commit":value["commit"],"digest":value["digest"]})
                    }
                } else {
                    serde_json::Value::String(cp.payload.clone())
                };
                Ok((cp.step_id.clone(), payload))
            })
            .collect::<Result<Vec<_>, SubstrateFault>>()?
    } else {
        Vec::new()
    };
    let mut inputs = json!({"kind":ctx.step.kind,"input":value,"upstream":upstream});
    if (!endpoint_inputs.is_empty() || !origin_inputs.is_empty())
        && matches!(
            ctx.step.kind,
            StepKind::Setup | StepKind::Prepare | StepKind::Start | StepKind::RunJob
        )
    {
        let mut namespace = substrate.build_namespace(
            ctx.def,
            ctx.instance,
            ctx.prior,
            &std::collections::BTreeMap::new(),
            crate::substrate::NamespacePurpose::ServiceEnv,
        );
        namespace.bind_endpoints(ctx.def);
        let endpoint_urls: std::collections::BTreeMap<_, _> = endpoint_inputs
            .keys()
            .map(|name| (name, namespace.endpoint_urls.get(name)))
            .collect();
        if !endpoint_inputs.is_empty() {
            inputs["endpoint_urls"] = json!(endpoint_urls);
            inputs["endpoints"] = json!(endpoint_inputs);
        }
        if !origin_inputs.is_empty() {
            let origins: std::collections::BTreeMap<_, _> = origin_inputs
                .iter()
                .map(|name| (name, namespace.service_origins.get(name)))
                .collect();
            inputs["service_origins"] = json!(origins);
        }
    }
    digest(&inputs)
}
