//! The agent-facing error contract (ARCHITECTURE.md §2).
//!
//! Every error stackless emits carries three parts: *what* failed (the
//! step and instance), *why* (the observed cause), and *how to proceed*
//! (a concrete command, flag, or fix). Agents branch on stable codes,
//! never on prose.

use serde::Serialize;

/// Stable machine-readable error codes.
///
/// Codes are versioned API surface: renaming one is a breaking change
/// (ARCHITECTURE.md §2). Every code lives here, in one registry, so the
/// full surface is greppable and uniqueness is testable.
pub mod codes {
    pub const DEF_PARSE_SYNTAX: &str = "def.parse.syntax";
    pub const DEF_PARSE_SCHEMA: &str = "def.parse.schema";
    pub const DEF_NAME_INVALID: &str = "def.validate.name_invalid";
    pub const DEF_NO_SERVICES: &str = "def.validate.no_services";
    pub const DEF_UNKNOWN_KEY: &str = "def.validate.unknown_key";
    pub const DEF_DEPENDS_ON_REJECTED: &str = "def.validate.depends_on_rejected";
    pub const DEF_SUBSTRATE_BLOCK_INVALID: &str = "def.validate.substrate_block_invalid";
    pub const DEF_SUBSTRATE_CONFIG_MISSING: &str = "def.validate.substrate_config_missing";
    pub const DEF_ENGINE_UNKNOWN: &str = "def.validate.engine_unknown";
    pub const DEF_ROOT_ORIGIN_CONFLICT: &str = "def.validate.root_origin_conflict";
    pub const DEF_REFERENCE_SYNTAX: &str = "def.validate.reference_syntax";
    pub const DEF_UNDECLARED_REFERENCE: &str = "def.validate.undeclared_reference";
    pub const DEF_SECRET_NOT_REQUIRED: &str = "def.validate.secret_not_required";
    pub const DEF_WIRING_CYCLE: &str = "def.validate.wiring_cycle";
    pub const DEF_ENV_NOT_STRINGS: &str = "def.validate.env_not_strings";
    pub const CLI_FILE_READ: &str = "cli.file.read";
    pub const CLI_SUBSTRATE_UNKNOWN: &str = "cli.substrate.unknown";
    pub const STATE_OPEN: &str = "state.open_failed";
    pub const STATE_MIGRATE: &str = "state.migrate_failed";
    pub const STATE_QUERY: &str = "state.query_failed";
    pub const STATE_INSTANCE_EXISTS: &str = "state.instance.exists";
    pub const STATE_INSTANCE_NOT_FOUND: &str = "state.instance.not_found";
    pub const STATE_LOCK_HELD: &str = "state.lock.held";
    pub const STATE_GC_FAILED: &str = "state.gc_failed";
    pub const STATE_REMOTE_OPEN: &str = "state.remote.open_failed";
    pub const STATE_REMOTE_QUERY: &str = "state.remote.query_failed";
    pub const STATE_REMOTE_RUNTIME: &str = "state.remote.runtime_failed";
    pub const STATE_REMOTE_WORKER: &str = "state.remote.worker_gone";
    pub const STATE_ROW_DECODE: &str = "state.row.decode_failed";
    pub const ENGINE_SUBSTRATE_MISMATCH: &str = "engine.substrate.mismatch";
    pub const ENGINE_SOURCE_OVERRIDE_UNSUPPORTED: &str = "engine.source_override.unsupported";
    pub const ENGINE_STEP_FAILED: &str = "engine.step.failed";
    pub const ENGINE_TEARDOWN_SURVIVORS: &str = "engine.teardown.survivors";
    pub const DAEMON_UNREACHABLE: &str = "daemon.unreachable";
    pub const DAEMON_REQUEST_FAILED: &str = "daemon.request_failed";
    pub const DAEMON_SPAWN_FAILED: &str = "daemon.spawn_failed";
    pub const CLI_RUNTIME: &str = "cli.runtime";
    pub const LOCAL_MATERIALIZE_UNAVAILABLE: &str = "local.materialize.unavailable";
    pub const LOCAL_SOURCE_PATH_INVALID: &str = "local.source_path.invalid";
    pub const LOCAL_CONFIG_INVALID: &str = "local.config.invalid";
    pub const LOCAL_PORT_ALLOC: &str = "local.port.alloc_failed";
    pub const LOCAL_LOG_FILE: &str = "local.log_file";
    pub const LOCAL_SPAWN_FAILED: &str = "local.spawn_failed";
    pub const LOCAL_HOOK_FAILED: &str = "local.hook_failed";
    pub const LOCAL_HEALTH_FAILED: &str = "local.health_failed";
    pub const LOCAL_SERVICE_DIED: &str = "local.service_died";
    pub const LOCAL_ENV_RESOLVE: &str = "local.env_resolve";
    pub const LOCAL_KILL_FAILED: &str = "local.kill_failed";
    pub const LOCAL_GIT_CLONE_FAILED: &str = "local.git.clone_failed";
    pub const LOCAL_GIT_FETCH_FAILED: &str = "local.git.fetch_failed";
    pub const LOCAL_GIT_REF_NOT_FOUND: &str = "local.git.ref_not_found";
    pub const LOCAL_GIT_CHECKOUT_FAILED: &str = "local.git.checkout_failed";
    pub const CLI_BAD_ARGUMENT: &str = "cli.bad_argument";
    pub const LOCAL_DOCKER_ENGINE: &str = "local.docker.engine";
    pub const LOCAL_DATASTORE_FAILED: &str = "local.datastore.failed";
    pub const LOCAL_DATASTORE_NOT_READY: &str = "local.datastore.not_ready";
    pub const SECRETS_UNRESOLVED: &str = "secrets.unresolved";
    pub const VERIFY_FAILED: &str = "verify.failed";
    pub const VERIFY_NOT_DECLARED: &str = "verify.not_declared";
    pub const RENDER_CONFIG_INVALID: &str = "render.config.invalid";
    pub const RENDER_API_KEY_MISSING: &str = "render.api_key.missing";
    pub const RENDER_API_FAILED: &str = "render.api.failed";
    pub const RENDER_STRIPE_UNAVAILABLE: &str = "render.stripe.unavailable";
    pub const RENDER_STRIPE_AUTH: &str = "render.stripe.auth";
    pub const RENDER_STRIPE_FAILED: &str = "render.stripe.failed";
    pub const RENDER_PROJECT_ANCHOR: &str = "render.project.anchor";
    pub const RENDER_PAYMENT_NOT_CONFIRMED: &str = "render.payment.not_confirmed";
    pub const RENDER_PROVISION_FAILED: &str = "render.provision.failed";
    pub const RENDER_DEPLOY_FAILED: &str = "render.deploy.failed";
    pub const RENDER_DEPLOY_TIMEOUT: &str = "render.deploy.timeout";
    pub const RENDER_HEALTH_FAILED: &str = "render.health.failed";
    pub const RENDER_PREPARE_FAILED: &str = "render.prepare.failed";
    pub const RENDER_TEARDOWN_SURVIVOR: &str = "render.teardown.survivor";

