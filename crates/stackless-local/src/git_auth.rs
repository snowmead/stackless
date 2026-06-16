//! Git HTTPS authentication for source materialization: an optional
//! `GITHUB_TOKEN` override for GitHub HTTPS repos, falling back to the
//! operator's git credential helpers. Resolution and the credential provider
//! live in `stackless-git`; this is the alias the local substrate uses.

pub use stackless_git::Credentials as GitAuth;
