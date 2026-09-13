//! stackless-core: definition model, state store, and lifecycle engine.
//!
//! Substrate-agnostic by construction (ARCHITECTURE.md §8): nothing in
//! this crate names a concrete substrate; providers implement the
//! `Substrate` trait and register by name in the binary.

pub mod checkpoint;
pub mod cli_binary;
pub mod def;
pub mod durable_command;
pub mod engine;
pub mod fault;
pub mod helper_command;
pub mod lockfile;
pub mod names;
pub mod paths;
pub mod process;
pub mod routing;
pub mod security;
pub mod source_archive;
pub mod state;
pub mod substrate;
pub mod types;

pub mod capabilities;
