//! Stack definition: parsing, validation, interpolation, and the
//! derived dependency graph (ARCHITECTURE.md §1).

pub mod error;
pub mod graph;
pub mod interp;
pub mod model;
pub mod parse;
pub mod placement;
pub mod validate;

pub use error::DefError;
pub use graph::{DependencyGraph, Node};
pub use interp::{Namespace, Reference};
pub use model::{
    DependencyCondition, Endpoint, EndpointSource, Health, HealthProtocol, Integration,
    ResolvedEndpoint, SecretsSpec, Service, Source, Stack, StackDef, VerifySpec, WorkloadKind,
};
