//! Git HTTPS authentication for source materialization: honor the
//! operator's git credential helpers by default, with an optional
//! `GITHUB_TOKEN` override for GitHub HTTPS repos.

use std::collections::BTreeMap;

use gix::remote::AuthenticateFn;
use secrecy::{ExposeSecret, SecretString};

const GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";
const GITHUB_TOKEN_USER: &str = "x-access-token";

/// Resolved git authentication for gix clone/fetch operations.
#[derive(Clone, Debug, Default)]
pub struct GitAuth {
    github_token: Option<SecretString>,
}

impl GitAuth {
    /// Read an optional GitHub token from the secrets map or process env.
    pub fn from_secrets(secrets: &BTreeMap<String, String>) -> Self {
        let raw = secrets
            .get(GITHUB_TOKEN_ENV)
            .cloned()
            .or_else(|| std::env::var(GITHUB_TOKEN_ENV).ok());
        Self {
            github_token: raw.map(SecretString::from),
        }
    }

    /// Install a credential handler on an open remote connection.
    pub fn install_on_connection<T>(
        &self,
        conn: &mut gix::remote::Connection<'_, '_, T>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: gix_transport::client::blocking_io::Transport,
    {
        let url = conn
            .remote()
            .url(gix::remote::Direction::Fetch)
            .ok_or("remote has no fetch URL")?
            .clone();
        let fallback = conn.configured_credentials(url.clone())?;
        conn.set_credentials(self.build_credentials(url, fallback));
        Ok(())
    }

    #[allow(clippy::result_large_err)] // gix `AuthenticateFn` uses `protocol::Result` verbatim.
    fn build_credentials(
        &self,
        remote_url: gix::Url,
        mut fallback: AuthenticateFn<'static>,
    ) -> AuthenticateFn<'static> {
        let token = self.github_token.clone();
        let github_https = is_github_https(&remote_url);
        let mut token_attempted = false;
        Box::new(move |action| {
            if github_https
                && !token_attempted
                && let gix::credentials::helper::Action::Get(ctx) = &action
                && let Some(tok) = &token
            {
                token_attempted = true;
                return Ok(Some(gix::credentials::protocol::Outcome {
                    identity: gix::sec::identity::Account {
                        username: GITHUB_TOKEN_USER.into(),
                        password: tok.expose_secret().to_owned(),
                        oauth_refresh_token: None,
                    },
                    next: gix::credentials::helper::NextAction::from(ctx.clone()),
                }));
            }
            fallback(action)
        })
    }

    #[cfg(test)]
    fn has_github_token(&self) -> bool {
        self.github_token.is_some()
    }

    #[cfg(test)]
    fn github_token_expose(&self) -> Option<&SecretString> {
        self.github_token.as_ref()
    }
}

fn is_github_https(url: &gix::Url) -> bool {
    url.scheme == gix::url::Scheme::Https
        && matches!(
            url.host.as_deref(),
            Some("github.com") | Some("www.github.com")
        )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use secrecy::ExposeSecret;

    use super::{GITHUB_TOKEN_ENV, GitAuth};

    #[test]
    fn wraps_token_from_secrets_map() {
        let mut secrets = BTreeMap::new();
        secrets.insert(GITHUB_TOKEN_ENV.into(), "ghp_test".into());
        let auth = GitAuth::from_secrets(&secrets);
        assert!(auth.has_github_token());
    }

    #[test]
    fn token_exposes_expected_value() {
        let mut secrets = BTreeMap::new();
        secrets.insert(GITHUB_TOKEN_ENV.into(), "ghp_test".into());
        let auth = GitAuth::from_secrets(&secrets);
        assert_eq!(
            auth.github_token_expose().expect("token").expose_secret(),
            "ghp_test"
        );
    }

    #[test]
    fn debug_redacts_token() {
        let mut secrets = BTreeMap::new();
        secrets.insert(GITHUB_TOKEN_ENV.into(), "ghp_super_secret".into());
        let auth = GitAuth::from_secrets(&secrets);
        let debug = format!("{auth:?}");
        assert!(!debug.contains("ghp_super_secret"));
    }

    #[test]
    fn github_https_host_detection() {
        let github = gix::url::parse("https://github.com/org/repo.git".into()).expect("url");
        assert!(super::is_github_https(&github));
        let gitlab = gix::url::parse("https://gitlab.com/org/repo.git".into()).expect("url");
        assert!(!super::is_github_https(&gitlab));
    }
}
