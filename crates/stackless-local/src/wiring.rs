//! Local wiring: origin derivation and per-instance env resolution.
//!
//! Origins are derivable from the instance name alone (§1/§3):
//! `http://{service}.{instance}.localhost:{proxy_port}`, with the
//! stack's root-origin service additionally claiming
//! `http://{instance}.localhost:{proxy_port}`.

use std::collections::BTreeMap;

use stackless_core::def::{Namespace, StackDef};
use stackless_core::state::Checkpoint;
use stackless_core::types::{DnsName, ProxyHost, TcpPort};

use crate::error::LocalError;

pub fn service_host(instance: &str, service: &str) -> ProxyHost {
    ProxyHost::try_new(format!("{service}.{instance}.localhost"))
        .expect("service host derived from DNS-safe names")
}

pub fn root_host(instance: &str) -> ProxyHost {
    ProxyHost::try_new(format!("{instance}.localhost"))
        .expect("root host derived from DNS-safe instance name")
}

/// The hosts a service claims on the proxy.
pub fn service_hosts(def: &StackDef, instance: &str, service: &str) -> Vec<ProxyHost> {
    let mut hosts = vec![service_host(instance, service)];
    if def
        .services
        .get(service)
        .is_some_and(|spec| spec.root_origin)
    {
        hosts.push(root_host(instance));
    }
    hosts
}

/// The origin `${services.X.origin}` resolves to — for a root-origin
/// service that is the root form: it is what browsers use, so it is
/// what CORS allowlists and links must carry.
pub fn service_origin(def: &StackDef, instance: &str, service: &str, proxy_port: TcpPort) -> String {
    let host = if def
        .services
        .get(service)
        .is_some_and(|spec| spec.root_origin)
    {
        root_host(instance)
    } else {
        service_host(instance, service)
    };
    format!("http://{}:{}", host, proxy_port.get())
}

/// Build the interpolation namespace for one instance from the
/// definition and the journal so far. Datastore URLs come from
/// provision checkpoints; referencing one that is not provisioned yet
/// is an engine-ordering bug surfaced as an error, never a guess.
pub fn namespace(
    def: &StackDef,
    instance: &str,
    proxy_port: TcpPort,
    prior: &[Checkpoint],
    secrets: &BTreeMap<String, String>,
) -> Namespace {
    let mut namespace = Namespace {
        stack_name: def.stack.name.clone(),
        instance_name: DnsName::try_new(instance).expect("instance name validated at creation"),
        ..Namespace::default()
    };
    for service in def.services.keys() {
        namespace.service_origins.insert(
            service.clone(),
            service_origin(def, instance, service, proxy_port),
        );
    }
    for checkpoint in prior {
        if let Some(name) = checkpoint.step_id.strip_prefix("provision:")
            && let Ok(payload) = serde_json::from_str::<serde_json::Value>(&checkpoint.payload)
            && let Some(url) = payload.get("url").and_then(|v| v.as_str())
        {
            namespace
                .datastore_urls
                .insert(name.to_owned(), url.to_owned());
        }
    }
    namespace.secrets = secrets.clone();
    namespace.add_integration_checkpoints(prior);
    namespace
}

/// Resolve a service's effective env for this substrate.
pub fn resolve_env(
    def: &StackDef,
    service: &str,
    namespace: &Namespace,
) -> Result<BTreeMap<String, String>, LocalError> {
    let Some(spec) = def.services.get(service) else {
        return Ok(BTreeMap::new());
    };
    let raw = spec
        .effective_env(service, crate::SUBSTRATE_NAME)
        .map_err(|err| LocalError::EnvResolve {
            service: service.to_owned(),
            reference: "env".into(),
            detail: err.to_string(),
        })?;
    let mut resolved = BTreeMap::new();
    for (key, value) in &raw {
        let location = format!("services.{service}.env.{key}");
        let value =
            stackless_core::def::interp::resolve(value, namespace, &location).map_err(|err| {
                LocalError::EnvResolve {
                    service: service.to_owned(),
                    reference: format!("${{{key}}}"),
                    detail: err.to_string(),
                }
            })?;
        resolved.insert(key.clone(), value);
    }
    // Same-named secret injection (§1).
    for key in &spec.secrets {
        if let Some(value) = namespace.secrets.get(key) {
            resolved.insert(key.clone(), value.clone());
        }
    }
    Ok(resolved)
}