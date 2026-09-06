//! The catalog-anchored provisioning seam.
//!
//! Every provisionable service implements [`CatalogService`] (a reference plus a
//! `Serialize` config). [`add_catalog_resource`] is the single path to
//! `stripe projects add`: it validates the config against the catalog schema and
//! derives paid confirmation from the selected pricing tier. [`verify_service`]
//! is the test-time gap check that reuses the exact same validation.

use serde::Serialize;
use serde_json::{Value, json};

use crate::catalog::Catalog;
use crate::error::ProjectsError;
use crate::project::{self, AddedResource};
use crate::stripe::{CommandRunner, StripeProjects};

/// A provisionable catalog service: a typed config bound to a catalog reference.
pub trait CatalogService: Serialize {
    /// The `stripe projects add <reference>` key, e.g. `"render/postgres"`.
    const REFERENCE: &'static str;
}

/// Add a catalog resource: look up the reference, validate the serialized config
/// against the catalog schema, derive paid confirmation from the selected tier,
/// then `stripe projects add`. Returns the attached local name and add payload.
pub async fn add_catalog_resource<C, R>(
    stripe: &StripeProjects<R>,
    catalog: &Catalog,
    config: &C,
    resource_name: &str,
) -> Result<AddedResource, ProjectsError>
where
    C: CatalogService,
    R: CommandRunner,
{
    add_catalog_resource_with_paid(stripe, catalog, config, resource_name, false).await
}

/// Like [`add_catalog_resource`], but passes explicit paid consent into
/// component-pricing tier selection and parent-plan provisioning.
pub async fn add_catalog_resource_with_paid<C, R>(
    stripe: &StripeProjects<R>,
    catalog: &Catalog,
    config: &C,
    resource_name: &str,
    confirm_paid: bool,
) -> Result<AddedResource, ProjectsError>
where
    C: CatalogService,
    R: CommandRunner,
{
    let value = serde_json::to_value(config).map_err(|err| ProjectsError::ProvisionFailed {
        resource: resource_name.to_owned(),
        detail: format!("config for {} did not serialize: {err}", C::REFERENCE),
    })?;
    let service = catalog
        .lookup(C::REFERENCE)
        .ok_or_else(|| ProjectsError::CatalogMissing {
            reference: C::REFERENCE.to_owned(),
        })?;
    service
        .validate_config(&value)
        .map_err(|violations| ProjectsError::ConfigSchema {
            reference: C::REFERENCE,
            violations,
        })?;
    let paid = service.requires_confirmation_with_paid(&value, confirm_paid);
    ensure_parent_plans(stripe, catalog, service, &value, confirm_paid).await?;
    project::add_resource(stripe, C::REFERENCE, resource_name, &value, paid).await
}

/// Provision catalog-named parent plans before a dependent service. Stripe
/// Projects 0.23+ enforces `PLAN_REQUIRED` when `parent_services` is unset.
async fn ensure_parent_plans<R: CommandRunner>(
    stripe: &StripeProjects<R>,
    catalog: &Catalog,
    service: &crate::catalog::ServiceDetail,
    config: &Value,
    prefer_paid: bool,
) -> Result<(), ProjectsError> {
    for plan_id in service.required_parent_services(config, prefer_paid) {
        let reference = format!("{}/{}", service.provider_name.to_ascii_lowercase(), plan_id);
        let plan = catalog
            .lookup(&reference)
            .ok_or_else(|| ProjectsError::ProvisionFailed {
                resource: plan_id.clone(),
                detail: format!("parent plan {reference} not found in catalog"),
            })?;
        // Paid parent plans require explicit consent. Without it, let the
        // plugin reject a missing paid plan before provisioning the child.
        let needs_paid = plan.requires_confirmation_with_paid(&json!({}), prefer_paid);
        let paid = needs_paid && prefer_paid;
        project::add_resource(stripe, &reference, &plan_id, &json!({}), paid).await?;
    }
    Ok(())
}

/// Whether provisioning `config` for `reference` needs paid confirmation, per the
/// catalog's selected pricing tier. Returns `None` if the reference is absent.
pub fn requires_confirmation<C>(catalog: &Catalog, config: &C) -> Option<bool>
where
    C: CatalogService,
{
    let value = serde_json::to_value(config).ok()?;
    catalog
        .lookup(C::REFERENCE)
        .map(|service| service.requires_confirmation(&value))
}

