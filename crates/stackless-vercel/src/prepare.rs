//! Operator-side cloud prepare: shallow git checkout on the operator's machine.
//! The mechanics are shared (`stackless_cloud::prepare`); this maps the neutral
//! failure to Vercel's error so its `vercel.*` code/remediation hold.

use crate::error::VercelError;

pub fn run_prepare_command(
    service: &str,
    repo: &str,
    reference: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(), VercelError> {
    stackless_cloud::prepare::run_prepare_command(service, repo, reference, command, env).map_err(
        |f| VercelError::PrepareFailed {
            service: f.service,
            command: f.command,
            message: f.message,
            log_tail: f.log_tail,
        },
    )
}
