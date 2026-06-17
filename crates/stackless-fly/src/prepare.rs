//! Operator-side cloud prepare (§4): shallow git checkout on the operator's
//! machine. The mechanics are shared (`stackless_cloud::prepare`); this maps the
//! neutral failure to Fly's error so its `fly.*` code/remediation hold.

use crate::error::FlyError;

pub fn run_prepare_command(
    service: &str,
    repo: &str,
    reference: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(), FlyError> {
    stackless_cloud::prepare::run_prepare_command(service, repo, reference, command, env).map_err(
        |f| FlyError::PrepareFailed {
            service: f.service,
            command: f.command,
            message: f.message,
            log_tail: f.log_tail,
        },
    )
}
