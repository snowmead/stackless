//! Typed model of `stripe projects catalog --json`.
//!
//! The catalog is the single source of truth for every provisionable provider
//! service: its `configuration_schema`, pricing tiers, constraints, and
//! categories. Plugins declare a [`CatalogService`](verify::CatalogService)
//! reference plus a typed config; [`ServiceDetail::validate_config`] checks that
//! config against the catalog, and [`ServiceDetail::requires_confirmation`]
//! derives paid confirmation from the selected pricing tier.
//!
//! Deserialization is permissive so `up` never breaks on additive drift: every
//! struct captures unmodeled keys in `extra`, and every enum has an `Unknown`
//! fallback. The strict [`Catalog::drift_report`] (used in tests) fails when the
//! live catalog contains anything we have not modeled.

pub mod verify;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `data` payload of a `stripe projects catalog --json` envelope.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Catalog {
    pub last_updated: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub category_filter: Option<String>,
    #[serde(default)]
    pub provider_filter: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    pub services: Vec<ServiceDetail>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Catalog {
    /// Parse a full `{ok, command, version, data}` envelope and return its
    /// `data` as a [`Catalog`].
    pub fn from_json_envelope(raw: &str) -> Result<Self, serde_json::Error> {
        #[derive(Deserialize)]
        struct Envelope {
            data: Catalog,
        }
        Ok(serde_json::from_str::<Envelope>(raw)?.data)
    }

    /// Look up a service by its `stripe projects add` reference
    /// (`provider_name` lowercased + `/` + `service_id`).
    pub fn lookup(&self, reference: &str) -> Option<&ServiceDetail> {
        self.services.iter().find(|s| s.reference() == reference)
    }

    /// Collect every unmodeled field (`extra`) and every `Unknown` enum across
    /// the whole catalog. Empty means the model fully covers the wire format.
    pub fn drift_report(&self) -> Vec<String> {
        let mut out = Vec::new();
        push_extra(&mut out, "catalog", &self.extra);
        for service in &self.services {
            service.collect_drift(&mut out);
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServiceDetail {
    pub id: String,
    pub object: String,
    pub provider_id: String,
    pub provider_name: String,
    pub service_id: String,
    #[serde(default)]
    pub categories: Vec<Category>,
    pub kind: Kind,
    pub scope: Scope,
    pub availability: Availability,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub llm_context: Option<String>,
    #[serde(default)]
    pub created: Option<Value>,
    pub development: bool,
    pub livemode: bool,
    #[serde(default)]
    pub allowed_updates: Vec<AllowedUpdate>,
    #[serde(default)]
    pub updateable_to: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    pub pricing: Pricing,
    #[serde(default)]
    pub configuration_schema: Option<ConfigSchema>,
    #[serde(default)]
    pub provider_configuration_schema: Option<ConfigSchema>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ServiceDetail {
    /// The `stripe projects add <reference>` key.
    pub fn reference(&self) -> String {
        format!(
            "{}/{}",
            self.provider_name.to_ascii_lowercase(),
            self.service_id
        )
    }

    /// Validate a serialized `--config` object against this service: required
    /// fields present, every key is a known schema property or pricing-tier
    /// selector, each value matches its property/selector constraints.
    pub fn validate_config(&self, config: &Value) -> Result<(), Vec<String>> {
        let mut violations = Vec::new();
        let object = match config.as_object() {
            Some(map) => map,
            None => {
                violations.push("config is not a JSON object".to_owned());
                return Err(violations);
            }
        };

        let schema = self.configuration_schema.as_ref();
        let selectors = self.pricing.selector_keys();

        // Required schema fields must be present.
        if let Some(schema) = schema {
            for name in &schema.required {
                if !object.contains_key(name) {
                    violations.push(format!("missing required field `{name}`"));
                }
            }
        }

        let allow_extra = schema
            .and_then(|s| s.additional_properties)
            .unwrap_or(false);

        for (key, value) in object {
            if let Some(property) = schema.and_then(|s| s.properties.get(key)) {
                violations.extend(
                    property
                        .validate_value(value)
                        .into_iter()
                        .map(|detail| format!("`{key}`: {detail}")),
                );
            } else if selectors.contains(key.as_str()) {
                let allowed = self.pricing.selector_values(key);
                if !allowed.iter().any(|candidate| candidate == value) {
                    let rendered: Vec<String> = allowed.iter().map(value_label).collect();
                    violations.push(format!(
                        "`{key}`: {} is not an allowed tier value (expected one of [{}])",
                        value_label(value),
                        rendered.join(", ")
                    ));
                }
            } else if !allow_extra {
                violations.push(format!(
                    "unknown field `{key}` (not in schema or pricing tiers)"
                ));
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }

    /// Whether provisioning this service with the given `--config` requires paid
    /// confirmation, derived from the selected pricing tier (or the service's
    /// pricing kind when there are no tier selectors).
    pub fn requires_confirmation(&self, config: &Value) -> bool {
        self.pricing.requires_confirmation(config)
    }

    fn collect_drift(&self, out: &mut Vec<String>) {
        let at = self.reference();
        push_extra(out, &at, &self.extra);
        push_unknown(out, &at, "kind", self.kind == Kind::Unknown);
        push_unknown(out, &at, "scope", self.scope == Scope::Unknown);
        push_unknown(
            out,
            &at,
            "availability",
            self.availability == Availability::Unknown,
        );
        for (i, category) in self.categories.iter().enumerate() {
            push_unknown(
                out,
                &at,
                &format!("categories[{i}]"),
                *category == Category::Unknown,
            );
        }
        for (i, update) in self.allowed_updates.iter().enumerate() {
            let path = format!("{at}.allowed_updates[{i}]");
            push_extra(out, &path, &update.extra);
            push_unknown(
                out,
                &path,
                "direction",
                update.direction == Direction::Unknown,
            );
        }
        for (i, constraint) in self.constraints.iter().enumerate() {
            constraint.collect_drift(out, &format!("{at}.constraints[{i}]"));
        }
        self.pricing.collect_drift(out, &at);
        if let Some(schema) = &self.configuration_schema {
            schema.collect_drift(out, &format!("{at}.configuration_schema"));
        }
        if let Some(schema) = &self.provider_configuration_schema {
            schema.collect_drift(out, &format!("{at}.provider_configuration_schema"));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Deployable,
    Plan,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Account,
    Project,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    NotInCountry,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Ai,
    Analytics,
    Auth,
    Browser,
    Cache,
    Cdn,
    Ci,
    Communications,
    Compute,
    Database,
    Domains,
    Ecommerce,
    Email,
    FeatureFlags,
    Messaging,
    Observability,
    Payments,
    Queue,
    Sandbox,
    Search,
    Storage,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Any,
    Up,
    Down,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AllowedUpdate {
    pub direction: Direction,
    pub service: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Constraint {
    Count {
        count: CountConstraint,
    },
    MutualExclusionAllowedUpdates {
        mutual_exclusion_allowed_updates: bool,
    },
    #[serde(other)]
    Unknown,
}

impl Constraint {
    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        match self {
            Self::Count { count } => push_extra(out, &format!("{at}.count"), &count.extra),
            Self::MutualExclusionAllowedUpdates { .. } => {}
            Self::Unknown => out.push(format!("{at}: unknown constraint type")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CountConstraint {
    pub at_most: i64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pricing {
    #[serde(rename = "type")]
    pub kind: PricingKind,
    #[serde(default)]
    pub paid: Option<PaidPricing>,
    #[serde(default)]
    pub paid_pricing: Vec<PaidPricingEntry>,
    #[serde(default)]
    pub component: Option<ComponentPricing>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Pricing {
    /// Keys that select a pricing tier via `--config` (the union of keys across
    /// `paid_pricing[].configuration`, e.g. `instance_type`).
    pub fn selector_keys(&self) -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        for entry in &self.paid_pricing {
            if let Some(Value::Object(map)) = &entry.configuration {
                keys.extend(map.keys().cloned());
            }
        }
        keys
    }

    /// Distinct allowed values for a tier-selector key.
    pub fn selector_values(&self, key: &str) -> Vec<Value> {
        let mut values = Vec::new();
        for entry in &self.paid_pricing {
            if let Some(Value::Object(map)) = &entry.configuration
                && let Some(value) = map.get(key)
                && !values.contains(value)
            {
                values.push(value.clone());
            }
        }
        values
    }

    /// The pricing tier selected by a `--config`: the entry whose
    /// `configuration` is fully satisfied by `config`, else the default entry.
    pub fn match_tier(&self, config: &Value) -> Option<&PaidPricingEntry> {
        let object = config.as_object();
        let mut default = None;
        for entry in &self.paid_pricing {
            if entry.is_default == Some(true) {
                default = Some(entry);
            }
            if let Some(Value::Object(tier)) = &entry.configuration {
                let satisfied = tier
                    .iter()
                    .all(|(k, v)| object.and_then(|o| o.get(k)) == Some(v));
                if satisfied && !tier.is_empty() {
                    return Some(entry);
                }
            }
        }
        default
    }

    /// Whether the selected tier requires paid confirmation.
    pub fn requires_confirmation(&self, config: &Value) -> bool {
        if !self.paid_pricing.is_empty()
            && self.paid_pricing.iter().any(|e| e.configuration.is_some())
        {
            return match self.match_tier(config) {
                Some(tier) => tier.kind != PaidKind::Free,
                None => self.kind == PricingKind::Paid,
            };
        }
        self.kind == PricingKind::Paid
    }

    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        let path = format!("{at}.pricing");
        push_extra(out, &path, &self.extra);
        push_unknown(out, &path, "type", self.kind == PricingKind::Unknown);
        if let Some(paid) = &self.paid {
            paid.collect_drift(out, &format!("{path}.paid"));
        }
        for (i, entry) in self.paid_pricing.iter().enumerate() {
            entry.collect_drift(out, &format!("{path}.paid_pricing[{i}]"));
        }
        if let Some(component) = &self.component {
            component.collect_drift(out, &format!("{path}.component"));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingKind {
    Free,
    Paid,
    Component,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaidKind {
    Free,
    Freeform,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PaidPricing {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub freeform: Option<String>,
    #[serde(rename = "type")]
    pub kind: PaidKind,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PaidPricing {
    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        push_extra(out, at, &self.extra);
        push_unknown(out, at, "type", self.kind == PaidKind::Unknown);
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PaidPricingEntry {
    #[serde(default)]
    pub configuration: Option<Value>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub freeform: Option<String>,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(rename = "type")]
    pub kind: PaidKind,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PaidPricingEntry {
    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        push_extra(out, at, &self.extra);
        push_unknown(out, at, "type", self.kind == PaidKind::Unknown);
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComponentPricing {
    #[serde(default)]
    pub options: Vec<ComponentOption>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ComponentPricing {
    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        push_extra(out, at, &self.extra);
        for (i, option) in self.options.iter().enumerate() {
            option.collect_drift(out, &format!("{at}.options[{i}]"));
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComponentOption {
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub paid: Option<PaidPricing>,
    #[serde(default)]
    pub parent_services: Vec<String>,
    #[serde(rename = "type")]
    pub kind: ComponentOptionKind,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ComponentOption {
    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        push_extra(out, at, &self.extra);
        push_unknown(out, at, "type", self.kind == ComponentOptionKind::Unknown);
        if let Some(paid) = &self.paid {
            paid.collect_drift(out, &format!("{at}.paid"));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentOptionKind {
    Free,
    Paid,
    #[serde(other)]
    Unknown,
}

/// A JSON-Schema subset describing a service's `--config`. Tri-state on the
/// wire: absent, `{}` (all defaults here), or a full object.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ConfigSchema {
    #[serde(rename = "type", default)]
    pub schema_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(rename = "additionalProperties", default)]
    pub additional_properties: Option<bool>,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, PropertySchema>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ConfigSchema {
    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        push_extra(out, at, &self.extra);
        for (name, property) in &self.properties {
            property.collect_drift(out, &format!("{at}.properties.{name}"));
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PropertySchema {
    #[serde(rename = "type")]
    pub prop_type: PropertyType,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(rename = "enum", default)]
    pub allowed: Vec<Value>,
    #[serde(rename = "minLength", default)]
    pub min_length: Option<i64>,
    #[serde(rename = "maxLength", default)]
    pub max_length: Option<i64>,
    #[serde(default)]
    pub minimum: Option<f64>,
    #[serde(default)]
    pub maximum: Option<f64>,
    #[serde(rename = "multipleOf", default)]
    pub multiple_of: Option<f64>,
    // `pattern` is modeled so it does not trip drift detection; not enforced
    // (no regex dependency, and none of the references we provision use it).
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl PropertySchema {
    /// Validate one config value against this property; returns violation
    /// details (empty when valid).
    pub fn validate_value(&self, value: &Value) -> Vec<String> {
        let mut out = Vec::new();
        let type_ok = match self.prop_type {
            PropertyType::String => value.is_string(),
            PropertyType::Integer => value.is_i64() || value.is_u64(),
            PropertyType::Number => value.is_number(),
            PropertyType::Boolean => value.is_boolean(),
            PropertyType::Unknown => true,
        };
        if !type_ok {
            out.push(format!(
                "expected {:?}, got {}",
                self.prop_type,
                value_label(value)
            ));
            // Type is wrong; further constraint checks would be noise.
            return out;
        }
        if !self.allowed.is_empty() && !self.allowed.iter().any(|candidate| candidate == value) {
            let rendered: Vec<String> = self.allowed.iter().map(value_label).collect();
            out.push(format!(
                "{} is not in enum [{}]",
                value_label(value),
                rendered.join(", ")
            ));
        }
        if let Some(text) = value.as_str() {
            let len = text.chars().count() as i64;
            if let Some(min) = self.min_length
                && len < min
            {
                out.push(format!("length {len} < minLength {min}"));
            }
            if let Some(max) = self.max_length
                && len > max
            {
                out.push(format!("length {len} > maxLength {max}"));
            }
        }
        if let Some(number) = value.as_f64() {
            if let Some(min) = self.minimum
                && number < min
            {
                out.push(format!("{number} < minimum {min}"));
            }
            if let Some(max) = self.maximum
                && number > max
            {
                out.push(format!("{number} > maximum {max}"));
            }
            if let Some(step) = self.multiple_of
                && step != 0.0
                && (number / step).fract().abs() > f64::EPSILON
            {
                out.push(format!("{number} is not a multiple of {step}"));
            }
        }
        out
    }

    fn collect_drift(&self, out: &mut Vec<String>, at: &str) {
        push_extra(out, at, &self.extra);
        push_unknown(out, at, "type", self.prop_type == PropertyType::Unknown);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PropertyType {
    String,
    Integer,
    Number,
    Boolean,
    #[serde(other)]
    Unknown,
}

fn push_extra(out: &mut Vec<String>, at: &str, extra: &BTreeMap<String, Value>) {
    for key in extra.keys() {
        out.push(format!("{at}: unmodeled field `{key}`"));
    }
}

fn push_unknown(out: &mut Vec<String>, at: &str, field: &str, is_unknown: bool) {
    if is_unknown {
        out.push(format!("{at}.{field}: unknown enum value"));
    }
}

fn value_label(value: &Value) -> String {
    match value {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
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
        svc.validate_config(
            &json!({"name": "db", "version": "17", "instance_type": "basic-256mb"}),
        )
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
}
