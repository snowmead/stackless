//! Shared test harness for driving the Stripe Projects layer without the real
//! `stripe` CLI. Enable via the `test-support` feature (typically as a
//! dev-dependency: `stackless-stripe-projects = { workspace = true, features =
//! ["test-support"] }`).
//!
//! [`ScriptedRunner`] is a [`CommandRunner`] that records every `stripe projects`
//! argv and replays a queue of canned [`CommandOutput`] envelopes, so a test can
//! script the exact CLI conversation a substrate or integration will have. The
//! `ok*` / `status` / `services` / `env_list` helpers build the JSON envelopes
//! the driver parses (see `responses.rs`).

// A test harness: panicking on a poisoned lock is acceptable here.
#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::ProjectsError;
use crate::stripe::{CommandOutput, CommandRunner};

/// A [`CommandRunner`] that records calls and replays a scripted output queue.
#[derive(Debug, Default)]
pub struct ScriptedRunner {
    outputs: Mutex<VecDeque<CommandOutput>>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl ScriptedRunner {
    /// Replay `outputs` in order — one per `run` call. Exhausting the queue is an
    /// error, so a test asserts the exact number of CLI calls it expected.
    pub fn new(outputs: Vec<CommandOutput>) -> Self {
        Self {
            outputs: Mutex::new(outputs.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The argv of every `stripe projects` call made so far, in order.
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl CommandRunner for ScriptedRunner {
    async fn run(&self, args: &[String], _cwd: &Path) -> Result<CommandOutput, ProjectsError> {
        self.calls.lock().unwrap().push(args.to_vec());
        self.outputs
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ProjectsError::Unavailable {
                detail: format!("ScriptedRunner exhausted at call {args:?}"),
            })
    }
}

/// A successful `{ "ok": true, "data": <data> }` envelope on stdout.
pub fn ok(data: Value) -> CommandOutput {
    CommandOutput {
        status: 0,
        stdout: json!({ "ok": true, "data": data }).to_string(),
        stderr: String::new(),
    }
}

/// A successful empty-data envelope — for writes like `add` / `remove` / `env use`.
pub fn ok_empty() -> CommandOutput {
    ok(json!({}))
}

/// `stripe projects status` data — linked to `project_id`, or unlinked if `None`.
pub fn status(project_id: Option<&str>) -> CommandOutput {
    match project_id {
        Some(id) => ok(json!({ "project": { "id": id } })),
        None => ok(json!({})),
    }
}

/// `stripe projects services list` data with the given registered service names.
pub fn services(names: &[&str]) -> CommandOutput {
    let list: Vec<Value> = names.iter().map(|n| json!({ "name": n })).collect();
    ok(json!({ "services": list }))
}

/// `stripe projects env list` data with the given environment names.
pub fn env_list(names: &[&str]) -> CommandOutput {
    let list: Vec<Value> = names.iter().map(|n| json!({ "name": n })).collect();
    ok(json!({ "environments": list }))
}

/// A raw stdout envelope (e.g. a full `catalog --json` payload passed verbatim).
pub fn raw(stdout: &str) -> CommandOutput {
    CommandOutput {
        status: 0,
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

/// A [`ScriptedRunner`] pre-loaded with the exact CLI conversation a
/// `CatalogResource` provision drives: catalog → ensure project/env → add (with
/// `add_variables` in the response) → env add → env pull. Collapses each
/// resource's provision test to one call; `add_variables` is the credential
/// envelope the resource maps to outputs.
pub fn provision_script(catalog_envelope: &str, add_variables: Value) -> ScriptedRunner {
    ScriptedRunner::new(vec![
        raw(catalog_envelope),                     // catalog --json
        status(Some("project_1")),                 // ensure_project
        env_list(&["demo"]),                       // ensure_environment (list)
        ok_empty(),                                // ensure_environment (use)
        services(&[]),                             // add_resource registered pre-check
        ok(json!({ "variables": add_variables })), // add <reference>
        ok_empty(),                                // env add
        ok_empty(),                                // env --pull --refresh
    ])
}
