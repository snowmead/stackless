//! Parse git remote URLs from stack definitions into Vercel `gitSource` fields.

use crate::error::VercelError;

/// A public GitHub repository referenced by `services.*.source.repo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepo {
    pub org: String,
    pub repo: String,
}

/// Parse `https://github.com/{org}/{repo}` (optional `.git` suffix).
pub fn parse_github_repo(url: &str) -> Result<GitHubRepo, VercelError> {
    let trimmed = url.trim().trim_end_matches('/');
    let Some(rest) = trimmed.strip_prefix("https://github.com/") else {
        return Err(VercelError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!(
                "vercel requires a public GitHub HTTPS remote (https://github.com/org/repo), \
                 got {url:?}"
            ),
        });
    };
    let mut parts = rest.split('/');
    let org = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VercelError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!("missing GitHub org in {url:?}"),
        })?;
    let repo = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VercelError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!("missing GitHub repo name in {url:?}"),
        })?
        .trim_end_matches(".git");
    if parts.next().is_some() {
        return Err(VercelError::ConfigInvalid {
            location: "services.*.source.repo".into(),
            detail: format!("unexpected path segments in {url:?}"),
        });
    }
    Ok(GitHubRepo {
        org: org.to_owned(),
        repo: repo.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_https_remotes() {
        let repo = parse_github_repo("https://github.com/acme/widget").unwrap();
        assert_eq!(repo.org, "acme");
        assert_eq!(repo.repo, "widget");
        let repo = parse_github_repo("https://github.com/acme/widget.git/").unwrap();
        assert_eq!(repo.repo, "widget");
    }

    #[test]
    fn rejects_non_github_remotes() {
        assert!(parse_github_repo("https://gitlab.com/a/b").is_err());
    }
}