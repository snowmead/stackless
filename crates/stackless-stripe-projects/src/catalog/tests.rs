use super::*;
use serde_json::json;

fn service(reference: &str, schema: Value, pricing: Value) -> ServiceDetail {
    let (provider, service_id) = reference.split_once('/').unwrap();
    serde_json::from_value(json!({
        "id": "prvsvc_test",
        "object": "v2.provisioning.provider_service_detail",
        "provider_id": "prvdr_test",
        "provider_name": provider,
        "service_id": service_id,
        "categories": [],
        "kind": "deployable",
        "scope": "project",
        "availability": "available",
        "development": false,
        "livemode": true,
        "pricing": pricing,
        "configuration_schema": schema,
    }))
    .unwrap()
}

#[test]
fn reference_is_provider_lowercased_slash_service_id() {
    let svc = service("Render/postgres", json!({}), json!({"type": "free"}));
    assert_eq!(svc.reference(), "render/postgres");
}

#[test]
fn validate_accepts_pricing_selector_keys_outside_schema() {
    // render/postgres: instance_type is a tier selector, not a schema prop.
    let svc = service(
        "Render/postgres",
        json!({
            "type": "object",
            "required": ["name"],
            "additionalProperties": false,
            "properties": { "name": {"type": "string"}, "version": {"type": "string"} }
        }),
        json!({
            "type": "paid",
            "paid_pricing": [
                {"configuration": {"instance_type": "free"}, "is_default": true, "type": "free"},
                {"configuration": {"instance_type": "basic-256mb"}, "type": "freeform"}
            ]
        }),
    );
    svc.validate_config(&json!({"name": "db", "version": "17", "instance_type": "basic-256mb"}))
        .unwrap();
}

#[test]
fn validate_rejects_unknown_field_and_bad_tier_and_missing_required() {
    let svc = service(
        "Render/postgres",
        json!({
            "type": "object",
            "required": ["name"],
            "additionalProperties": false,
            "properties": { "name": {"type": "string"} }
        }),
        json!({
            "type": "paid",
            "paid_pricing": [
                {"configuration": {"instance_type": "free"}, "is_default": true, "type": "free"},
                {"configuration": {"instance_type": "basic-256mb"}, "type": "freeform"}
            ]
        }),
    );
    let err = svc
        .validate_config(&json!({"bogus": 1, "instance_type": "nope"}))
        .unwrap_err();
    assert!(
        err.iter()
            .any(|v| v.contains("missing required field `name`")),
        "{err:?}"
    );
    assert!(
        err.iter().any(|v| v.contains("unknown field `bogus`")),
        "{err:?}"
    );
    assert!(
        err.iter().any(|v| v.contains("not an allowed tier value")),
        "{err:?}"
    );
}

#[test]
fn validate_enforces_enum_and_type() {
    let svc = service(
        "Render/web-service",
        json!({
            "type": "object",
            "required": ["name", "runtime"],
            "additionalProperties": false,
            "properties": {
                "name": {"type": "string"},
                "runtime": {"type": "string", "enum": ["rust", "node"]},
                "auto_deploy": {"type": "string", "enum": ["yes", "no"]}
            }
        }),
        json!({"type": "paid", "paid_pricing": [
            {"configuration": {"instance_type": "free"}, "is_default": true, "type": "free"}
        ]}),
    );
    svc.validate_config(&json!({"name": "api", "runtime": "rust", "auto_deploy": "no"}))
        .unwrap();
    let err = svc
        .validate_config(&json!({"name": "api", "runtime": "elixir"}))
        .unwrap_err();
    assert!(err.iter().any(|v| v.contains("not in enum")), "{err:?}");
}

