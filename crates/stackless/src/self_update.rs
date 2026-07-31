//! CLI self-update via axoupdater + install receipts.
//!
//! Soft-fails on network/receipt problems so ordinary verbs keep working.
//! Explicit `stackless update` surfaces those failures.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axoupdater::AxoUpdater;
use serde::{Deserialize, Serialize};
use stackless_core::lockfile::{FileLock, LockError};
use stackless_core::paths::Paths;

const APP_NAME: &str = "stackless";
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

const ENV_NO_SELF_UPDATE: &str = "STACKLESS_NO_SELF_UPDATE";
pub(crate) const ENV_FORCE_SELF_UPDATE: &str = "STACKLESS_FORCE_SELF_UPDATE";
const ENV_JUST_UPDATED: &str = "STACKLESS_JUST_UPDATED";
const ENV_GITHUB_TOKEN: &str = "STACKLESS_GITHUB_TOKEN";
pub(crate) const ENV_SELF_UPDATE_VERBOSE: &str = "STACKLESS_SELF_UPDATE_VERBOSE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSkip {
    EnvDisabled,
    NoReceipt,
    InternalCommand,
    ReaperChild,
    AlreadyApplied,
    Throttled,
    LockBusy,
    ReceiptNotThisExecutable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Skipped(UpdateSkip),
    Current {
        version: String,
    },
    Updated {
        from: String,
        to: String,
        exe: PathBuf,
    },
    SoftFailed {
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    User,
    Internal,
}

#[derive(Debug)]
pub struct UpdateContext {
    pub command_kind: CommandKind,
    pub force: bool,
    pub state_dir: Paths,
    pub has_state_dir_flag: bool,
    pub has_proxy_port_flag: bool,
}

/// Pure skip matrix inputs (no I/O). Tests drive this directly.
#[derive(Debug, Clone)]
pub struct SkipInputs {
    pub just_updated: bool,
    pub env_disabled: bool,
    pub command_kind: CommandKind,
    pub has_state_dir_flag: bool,
    pub has_proxy_port_flag: bool,
    pub force: bool,
    pub last_checked_at: Option<SystemTime>,
    pub now: SystemTime,
}

/// Ordered guards before any network call. Returns `None` when an update check
/// should proceed.
pub fn evaluate_skip(inputs: &SkipInputs) -> Option<UpdateSkip> {
    if inputs.just_updated {
        return Some(UpdateSkip::AlreadyApplied);
    }
    if inputs.env_disabled {
        return Some(UpdateSkip::EnvDisabled);
    }
    if inputs.command_kind == CommandKind::Internal {
        return Some(UpdateSkip::InternalCommand);
    }
    if inputs.has_state_dir_flag || inputs.has_proxy_port_flag {
        return Some(UpdateSkip::ReaperChild);
    }
    if !inputs.force
        && let Some(last) = inputs.last_checked_at
        && inputs
            .now
            .duration_since(last)
            .is_ok_and(|elapsed| elapsed < TTL)
    {
        return Some(UpdateSkip::Throttled);
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct SelfUpdateLedger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_checked_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_current_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_failed_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_applied_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

/// Check GitHub Releases and install a newer binary when the install receipt
/// says this executable is a cargo-dist install. Never panics.
pub fn maybe_self_update(ctx: UpdateContext) -> UpdateOutcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        maybe_self_update_inner(ctx)
    })) {
        Ok(outcome) => outcome,
        Err(_) => UpdateOutcome::SoftFailed {
            detail: "self-update panicked".into(),
        },
    }
}

fn maybe_self_update_inner(ctx: UpdateContext) -> UpdateOutcome {
    let force = ctx.force || env_truthy(ENV_FORCE_SELF_UPDATE);
    let ledger_path = ctx.state_dir.self_update_ledger();
    let ledger = read_ledger(&ledger_path);
    let now = SystemTime::now();

    let inputs = SkipInputs {
        just_updated: env_truthy(ENV_JUST_UPDATED),
        env_disabled: env_truthy(ENV_NO_SELF_UPDATE),
        command_kind: ctx.command_kind,
        has_state_dir_flag: ctx.has_state_dir_flag,
        has_proxy_port_flag: ctx.has_proxy_port_flag,
        force,
        last_checked_at: ledger
            .last_checked_unix
            .and_then(|secs| UNIX_EPOCH.checked_add(Duration::from_secs(secs))),
        now,
    };
    if let Some(skip) = evaluate_skip(&inputs) {
        return UpdateOutcome::Skipped(skip);
    }

    let lock = match FileLock::try_acquire(&ctx.state_dir.self_update_lock()) {
        Ok(lock) => lock,
        Err(LockError::Held { .. }) => return UpdateOutcome::Skipped(UpdateSkip::LockBusy),
        Err(err) => {
            return UpdateOutcome::SoftFailed {
                detail: err.to_string(),
            };
        }
    };

    let outcome = run_axoupdater();
    // Only stamp the throttle ledger after a real network check attempt.
    // NoReceipt / ReceiptNotThisExecutable must not start the 24h TTL for
    // cargo-built or foreign binaries.
    if matches!(
        outcome,
        UpdateOutcome::Current { .. }
            | UpdateOutcome::Updated { .. }
            | UpdateOutcome::SoftFailed { .. }
    ) {
        write_ledger_for_outcome(&ledger_path, &ledger, now, &outcome);
    }
    drop(lock);
    outcome
}

