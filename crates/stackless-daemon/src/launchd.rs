//! Boot persistence on macOS (ARCHITECTURE.md §3): register the daemon
//! as a launchd user agent so the lease is a system guarantee across
//! reboots and crashes. If registration is refused, stackless degrades
//! loudly — `status`/`list` read the reason from `daemon.persistence`
//! and warn that leases hold only while the daemon happens to run.
//!
//! KeepAlive is `{ SuccessfulExit = false }`: launchd restarts a crash
//! but honors a clean drain, so `daemon stop` and the upgrade handshake
//! (§3) still take the daemon down for good.

use std::path::PathBuf;
use std::process::Command;

use stackless_core::state::Store;

pub const LABEL: &str = "dev.stackless.daemon";

/// The one-line file `status`/`list` read: "registered" on success, the
/// failure reason otherwise.
pub fn persistence_status_path() -> PathBuf {
    Store::state_dir().join("daemon.persistence")
}

fn plist_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).map(|home| {
        home.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    })
}

/// Hand-written plist (a crate would be overkill for four keys). The
/// binary path is the current exe — if the plist names a different one
/// (the binary moved), it is rewritten.
fn plist_xml(exe: &str) -> String {
    let exe = xml_escape(exe);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>daemon</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
</dict>
</plist>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Ensure the LaunchAgent exists and is bootstrapped. Records the
/// outcome to `daemon.persistence` and never fails the daemon: a refused
/// registration degrades loudly rather than aborting (§3).
pub fn ensure_registered() {
    let outcome = register();
    let text = match &outcome {
        Ok(()) => "registered".to_owned(),
        Err(why) => why.clone(),
    };
    if let Some(dir) = persistence_status_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(persistence_status_path(), text);
}

/// The actual registration work, returning a human reason on failure.
fn register() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .map_err(|err| format!("cannot resolve the stackless binary path: {err}"))?;
    let plist =
        plist_path().ok_or_else(|| "HOME is unset; cannot locate LaunchAgents".to_owned())?;
    if let Some(dir) = plist.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    }

    // Rewrite only when the content differs — a moved binary or a first
    // install. Comparing avoids churning the file (and re-bootstrap) on
    // every start.
    let want = plist_xml(&exe);
    let current = std::fs::read_to_string(&plist).ok();
    if current.as_deref() != Some(want.as_str()) {
        std::fs::write(&plist, &want)
            .map_err(|err| format!("cannot write {}: {err}", plist.display()))?;
    }

    let uid = nix_getuid();
    let domain = format!("gui/{uid}");
    // Already bootstrapped is success — that is the steady state, and
    // re-bootstrapping a loaded service is what launchd rejects (with an
    // unhelpful "5: Input/output error", not a string we can match). So
    // ask first: a service `launchctl print` can see is registered, full
    // stop. Only bootstrap when it is genuinely absent.
    if service_loaded(&domain) {
        return Ok(());
    }
    let output = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(&plist)
        .output()
        .map_err(|err| format!("cannot run launchctl: {err}"))?;
    if output.status.success() || service_loaded(&domain) {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "launchctl bootstrap {domain} failed: {}",
        stderr.trim()
    ))
}

/// Whether launchd already knows our service in `domain`. `print`
/// exiting zero means the service is registered — the one fact we need.
fn service_loaded(domain: &str) -> bool {
    Command::new("launchctl")
        .arg("print")
        .arg(format!("{domain}/{LABEL}"))
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Start the daemon under launchd supervision, returning `true` only when
/// it actually ran the service via `launchctl kickstart`.
///
/// The CLI calls this from `spawn_daemon` before falling back to a direct
/// `Command` spawn. A direct spawn produces an *unsupervised* daemon:
/// `launchctl print` reports `state = not running` and KeepAlive never
/// fires, so a `kill -9` is never restarted. Kickstarting an already
/// bootstrapped service is what makes KeepAlive({SuccessfulExit=false})
/// genuinely protect the steady-state daemon.
///
/// Returns `false` — and the caller spawns directly — unless all hold:
///   * the plist exists at `~/Library/LaunchAgents/<LABEL>.plist`, and
///   * its `ProgramArguments` binary equals the current exe (no stale
///     path left behind by a moved/upgraded binary), and
///   * the service is bootstrapped (`launchctl print` succeeds).
///
/// Those guards keep the first-ever run (no plist yet) and the
/// post-upgrade run (plist still names the old binary) on the direct-spawn
/// path. That spawned daemon's own `ensure_registered` then rewrites the
/// plist to the current exe and re-bootstraps, so the *next* spawn sees a
/// fresh, loaded service and converges onto this supervised path.
pub fn kickstart_if_supervised() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.display().to_string();
    let Some(plist) = plist_path() else {
        return false;
    };
    // Plist must exist and name *this* binary; a stale path means a direct
    // spawn is needed so the new daemon can rewrite + re-bootstrap.
    match plist_program_binary(&plist) {
        Some(named) if named == exe => {}
        _ => return false,
    }
    let domain = format!("gui/{}", nix_getuid());
    if !service_loaded(&domain) {
        return false;
    }
    Command::new("launchctl")
        .args(["kickstart", &format!("{domain}/{LABEL}")])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// The first `<string>` inside the plist's `ProgramArguments` array — the
/// binary launchd would exec. Returns `None` if the file is missing or the
/// array cannot be located. A minimal scan: our plist is hand-written by
/// `plist_xml`, so the first array entry is always the binary path.
fn plist_program_binary(plist: &PathBuf) -> Option<String> {
    let xml = std::fs::read_to_string(plist).ok()?;
    let after_key = xml.split("<key>ProgramArguments</key>").nth(1)?;
    let array = after_key.split("<array>").nth(1)?;
    let array = array.split("</array>").next()?;
    let open = array.find("<string>")? + "<string>".len();
    let close = array[open..].find("</string>")? + open;
    Some(xml_unescape(array[open..close].trim()))
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The caller's real uid. `launchctl`'s `gui/<uid>` domain wants the
/// numeric uid; libc's `getuid` is the portable source without a crate.
fn nix_getuid() -> u32 {
    // SAFETY is enforced by the workspace `unsafe_code = "forbid"`; we
    // shell out instead to stay within it.
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// The degradation warning for `status`/`list` — `None` when persistence
/// is registered (the steady state), `Some(line)` when it is degraded.
pub fn degradation_warning() -> Option<String> {
    let status = std::fs::read_to_string(persistence_status_path()).ok()?;
    let status = status.trim();
    if status.is_empty() || status == "registered" {
        return None;
    }
    Some(format!(
        "leases enforced only while the daemon happens to be running: {status}"
    ))
}
