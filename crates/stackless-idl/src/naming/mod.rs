//! Emit-time language naming over DNS wire names.

pub mod go;
mod parts;
pub mod python;
pub mod rust;
pub mod typescript;

pub use parts::Parts;

pub(crate) use parts::capitalize_first;

use crate::error::IdlError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentNamespace {
    Service,
    Tier,
    Integration,
}

impl IdentNamespace {
    pub fn label(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Tier => "tier",
            Self::Integration => "integration",
        }
    }
}

/// Shared DNS → names → collision-check seam used by every emitter.
pub(crate) fn named_entries<'a, N, F, C>(
    dns_names: impl Iterator<Item = &'a str>,
    namespace: IdentNamespace,
    mut from_dns: F,
    check: C,
) -> Result<Vec<(String, N)>, IdlError>
where
    F: FnMut(&str) -> Result<N, IdlError>,
    C: FnOnce(&[(String, N)], IdentNamespace) -> Result<(), IdlError>,
{
    let mut entries = Vec::new();
    for dns in dns_names {
        entries.push((dns.to_owned(), from_dns(dns)?));
    }
    check(&entries, namespace)?;
    Ok(entries)
}
