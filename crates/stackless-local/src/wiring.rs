//! Local wiring: origin derivation and per-instance env resolution.
//!
//! Origins are derivable from the instance name alone (§1/§3):
//! `http://{service}.{instance}.localhost:{proxy_port}`, with the
//! stack's root-origin service additionally claiming
//! `http://{instance}.localhost:{proxy_port}`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use stackless_core::def::{Namespace, StackDef};
use stackless_core::state::Checkpoint;
use stackless_core::types::{ProxyHost, TcpPort};

use crate::LocalSubstrate;
use crate::error::LocalError;

fn shell(proxy_port: TcpPort) -> LocalSubstrate {
    LocalSubstrate {
        proxy_port,
        secrets: BTreeMap::new(),
        definition_dir: PathBuf::new(),
    }
}

pub fn service_host(instance: &str, service: &str) -> ProxyHost {
    LocalSubstrate::service_host(instance, service)
}

pub fn root_host(instance: &str) -> ProxyHost {
    LocalSubstrate::root_host(instance)
}

/// The hosts a service claims on the proxy.
pub fn service_hosts(def: &StackDef, instance: &str, service: &str) -> Vec<ProxyHost> {
    shell(stackless_daemon::proxy::proxy_port()).service_hosts(def, instance, service)
}

/// The origin `${services.X.origin}` resolves to — for a root-origin
/// service that is the root form: it is what browsers use, so it is
/// what CORS allowlists and links must carry.
pub fn service_origin(def: &StackDef, instance: &str, service: &str, proxy_port: TcpPort) -> String {
    shell(proxy_port).local_service_origin(def, instance, service)
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
    shell(proxy_port).local_namespace(def, instance, prior, secrets)
}

/// Resolve a service's effective env for this substrate.
pub fn resolve_env(
    def: &StackDef,
    service: &str,
    namespace: &Namespace,
) -> Result<BTreeMap<String, String>, LocalError> {
    shell(stackless_daemon::proxy::proxy_port()).resolve_env(def, service, namespace)
}