    /// Every code in the registry, for uniqueness tests.
    pub const ALL: &[&str] = &[
        DEF_PARSE_SYNTAX,
        DEF_PARSE_SCHEMA,
        DEF_NAME_INVALID,
        DEF_NO_SERVICES,
        DEF_UNKNOWN_KEY,
        DEF_DEPENDS_ON_REJECTED,
        DEF_SUBSTRATE_BLOCK_INVALID,
        DEF_SUBSTRATE_CONFIG_MISSING,
        DEF_ENGINE_UNKNOWN,
        DEF_ROOT_ORIGIN_CONFLICT,
        DEF_REFERENCE_SYNTAX,
        DEF_UNDECLARED_REFERENCE,
        DEF_SECRET_NOT_REQUIRED,
        DEF_WIRING_CYCLE,
        DEF_ENV_NOT_STRINGS,
        CLI_FILE_READ,
        CLI_SUBSTRATE_UNKNOWN,
        STATE_OPEN,
        STATE_MIGRATE,
        STATE_QUERY,
        STATE_INSTANCE_EXISTS,
        STATE_INSTANCE_NOT_FOUND,
        STATE_LOCK_HELD,
        STATE_GC_FAILED,
        STATE_REMOTE_OPEN,
        STATE_REMOTE_QUERY,
        STATE_REMOTE_RUNTIME,
        STATE_REMOTE_WORKER,
        STATE_ROW_DECODE,
        ENGINE_SUBSTRATE_MISMATCH,
        ENGINE_SOURCE_OVERRIDE_UNSUPPORTED,
        ENGINE_STEP_FAILED,
        ENGINE_TEARDOWN_SURVIVORS,
        DAEMON_UNREACHABLE,
        DAEMON_REQUEST_FAILED,
        DAEMON_SPAWN_FAILED,
        CLI_RUNTIME,
        LOCAL_MATERIALIZE_UNAVAILABLE,
        LOCAL_SOURCE_PATH_INVALID,
        LOCAL_CONFIG_INVALID,
        LOCAL_PORT_ALLOC,
        LOCAL_LOG_FILE,
        LOCAL_SPAWN_FAILED,
        LOCAL_HOOK_FAILED,
        LOCAL_HEALTH_FAILED,
        LOCAL_SERVICE_DIED,
        LOCAL_ENV_RESOLVE,
        LOCAL_KILL_FAILED,
        LOCAL_GIT_CLONE_FAILED,
        LOCAL_GIT_FETCH_FAILED,
        LOCAL_GIT_REF_NOT_FOUND,
        LOCAL_GIT_CHECKOUT_FAILED,
        CLI_BAD_ARGUMENT,
        LOCAL_DOCKER_ENGINE,
        LOCAL_DATASTORE_FAILED,
        LOCAL_DATASTORE_NOT_READY,
        SECRETS_UNRESOLVED,
        VERIFY_FAILED,
        VERIFY_NOT_DECLARED,
        RENDER_CONFIG_INVALID,
        RENDER_API_KEY_MISSING,
        RENDER_API_FAILED,
        RENDER_STRIPE_UNAVAILABLE,
        RENDER_STRIPE_AUTH,
        RENDER_STRIPE_FAILED,
        RENDER_PROJECT_ANCHOR,
        RENDER_PAYMENT_NOT_CONFIRMED,
        RENDER_PROVISION_FAILED,
        RENDER_DEPLOY_FAILED,
        RENDER_DEPLOY_TIMEOUT,
        RENDER_HEALTH_FAILED,
        RENDER_PREPARE_FAILED,
        RENDER_TEARDOWN_SURVIVOR,
    ];
}