fn run_axoupdater() -> UpdateOutcome {
    let mut updater = AxoUpdater::new_for(APP_NAME);
    updater.disable_installer_output();
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return UpdateOutcome::SoftFailed {
                detail: err.to_string(),
            };
        }
    };
    updater.set_client(client);
    if let Ok(token) = std::env::var(ENV_GITHUB_TOKEN)
        && !token.is_empty()
    {
        updater.set_github_token(&token);
    }

    if updater.load_receipt().is_err() {
        return UpdateOutcome::Skipped(UpdateSkip::NoReceipt);
    }

    match updater.check_receipt_is_for_this_executable() {
        Ok(true) => {}
        Ok(false) => {
            return UpdateOutcome::Skipped(UpdateSkip::ReceiptNotThisExecutable);
        }
        Err(err) => {
            return UpdateOutcome::SoftFailed {
                detail: err.to_string(),
            };
        }
    }

    let needed = match updater.is_update_needed_sync() {
        Ok(needed) => needed,
        Err(err) => {
            return UpdateOutcome::SoftFailed {
                detail: err.to_string(),
            };
        }
    };
    if !needed {
        return UpdateOutcome::Current {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        };
    }

    // Drain before the installer mv's over the binary so a live daemon does
    // not hold a deleted inode (Linux current_exe → "stackless (deleted)").
    drain_daemon_best_effort();

    match updater.run_sync() {
        Ok(Some(result)) => {
            let from = result
                .old_version
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
            let to = result.new_version.to_string();
            let prefix = updater
                .install_prefix_root()
                .map(|p| PathBuf::from(p.as_std_path()))
                .unwrap_or_else(|_| PathBuf::from(result.install_prefix.as_std_path()));
            let exe = resolve_updated_exe(&prefix);
            UpdateOutcome::Updated { from, to, exe }
        }
        Ok(None) => {
            // Installer decided nothing to do after we drained; bring the
            // operator daemon back so lease reaping does not stay offline.
            respawn_daemon_best_effort();
            UpdateOutcome::Current {
                version: env!("CARGO_PKG_VERSION").to_owned(),
            }
        }
        Err(err) => {
            respawn_daemon_best_effort();
            UpdateOutcome::SoftFailed {
                detail: err.to_string(),
            }
        }
    }
}

fn resolve_updated_exe(prefix: &Path) -> PathBuf {
    let bin = prefix.join("bin").join(APP_NAME);
    if bin.exists() {
        bin
    } else {
        prefix.join(APP_NAME)
    }
}

/// Best-effort drain of the resident operator daemon. Called before the
/// installer replaces the binary (and again before re-exec). Never fails
/// the update if nothing is listening.
pub fn drain_daemon_best_effort() {
    use stackless_daemon::DaemonClient;
    use stackless_daemon::rpc::Request;

    if let Ok(mut client) = DaemonClient::connect_with(&Paths::from_env()) {
        let _ = client.call(Request::Shutdown);
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Best-effort respawn after a pre-install drain when the install itself
/// did not apply (failed or no-op). Never fails the update.
fn respawn_daemon_best_effort() {
    use stackless_daemon::DaemonClient;

    let _ = DaemonClient::ensure();
}

/// Replace this process with the updated binary, forwarding argv[1..].
pub fn reexec(exe: &Path) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new(exe)
            .args(std::env::args().skip(1))
            .env(ENV_JUST_UPDATED, "1")
            .exec();
        eprintln!("stackless: failed to re-exec {}: {err}", exe.display());
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        let status = Command::new(exe)
            .args(std::env::args().skip(1))
            .env(ENV_JUST_UPDATED, "1")
            .status();
        match status {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => {
                eprintln!("stackless: failed to re-exec {}: {err}", exe.display());
                std::process::exit(1);
            }
        }
    }
}

