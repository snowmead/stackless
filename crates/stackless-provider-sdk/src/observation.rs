use stackless_core::substrate::Observation;

pub use stackless_core::substrate::SettingDrift as Drift;

/// What an integration resource looks like when re-checked beyond Stripe
/// registration. Providers return empty drift until a check-time surface
/// consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationObservation {
    Present { drift: Vec<Drift> },
    Gone,
}

impl IntegrationObservation {
    /// Preserve configuration drift at the engine boundary.
    pub fn into_substrate(self) -> Observation {
        match self {
            Self::Present { drift } if drift.is_empty() => Observation::Present,
            Self::Present { drift } => Observation::Drifted { settings: drift },
            Self::Gone => Observation::Gone,
        }
    }
}
