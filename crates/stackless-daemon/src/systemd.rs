//! Verify a user service's restart, boot, and logout behavior before claiming persistence.

use stackless_core::helper_command::HelperCommand;
use std::collections::BTreeMap;
use std::time::Duration;

pub const UNIT: &str = "stackless-controller.service";

fn read(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = HelperCommand::new(program);
    command.args(args);
    match command.run(Duration::from_secs(5)) {
        stackless_core::process::TimedCommand::Finished(output) if output.status.success() => {
            String::from_utf8(output.stdout).map_err(|_| format!("{program} returned invalid text"))
        }
        _ => Err(format!("cannot verify persistence with {program}")),
    }
}

pub fn verify_current_process() -> Result<(), String> {
    let properties = read(
        "systemctl",
        &[
            "--user",
            "show",
            UNIT,
            "--property=MainPID,ActiveState,UnitFileState,Restart,StartLimitIntervalUSec",
            "--no-pager",
        ],
    )?;
    let uid = stackless_core::process::effective_user_id().to_string();
    let linger = read(
        "loginctl",
        &[
            "show-user",
            &uid,
            "--property=Linger",
            "--value",
            "--no-pager",
        ],
    )?;
    verify(std::process::id(), &properties, &linger)
}

fn verify(pid: u32, properties: &str, linger: &str) -> Result<(), String> {
    let mut values = BTreeMap::new();
    for line in properties.lines().filter(|line| !line.is_empty()) {
        let (key, value) = line
            .split_once('=')
            .ok_or("systemd returned malformed properties")?;
        if values.insert(key, value).is_some() {
            return Err("systemd returned duplicate properties".into());
        }
    }
    if values
        .get("MainPID")
        .and_then(|value| value.parse::<u32>().ok())
        != Some(pid)
    {
        return Err("controller is not the systemd unit's main process".into());
    }
    if !matches!(
        values.get("ActiveState").copied(),
        Some("active" | "activating")
    ) {
        return Err("controller systemd unit is not active".into());
    }
    if values.get("UnitFileState") != Some(&"enabled") || values.get("Restart") != Some(&"always") {
        return Err("controller unit must be enabled with Restart=always".into());
    }
    if values.get("StartLimitIntervalUSec") != Some(&"0") {
        return Err(
            "controller unit must disable restart rate limiting with StartLimitIntervalSec=0"
                .into(),
        );
    }
    if linger.trim() != "yes" {
        return Err(
            "enable user lingering so the controller starts at boot and survives logout".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_requires_the_running_enabled_unit_and_user_lingering() {
        let good = "MainPID=42\nActiveState=active\nUnitFileState=enabled\nRestart=always\nStartLimitIntervalUSec=0\n";
        assert!(verify(42, good, "yes\n").is_ok());
        for broken in [
            good.replace("MainPID=42", "MainPID=41"),
            good.replace("active", "inactive"),
            good.replace("enabled", "disabled"),
            good.replace("always", "on-failure"),
            good.replace("USec=0", "USec=10s"),
            format!("{good}MainPID=42\n"),
        ] {
            assert!(verify(42, &broken, "yes").is_err());
        }
        assert!(verify(42, good, "no").is_err());
        assert!(verify(42, "", "yes").is_err());
    }
}
