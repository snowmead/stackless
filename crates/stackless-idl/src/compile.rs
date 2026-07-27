//! `StackDef` → `InterfaceV1` (feature `compile`).

use stackless_core::def::StackDef;

use crate::canonical::{fingerprint_for, sha256_hex_prefixed};
use crate::error::IdlError;
use crate::model::{
    IntegrationEntry, InterfaceV1, KIND_V1, ServiceEntry, SourceMeta, TierEntry, VerifySection,
};
use crate::remap::{IdentNamespace, check_collisions, remap_dns};

#[derive(Debug)]
pub struct Compiled {
    pub idl: InterfaceV1,
    pub canonical_json: String,
}

pub fn compile_source(text: &str, known_substrates: &[&str]) -> Result<Compiled, IdlError> {
    let def = StackDef::parse(text)?;
    def.validate_hosts(known_substrates)?;
    let _graph = stackless_core::def::DependencyGraph::derive(&def)?;
    let toml_sha256 = sha256_hex_prefixed(text.as_bytes());
    let idl = compile(&def, &toml_sha256)?;
    let canonical_json = crate::canonical::canonical_json(&idl)?;
    Ok(Compiled {
        idl,
        canonical_json,
    })
}

pub fn compile(def: &StackDef, toml_sha256: &str) -> Result<InterfaceV1, IdlError> {
    let mut services = Vec::new();
    let mut service_idents = Vec::new();
    for (dns, service) in &def.services {
        let idents = remap_dns(dns)?;
        service_idents.push((dns.clone(), idents.clone()));
        services.push(ServiceEntry {
            dns: dns.clone(),
            root_origin: service.root_origin,
            idents,
        });
    }
    services.sort_by(|a, b| a.dns.cmp(&b.dns));
    check_collisions(&service_idents, IdentNamespace::Service)?;

    let (has_default, tiers) = match &def.stack.verify {
        Some(verify) => {
            let mut tiers = Vec::new();
            let mut tier_idents = Vec::new();
            for dns in verify.tiers.keys() {
                if dns == "default" {
                    return Err(IdlError::DefaultTierRejected);
                }
                let idents = remap_dns(dns)?;
                tier_idents.push((dns.clone(), idents.clone()));
                tiers.push(TierEntry {
                    dns: dns.clone(),
                    idents,
                });
            }
            tiers.sort_by(|a, b| a.dns.cmp(&b.dns));
            check_collisions(&tier_idents, IdentNamespace::Tier)?;
            (verify.run.is_some(), tiers)
        }
        None => (false, Vec::new()),
    };

    let mut integrations = Vec::new();
    let mut integration_idents = Vec::new();
    for (dns, integration) in &def.integrations {
        let idents = remap_dns(dns)?;
        integration_idents.push((dns.clone(), idents.clone()));
        integrations.push(IntegrationEntry {
            dns: dns.clone(),
            provider: integration.provider.clone(),
            idents,
        });
    }
    integrations.sort_by(|a, b| a.dns.cmp(&b.dns));
    check_collisions(&integration_idents, IdentNamespace::Integration)?;

    let mut secrets_required = def.secrets.required.clone();
    secrets_required.sort();
    secrets_required.dedup();

    let mut idl = InterfaceV1 {
        kind: KIND_V1.to_owned(),
        fingerprint: String::new(),
        source: SourceMeta {
            stack_name: def.stack.name.as_str().to_owned(),
            toml_sha256: toml_sha256.to_owned(),
        },
        services,
        verify: VerifySection { has_default, tiers },
        integrations,
        secrets_required,
    };
    idl.fingerprint = fingerprint_for(&idl)?;
    Ok(idl)
}