#[test]
fn requires_confirmation_is_per_tier() {
    let svc = service(
        "Render/postgres",
        json!({"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}),
        json!({
            "type": "paid",
            "paid_pricing": [
                {"configuration": {"instance_type": "free"}, "is_default": true, "type": "free"},
                {"configuration": {"instance_type": "basic-256mb"}, "type": "freeform"}
            ]
        }),
    );
    assert!(svc.requires_confirmation(&json!({"name": "db", "instance_type": "basic-256mb"})));
    // No selector → default (free) tier → no confirmation.
    assert!(!svc.requires_confirmation(&json!({"name": "db"})));
}

#[test]
fn requires_confirmation_falls_back_to_pricing_kind() {
    let free = service("Render/static-site", json!({}), json!({"type": "free"}));
    assert!(!free.requires_confirmation(&json!({})));
    let paid = service(
        "Vercel/pro",
        json!({}),
        json!({"type": "paid", "paid_pricing": [{"type": "freeform"}]}),
    );
    assert!(paid.requires_confirmation(&json!({})));
    let component = service("Clerk/auth", json!({}), json!({"type": "component"}));
    assert!(!component.requires_confirmation(&json!({})));
}

#[test]
fn required_parent_services_returns_single_parent_only() {
    let kv = service(
        "cloudflare/kv",
        json!({"type": "object", "properties": {"title": {"type": "string"}}, "required": ["title"]}),
        json!({
            "type": "component",
            "component": {
                "options": [
                    {"type": "free", "parent_services": ["workers:free"]},
                    {"type": "paid", "parent_services": ["workers:paid"]}
                ]
            }
        }),
    );
    assert_eq!(
        kv.required_parent_services(&json!({"title": "cache"}), false),
        vec!["workers:free"]
    );
    assert_eq!(
        kv.required_parent_services(&json!({"title": "cache"}), true),
        vec!["workers:paid"]
    );
    assert!(kv.requires_confirmation_with_paid(&json!({"title": "cache"}), true));

    let vercel = service(
        "vercel/project",
        json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
        json!({
            "type": "component",
            "component": {
                "options": [
                    {"type": "free", "parent_services": ["pro", "hobby"]}
                ]
            }
        }),
    );
    assert!(
        vercel
            .required_parent_services(&json!({"name": "demo"}), false)
            .is_empty()
    );
}

#[test]
fn match_option_without_paid_consent_ignores_paid_default() {
    let svc = service(
        "cloudflare/kv",
        json!({}),
        json!({
            "type": "component",
            "component": {
                "options": [
                    {"type": "paid", "is_default": true, "parent_services": ["workers:paid"]},
                    {"type": "free", "parent_services": ["workers:free"]}
                ]
            }
        }),
    );
    assert!(!svc.requires_confirmation_with_paid(&json!({}), false));
    assert_eq!(
        svc.required_parent_services(&json!({}), false),
        vec!["workers:free"]
    );
    assert!(svc.requires_confirmation_with_paid(&json!({}), true));
    assert_eq!(
        svc.required_parent_services(&json!({}), true),
        vec!["workers:paid"]
    );
}

#[test]
fn config_schema_deserializes_optional_without_drift() {
    let schema_json = json!({
        "type": "object",
        "required": ["region"],
        "optional": ["store_name", "plan"],
        "properties": {
            "region": {"type": "string"},
            "store_name": {"type": "string"},
            "plan": {"type": "string"}
        }
    });
    let schema: ConfigSchema = serde_json::from_value(schema_json.clone()).unwrap();
    assert_eq!(
        schema.optional,
        vec!["store_name".to_owned(), "plan".to_owned()]
    );

    let svc = service("shopify/store", schema_json, json!({"type": "free"}));
    let catalog = Catalog {
        last_updated: "test".to_owned(),
        provider: None,
        category_filter: None,
        provider_filter: None,
        source: None,
        services: vec![svc],
        extra: BTreeMap::new(),
    };
    let drift = catalog.drift_report();
    assert!(
        !drift.iter().any(|line| line.contains("optional")),
        "optional must be modeled; drift={drift:?}"
    );
}

#[test]
fn match_option_with_paid_consent_ignores_free_default() {
    let svc = service(
        "cloudflare/kv",
        json!({}),
        json!({
            "type": "component",
            "component": {
                "options": [
                    {"type": "free", "is_default": true, "parent_services": ["workers:free"]},
                    {"type": "paid", "parent_services": ["workers:paid"]}
                ]
            }
        }),
    );
    assert_eq!(
        svc.required_parent_services(&json!({}), true),
        vec!["workers:paid"]
    );
}