/// Implemented by every error enum in every stackless crate.
///
/// A new error variant is only complete when its remediation text says
/// what the operator should actually do (ARCHITECTURE.md §8).
pub trait Fault: std::error::Error {
    /// The stable machine-readable code from [`codes`].
    fn code(&self) -> &'static str;
    /// How to proceed: a concrete command, flag, or fix.
    fn remediation(&self) -> String;
    /// The lifecycle step that failed, when one was executing.
    fn step(&self) -> Option<&str> {
        None
    }
    /// The instance the failure belongs to, when there is one.
    fn instance(&self) -> Option<&str> {
        None
    }
}

/// The serialized error shape agents consume in `--json` mode.
#[derive(Debug, Serialize)]
pub struct Report {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    pub remediation: String,
}

impl Report {
    pub fn from_fault(fault: &dyn Fault) -> Self {
        Self {
            code: fault.code(),
            message: fault.to_string(),
            step: fault.step().map(str::to_owned),
            instance: fault.instance().map(str::to_owned),
            remediation: fault.remediation(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::codes;
    use std::collections::BTreeSet;

    #[test]
    fn codes_are_unique() {
        let set: BTreeSet<_> = codes::ALL.iter().collect();
        assert_eq!(set.len(), codes::ALL.len());
    }
}
