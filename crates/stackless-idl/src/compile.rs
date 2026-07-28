//! `StackDef` → `InterfaceV1` (feature `compile`).

use stackless_core::def::StackDef;

use crate::canonical::{fingerprint_for, sha256_hex_prefixed};
use crate::error::IdlError;
use crate::model::{
    BodyV1, IntegrationEntry, InterfaceV1, KIND_V1, ServiceEntry, SourceMeta, TierEntry,
    VerifySection,
};
use crate::naming::Parts;

#[derive(Debug)]
pub struct Compiled {
    pub idl: InterfaceV1,
    pub pretty_json: String,
}

pub fn compile_source(text: &str, known_substrates: &[&str]) -> Result<Compiled, IdlError> {
    let def = StackDef::parse(text)?;
    def.validate_hosts(known_substrates)?;
    let _graph = stackless_core::def::DependencyGraph::derive(&def)?;
    let toml_sha256 = sha256_hex_prefixed(text.as_bytes());
    let idl = compile(&def, &toml_sha256)?;
    let pretty_json = crate::canonical::pretty_json(&idl)?;
    Ok(Compiled { idl, pretty_json })
}

pub fn compile(def: &StackDef, toml_sha256: &str) -> Result<InterfaceV1, IdlError> {
    let mut services = Vec::new();
    for (dns, service) in &def.services {
        // Reject reserved wire names early so compile fails before emit.
        Parts::from_dns(dns)?;
        services.push(ServiceEntry {
            dns: dns.clone(),
            root_origin: service.root_origin,
        });
    }
    services.sort_by(|a, b| a.dns.cmp(&b.dns));

    let (has_default, tiers) = match &def.stack.verify {
        Some(verify) => {
            let mut tiers = Vec::new();
            for dns in verify.tiers.keys() {
                if dns == "default" {
                    return Err(IdlError::DefaultTierRejected);
                }
                Parts::from_dns(dns)?;
                tiers.push(TierEntry { dns: dns.clone() });
            }
            tiers.sort_by(|a, b| a.dns.cmp(&b.dns));
            (verify.run.is_some(), tiers)
        }
        None => (false, Vec::new()),
    };

    let mut integrations = Vec::new();
    for (dns, integration) in &def.integrations {
        Parts::from_dns(dns)?;
        integrations.push(IntegrationEntry {
            dns: dns.clone(),
            provider: integration.provider.clone(),
        });
    }
    integrations.sort_by(|a, b| a.dns.cmp(&b.dns));

    let mut secrets_required = def.secrets.required.clone();
    secrets_required.sort();
    secrets_required.dedup();

    let mut idl = InterfaceV1 {
        kind: KIND_V1.to_owned(),
        fingerprint: String::new(),
        body: BodyV1 {
            source: SourceMeta {
                stack_name: def.stack.name.as_str().to_owned(),
                toml_sha256: toml_sha256.to_owned(),
            },
            services,
            verify: VerifySection { has_default, tiers },
            integrations,
            secrets_required,
        },
    };
    idl.fingerprint = fingerprint_for(&idl)?;
    Ok(idl)
}
