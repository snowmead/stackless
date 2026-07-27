//! Stackless library: sync [`Client`] lifecycle API plus the CLI entrypoint
//! under [`cli`].
//!
//! # Quick start
//!
//! ```no_run
//! use stackless::{Client, Create, UpRequest};
//!
//! let client = Client::system()?;
//! let created = client.up(UpRequest::Create(
//!     Create::new("stackless.toml", "local").named("demo"),
//! ))?;
//! println!("{}", created.origin("web")?);
//! client.down(&created.name)?;
//! # Ok::<(), stackless::Error>(())
//! ```
//!
//! For hermetic tests, enable feature `test-support` and use
//! [`test_support::TestContext`].

pub mod client;
pub mod error;

pub mod cli;

pub(crate) mod adopt;
pub(crate) mod authoring;
pub(crate) mod bind_cmd;
pub(crate) mod commands;
pub(crate) mod daemon_cmd;
pub(crate) mod doctor;
pub(crate) mod init;
pub(crate) mod mcp;
pub(crate) mod output;
pub(crate) mod secrets;
pub(crate) mod substrates;
pub(crate) mod verify;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use client::{
    CheckOutcome, Client, ClientBuilder, Create, DownOutcome, DownStatus, InstanceReport, LogEntry,
    LogsOutcome, PaidConsent, Resume, UpOutcome, UpRequest, VerifyOutcome,
};
pub use error::{Error, ErrorCode};
pub use stackless_core::paths::Paths;

#[cfg(feature = "test-support")]
pub use test_support::{Environment, GuardPolicy, TestContext};
