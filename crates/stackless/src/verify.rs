//! `stackless verify` (§7): run the stack's one verify command with
//! env built by the same interpolation mechanism services use. Success
//! renews the lease (§6) — verify is the keepalive an agent runs
//! mid-work: it renews *and* proves health.

use std::path::PathBuf;

use stackless_core::def;

use crate::commands::open_store;
use crate::error::CliError;
use crate::output::Output;

pub fn verify(name: &str, output: &Output) -> Result<(), CliError> {
    let store = open_store()?;
    let record = store
        .instance(name)?
        .ok_or_else(|| stackless_core::state::StateError::InstanceNotFound { name: name.into() })?;
    let def = def::parse(&record.definition)?;
    let Some(spec) = &def.stack.verify else {
        return Err(CliError::VerifyNotDeclared);
    };

    // Renewal at the start of every mutating verb (§6).
    store.renew_lease_at_recorded_duration(name)?;

    let checkpoints = store.checkpoints(name)?;
    let def_dir = if record.definition_dir.is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(&record.definition_dir)
    };
    let secrets = crate::secrets::resolve(&def, &def_dir)?;
    let namespace = stackless_local::wiring::namespace(
        &def,
        name,
        stackless_daemon::proxy::proxy_port(),
        &checkpoints,
        &secrets,
    );
    let mut env = Vec::new();
    for (key, value) in &spec.env {
        let location = format!("stack.verify.env.{key}");
        let resolved = def::interp::resolve(value, &namespace, &location)?;
        env.push((key.clone(), resolved));
    }

    // The verify command runs from the root-origin service's
    // materialized source (the stack's "face"); without one, the first
    // service's. (D19 — [stack.verify] has no service to anchor to.)
    let anchor = def
        .services
        .iter()
        .find(|(_, s)| s.root_origin)
        .map(|(n, _)| n.clone())
        .or_else(|| def.services.keys().next().cloned())
        .unwrap_or_default();
    let dir = materialized_dir(&checkpoints, &anchor).unwrap_or_else(|| PathBuf::from("."));

    output.message(&format!(
        "verify: running `{}` in {}",
        spec.run,
        dir.display()
    ));
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", &spec.run])
        .current_dir(&dir)
        .envs(env)
        .status()
        .map_err(CliError::Runtime)?;
    if !status.success() {
        return Err(CliError::VerifyFailed {
            status: status.to_string(),
        });
    }
    // A successful verify renews again (§6).
    store.renew_lease_at_recorded_duration(name)?;
    output.message(&format!("{name}: verify passed (lease renewed)"));
    Ok(())
}

fn materialized_dir(
    checkpoints: &[stackless_core::state::Checkpoint],
    service: &str,
) -> Option<PathBuf> {
    let checkpoint = checkpoints
        .iter()
        .find(|c| c.step_id == format!("materialize:{service}"))?;
    let payload = serde_json::from_str::<serde_json::Value>(&checkpoint.payload).ok()?;
    Some(PathBuf::from(payload.get("path")?.as_str()?))
}
