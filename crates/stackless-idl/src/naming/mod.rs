//! Emit-time language naming over DNS wire names.

pub mod go;
mod parts;
pub mod python;
pub mod rust;
pub mod typescript;

pub use parts::Parts;

pub(crate) use parts::capitalize_first;

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
