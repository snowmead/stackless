use stackless_core::substrate::Observation;

/// One provider-side setting that differs from what the checkpoint records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    pub setting: String,
    pub expected: String,
    pub actual: String,
}

/// What an integration resource looks like when re-checked beyond Stripe
/// registration. Providers return empty drift until a check-time surface
/// consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationObservation {
    Present { drift: Vec<Drift> },
    Gone,
}

impl IntegrationObservation {
    /// Reduce to the substrate/engine boundary type (drift is dropped for now).
    pub fn into_substrate(self) -> Observation {
        match self {
            Self::Present { .. } => Observation::Present,
            Self::Gone => Observation::Gone,
        }
    }
}
