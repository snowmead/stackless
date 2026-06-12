//! stackless-core: definition model, state store, and lifecycle engine.
//!
//! Substrate-agnostic by construction (ARCHITECTURE.md §8): nothing in
//! this crate names a concrete substrate; providers implement the
//! `Substrate` trait and register by name in the binary.

pub mod checkpoint;
pub mod def;
pub mod engine;
pub mod host;
pub mod names;
pub mod fault;
pub mod lockfile;
pub mod process;
pub mod state;
pub mod substrate;
pub mod types;
