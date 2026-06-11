//! stackless-core: definition model, state store, and lifecycle engine.
//!
//! Substrate-agnostic by construction (ARCHITECTURE.md §8): nothing in
//! this crate names a concrete substrate; providers implement the
//! `Substrate` trait and register by name in the binary.

pub mod def;
pub mod fault;