/// Test-time gap check: assert a service's reference exists in `catalog` and a
/// representative config validates against its schema + pricing tiers. Returns
/// violation strings (empty means no gap). Reuses the runtime validator.
pub fn verify_service<C>(catalog: &Catalog, sample: &C) -> Vec<String>
where
    C: CatalogService,
{
    let mut out = Vec::new();
    let Some(service) = catalog.lookup(C::REFERENCE) else {
        out.push(format!("{}: reference not found in catalog", C::REFERENCE));
        return out;
    };
    match serde_json::to_value(sample) {
        Ok(value) => {
            if let Err(violations) = service.validate_config(&value) {
                out.extend(
                    violations
                        .into_iter()
                        .map(|v| format!("{}: {v}", C::REFERENCE)),
                );
            }
        }
        Err(err) => out.push(format!(
            "{}: sample config did not serialize: {err}",
            C::REFERENCE
        )),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ScriptedRunner, ok, ok_empty, raw, services};
    use serde::Serialize;
    use serde_json::json;
    use tempfile::tempdir;

    #[derive(Serialize)]
    struct AppCfg {}

    impl CatalogService for AppCfg {
        const REFERENCE: &'static str = "example/app";
    }

    fn catalog_with_paid_parent(child_pricing: &str) -> Catalog {
        let envelope = json!({
            "ok": true,
            "command": "projects catalog",
            "data": {
                "last_updated": "2026-09-02T00:00:00Z",
                "services": [
                    {
                        "id": "prvsvc_app",
                        "object": "v2.provisioning.provider_service_detail",
                        "provider_id": "prvdr_example",
                        "provider_name": "Example",
                        "service_id": "app",
                        "categories": ["communications"],
                        "kind": "deployable",
                        "scope": "project",
                        "availability": "available",
                        "development": false,
                        "livemode": true,
                        "pricing": {
                            "type": "component",
                            "component": {
                                "options": [{
                                    "type": child_pricing,
                                    "parent_services": ["starter"]
                                }]
                            }
                        },
                        "configuration_schema": {
                            "type": "object",
                            "required": [],
                            "additionalProperties": false,
                            "properties": {}
                        }
                    },
                    {
                        "id": "prvsvc_starter",
                        "object": "v2.provisioning.provider_service_detail",
                        "provider_id": "prvdr_example",
                        "provider_name": "Example",
                        "service_id": "starter",
                        "categories": ["communications"],
                        "kind": "plan",
                        "scope": "account",
                        "availability": "available",
                        "development": false,
                        "livemode": true,
                        "pricing": {
                            "type": "paid",
                            "paid": { "type": "freeform", "freeform": "$19/seat" },
                            "paid_pricing": [{
                                "type": "freeform",
                                "freeform": "$19/seat",
                                "is_default": true
                            }]
                        },
                        "configuration_schema": {
                            "type": "object",
                            "required": [],
                            "additionalProperties": false,
                            "properties": {}
                        }
                    }
                ]
            }
        });
        Catalog::from_json_envelope(&envelope.to_string()).unwrap()
    }

    #[tokio::test]
    async fn paid_parent_without_consent_blocks_free_and_paid_children() {
        for child_pricing in ["free", "paid"] {
            let catalog = catalog_with_paid_parent(child_pricing);
            let runner = ScriptedRunner::new(vec![
                services(&[]),
                raw(
                    r#"{"ok":false,"error":{"code":"PRICE_CONFIRMATION_REQUIRED","message":"paid parent requires confirmation"}}"#,
                ),
            ]);
            let dir = tempdir().unwrap();
            let stripe = StripeProjects::new(&runner, dir.path());
            let err = add_catalog_resource(&stripe, &catalog, &AppCfg {}, "res")
                .await
                .unwrap_err();
            assert!(matches!(err, ProjectsError::Failed { command, detail }
                if command == "add example/starter" && detail.contains("paid parent requires confirmation")));
            let calls = runner.calls();
            assert_eq!(calls.len(), 2, "must stop after parent rejection");
            let parent_add = &calls[1];
            assert!(
                parent_add
                    .windows(2)
                    .any(|w| w == ["add", "example/starter"])
            );
            assert!(
                !parent_add.iter().any(|a| a == "--confirm-paid-service"),
                "{child_pricing} child must not authorize a paid parent: {parent_add:?}"
            );
        }
    }

    #[tokio::test]
    async fn explicit_consent_confirms_paid_parent_of_free_child() {
        let catalog = catalog_with_paid_parent("free");
        let runner = ScriptedRunner::new(vec![
            services(&[]),                  // parent registered pre-check
            ok(json!({})),                  // parent add
            ok_empty(),                     // parent env add
            services(&[]),                  // child registered pre-check
            ok(json!({ "variables": {} })), // child add
            ok_empty(),                     // child env add
        ]);
        let dir = tempdir().unwrap();
        let stripe = StripeProjects::new(&runner, dir.path());
        add_catalog_resource_with_paid(&stripe, &catalog, &AppCfg {}, "res", true)
            .await
            .unwrap();
        let calls = runner.calls();
        let parent_add = calls
            .iter()
            .find(|c| c.windows(2).any(|w| w == ["add", "example/starter"]))
            .expect("parent plan add");
        assert!(
            parent_add.iter().any(|a| a == "--confirm-paid-service"),
            "explicit consent must confirm the paid parent; got {parent_add:?}"
        );
        let child_add = calls
            .iter()
            .find(|c| c.windows(2).any(|w| w == ["add", "example/app"]))
            .expect("child add");
        assert!(
            !child_add.iter().any(|a| a == "--confirm-paid-service"),
            "free child option must not confirm paid; got {child_add:?}"
        );
    }
}
