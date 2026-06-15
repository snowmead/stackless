//! Source materialization (ARCHITECTURE.md §8) on grit-lib, via the
//! `stackless-git` primitives: one bare cache repo per source URL, shared
//! across instances; per instance a thin repo whose `objects/info/alternates`
//! points at the cache, HEAD detached at the pinned commit, the tree checked
//! out into instance-owned space.
//!
//! No git CLI dependency. grit-lib's blocking network/checkout work runs inside
//! `spawn_blocking` at the call site (the substrate's `execute` is async).
//! Credential prompting cannot be honored non-interactively and would hang
//! `up`; grit-lib's helper provider is non-interactive by design, so a missing
//! credential surfaces as a `local.git.*` fault instead.

use std::path::{Path, PathBuf};

use stackless_core::lockfile::FileLock;
use stackless_git::Credentials;

use crate::error::LocalError;

/// How long parallel materialize calls wait for the shared bare cache.
const GIT_CACHE_LOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// All branches and tags at full depth: the cache must hold whatever commit the
/// pinned ref names (branch, tag, or SHA), and a refresh must pick up new
/// commits the pin may now point at.
const CACHE_REFSPECS: &[&str] = &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"];

/// Source materialization scoped to a state root (§8).
#[derive(Debug)]
pub struct Materializer<'a> {
    state_root: &'a Path,
    auth: Credentials,
}

impl<'a> Materializer<'a> {
    pub fn new(state_root: &'a Path) -> Self {
        Self {
            state_root,
            auth: Credentials::default(),
        }
    }

    pub fn with_auth(mut self, auth: Credentials) -> Self {
        self.auth = auth;
        self
    }

    /// A filesystem-safe, collision-resistant slug for a source URL.
    pub fn cache_key(repo: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        repo.hash(&mut hasher);
        let digest = hasher.finish();
        let tail: String = repo
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let tail = tail.trim_matches('-');
        let tail = &tail[tail.len().saturating_sub(48)..];
        format!("{tail}-{digest:016x}")
    }

    /// `<state_root>/sources/<instance>/<service>` (§8).
    pub fn source_dir(&self, instance: &str, service: &str) -> PathBuf {
        self.state_root.join("sources").join(instance).join(service)
    }

    fn cache_path(&self, repo: &str) -> PathBuf {
        self.state_root
            .join("cache/git")
            .join(Self::cache_key(repo))
    }

    /// Materialize `service`'s source at the pinned `reference` into
    /// instance-owned space, returning the checkout path and commit hex.
    ///
    /// Blocking by construction (grit-lib's network/checkout work); callers run
    /// it inside `spawn_blocking`.
    pub fn materialize(
        &self,
        instance: &str,
        service: &str,
        repo: &str,
        reference: &str,
    ) -> Result<(PathBuf, String), LocalError> {
        let cache = self.cache_path(repo);
        self.ensure_cache(repo, &cache)?;
        let commit = stackless_git::resolve_commit(&cache, reference).map_err(|err| {
            LocalError::GitRefNotFound {
                service: service.to_owned(),
                repo: repo.to_owned(),
                reference: reference.to_owned(),
                detail: err.to_string(),
            }
        })?;
        let dest = self.source_dir(instance, service);
        stackless_git::checkout_detached(&dest, &cache, &commit).map_err(|err| {
            LocalError::GitCheckoutFailed {
                service: service.to_owned(),
                commit: commit.clone(),
                dest: dest.display().to_string(),
                detail: err.to_string(),
            }
        })?;
        Ok((dest, commit))
    }

    /// Ensure the bare cache exists and is current: a fresh fetch clones it,
    /// a subsequent one refreshes it (grit-lib copies only objects not already
    /// present, so a no-op refresh writes nothing). A failure surfaces as
    /// `GitCloneFailed` when the cache was absent, else `GitFetchFailed`.
    fn ensure_cache(&self, repo: &str, cache: &Path) -> Result<(), LocalError> {
        let lock_path = FileLock::git_cache_lock_path(&Materializer::cache_key(repo));
        let _guard =
            FileLock::acquire_with_wait(&lock_path, GIT_CACHE_LOCK_BUDGET).map_err(|err| {
                let detail = format!("git cache lock: {err}");
                cache_fault(repo, cache, detail)
            })?;
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent).map_err(|err| LocalError::GitCloneFailed {
                repo: repo.to_owned(),
                detail: err.to_string(),
            })?;
        }
        let existed = cache.join("objects").is_dir();
        stackless_git::fetch_bare(cache, repo, CACHE_REFSPECS, None, &self.auth).map_err(|err| {
            let detail = err.to_string();
            if existed {
                LocalError::GitFetchFailed {
                    repo: repo.to_owned(),
                    detail,
                }
            } else {
                LocalError::GitCloneFailed {
                    repo: repo.to_owned(),
                    detail,
                }
            }
        })?;
        Ok(())
    }
}

/// Classify a cache failure by whether the cache already exists: a missing
/// cache is a clone failure, an existing one a fetch (refresh) failure.
fn cache_fault(repo: &str, cache: &Path, detail: String) -> LocalError {
    if cache.join("objects").is_dir() {
        LocalError::GitFetchFailed {
            repo: repo.to_owned(),
            detail,
        }
    } else {
        LocalError::GitCloneFailed {
            repo: repo.to_owned(),
            detail,
        }
    }
}

/// Observe a materialized source (§8 observe contract for kind
/// "source"): Present iff the checkout exists and its `.git/HEAD` still
/// names the recorded commit; Gone otherwise.
pub fn observe(dest: &Path, commit: &str) -> bool {
    if !dest.exists() {
        return false;
    }
    std::fs::read_to_string(dest.join(".git/HEAD"))
        .map(|head| head.trim() == commit)
        .unwrap_or(false)
}

/// Destroy a materialized source (§8): remove the instance's checkout
/// for the service. The shared cache is per-URL, not per-instance, and
/// stays. Tolerates an already-absent directory.
pub fn destroy(dest: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dest) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