pub(crate) fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn read_ledger(path: &Path) -> SelfUpdateLedger {
    let Ok(bytes) = std::fs::read(path) else {
        return SelfUpdateLedger::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn write_ledger_for_outcome(
    path: &Path,
    previous: &SelfUpdateLedger,
    now: SystemTime,
    outcome: &UpdateOutcome,
) {
    let unix = system_time_unix(now);
    let mut next = previous.clone();
    next.last_checked_unix = Some(unix);
    match outcome {
        UpdateOutcome::Current { .. } => {
            next.last_current_unix = Some(unix);
            next.last_error = None;
        }
        UpdateOutcome::Updated { from, to, .. } => {
            next.last_applied_unix = Some(unix);
            next.last_from = Some(from.clone());
            next.last_to = Some(to.clone());
            next.last_error = None;
        }
        UpdateOutcome::SoftFailed { detail } => {
            next.last_failed_unix = Some(unix);
            next.last_error = Some(detail.clone());
        }
        UpdateOutcome::Skipped(_) => return,
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&next) {
        let _ = std::fs::write(path, bytes);
    }
}

fn system_time_unix(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_inputs(dir: &tempfile::TempDir) -> (Paths, SkipInputs) {
        let paths = Paths::new(dir.path());
        let inputs = SkipInputs {
            just_updated: false,
            env_disabled: false,
            command_kind: CommandKind::User,
            has_state_dir_flag: false,
            has_proxy_port_flag: false,
            force: false,
            last_checked_at: None,
            now: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        };
        (paths, inputs)
    }

    #[test]
    fn skip_kill_switch() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.env_disabled = true;
        assert_eq!(evaluate_skip(&inputs), Some(UpdateSkip::EnvDisabled));
    }

    #[test]
    fn skip_daemon_and_mcp() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.command_kind = CommandKind::Internal;
        assert_eq!(evaluate_skip(&inputs), Some(UpdateSkip::InternalCommand));
    }

    #[test]
    fn skip_reaper_flags() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.has_state_dir_flag = true;
        assert_eq!(evaluate_skip(&inputs), Some(UpdateSkip::ReaperChild));
        inputs.has_state_dir_flag = false;
        inputs.has_proxy_port_flag = true;
        assert_eq!(evaluate_skip(&inputs), Some(UpdateSkip::ReaperChild));
    }

    #[test]
    fn skip_throttle_unless_force() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.last_checked_at = Some(inputs.now - Duration::from_secs(60));
        assert_eq!(evaluate_skip(&inputs), Some(UpdateSkip::Throttled));
        inputs.force = true;
        assert_eq!(evaluate_skip(&inputs), None);
    }

    #[test]
    fn skip_already_applied() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.just_updated = true;
        assert_eq!(evaluate_skip(&inputs), Some(UpdateSkip::AlreadyApplied));
    }

    #[test]
    fn skip_order_already_applied_before_kill_switch() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.just_updated = true;
        inputs.env_disabled = true;
        assert_eq!(evaluate_skip(&inputs), Some(UpdateSkip::AlreadyApplied));
    }

    #[test]
    fn throttle_expired_allows_check() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.last_checked_at = Some(inputs.now - TTL - Duration::from_secs(1));
        assert_eq!(evaluate_skip(&inputs), None);
    }

    #[test]
    fn throttle_future_last_checked_allows_check() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut inputs) = base_inputs(&dir);
        inputs.last_checked_at = Some(inputs.now + Duration::from_secs(60));
        assert_eq!(evaluate_skip(&inputs), None);
    }

    #[test]
    fn maybe_self_update_respects_internal_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let outcome = maybe_self_update(UpdateContext {
            command_kind: CommandKind::Internal,
            force: true,
            state_dir: paths,
            has_state_dir_flag: false,
            has_proxy_port_flag: false,
        });
        assert_eq!(outcome, UpdateOutcome::Skipped(UpdateSkip::InternalCommand));
    }

    #[test]
    fn resolve_exe_prefers_bin_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe = bin_dir.join(APP_NAME);
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(resolve_updated_exe(dir.path()), exe);
    }

    #[test]
    fn resolve_exe_falls_back_to_prefix() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(resolve_updated_exe(dir.path()), dir.path().join(APP_NAME));
    }
}
