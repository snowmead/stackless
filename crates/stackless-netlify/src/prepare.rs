//! Operator-side cloud prepare (§4): shallow git checkout on the operator's
//! machine. The mechanics are shared (`stackless_cloud::prepare`); this maps the
//! neutral failure to Netlify's error so its `netlify.*` code/remediation hold.

use crate::error::NetlifyError;

pub fn run_prepare_command(
    service: &str,
    repo: &str,
    reference: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<(), NetlifyError> {
    stackless_cloud::prepare::run_prepare_command(service, repo, reference, command, env).map_err(
        |f| NetlifyError::PrepareFailed {
            service: f.service,
            command: f.command,
            message: f.message,
            log_tail: f.log_tail,
        },
    )
}
