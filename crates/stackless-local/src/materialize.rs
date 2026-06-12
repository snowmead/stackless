//! Source materialization with gix (ARCHITECTURE.md §8): one bare cache
//! repo per source URL, shared across instances; per instance a thin
//! repo whose `objects/info/alternates` points at the cache, HEAD
//! detached at the pinned commit, checked out into instance-owned space.
//!
//! No git CLI dependency. gix's blocking network operations run inside
//! `spawn_blocking` at the call site (the substrate's `execute` is
//! async). Credential prompting is disabled on every repo we open: a
//! prompt cannot be honored non-interactively and would hang `up`, so a
//! missing credential must surface as a `local.git.*` fault instead.

use std::path::{Path, PathBuf};

use stackless_core::lockfile::FileLock;

use crate::error::LocalError;
use crate::git_auth::GitAuth;

/// How long parallel materialize calls wait for the shared bare cache.
const GIT_CACHE_LOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Disables the interactive credential prompt on any repo we open or
/// clone. gix's credential cascade otherwise falls through to a terminal
/// prompt, which would hang an unattended `up`; with this set a missing
/// credential fails fast and we map it to a `local.git.*` fault.
const NO_PROMPT: &str = "credential.terminalPrompt=false";

/// Source materialization scoped to a state root (§8).
#[derive(Debug)]
pub struct Materializer<'a> {
    state_root: &'a Path,
    auth: GitAuth,
}

impl<'a> Materializer<'a> {
    pub fn new(state_root: &'a Path) -> Self {
        Self {
            state_root,
            auth: GitAuth::default(),
        }
    }

    pub fn with_auth(mut self, auth: GitAuth) -> Self {
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
    /// Blocking by construction (gix's `blocking-network-client`); callers
    /// run it inside `spawn_blocking`.
    pub fn materialize(
        &self,
        instance: &str,
        service: &str,
        repo: &str,
        reference: &str,
    ) -> Result<(PathBuf, String), LocalError> {
        let cache = self.cache_path(repo);
        let cache_repo = self.ensure_cache(repo, &cache)?;
        let commit_id = resolve_ref(&cache_repo, service, repo, reference)?;
        let commit =
            cache_repo
                .find_commit(commit_id)
                .map_err(|err| LocalError::GitRefNotFound {
                    service: service.to_owned(),
                    repo: repo.to_owned(),
                    reference: reference.to_owned(),
                    detail: err.to_string(),
                })?;
        let tree_id = commit
            .tree_id()
            .map_err(|err| checkout_err(service, &commit_id.to_string(), Path::new(""), &err))?
            .detach();

        let dest = self.source_dir(instance, service);
        checkout(&cache, &dest, service, &commit_id, &tree_id)?;
        Ok((dest, commit_id.to_string()))
    }

    /// Clone the bare cache if absent, else fetch the default remote to
    /// refresh it. A fetch failure (network, auth) surfaces as
    /// `GitFetchFailed`; a clone failure as `GitCloneFailed`.
    fn ensure_cache(&self, repo: &str, cache: &Path) -> Result<gix::Repository, LocalError> {
        let lock_path = FileLock::git_cache_lock_path(&Materializer::cache_key(repo));
        let _guard =
            FileLock::acquire_with_wait(&lock_path, GIT_CACHE_LOCK_BUDGET).map_err(|err| {
                if cache.join("objects").is_dir() {
                    LocalError::GitFetchFailed {
                        repo: repo.to_owned(),
                        detail: format!("git cache lock: {err}"),
                    }
                } else {
                    LocalError::GitCloneFailed {
                        repo: repo.to_owned(),
                        detail: format!("git cache lock: {err}"),
                    }
                }
            })?;
        if cache.join("objects").is_dir() {
            let cache_repo = gix::open_opts(cache, gix_open_opts()).map_err(|err| {
                LocalError::GitFetchFailed {
                    repo: repo.to_owned(),
                    detail: err.to_string(),
                }
            })?;
            self.fetch_cache(repo, &cache_repo)?;
            return Ok(cache_repo);
        }
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent).map_err(|err| LocalError::GitCloneFailed {
                repo: repo.to_owned(),
                detail: err.to_string(),
            })?;
        }
        let auth = self.auth.clone();
        let mut prepare = gix::prepare_clone_bare(repo, cache)
            .map_err(|err| LocalError::GitCloneFailed {
                repo: repo.to_owned(),
                detail: err.to_string(),
            })?
            .with_in_memory_config_overrides([NO_PROMPT])
            .configure_connection(move |conn| {
                auth.install_on_connection(conn)?;
                Ok(())
            });
        let (cache_repo, _outcome) = prepare
            .fetch_only(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)
            .map_err(|err| LocalError::GitCloneFailed {
                repo: repo.to_owned(),
                detail: err.to_string(),
            })?;
        Ok(cache_repo)
    }

    /// Fetch the cache's default remote to pick up new commits/refs the
    /// pinned ref may point at — the update path the clone-only spike lacked.
    fn fetch_cache(&self, repo: &str, cache_repo: &gix::Repository) -> Result<(), LocalError> {
        let fetch_err = |detail: String| LocalError::GitFetchFailed {
            repo: repo.to_owned(),
            detail,
        };
        let remote = cache_repo
            .find_default_remote(gix::remote::Direction::Fetch)
            .ok_or_else(|| fetch_err("the cache repo has no default remote to fetch from".into()))?
            .map_err(|err| fetch_err(err.to_string()))?;
        let mut connection = remote
            .connect(gix::remote::Direction::Fetch)
            .map_err(|err| fetch_err(err.to_string()))?;
        self.auth
            .install_on_connection(&mut connection)
            .map_err(|err| fetch_err(err.to_string()))?;
        connection
            .prepare_fetch(
                gix::progress::Discard,
                gix::remote::ref_map::Options::default(),
            )
            .map_err(|err| fetch_err(err.to_string()))?
            .receive(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)
            .map_err(|err| fetch_err(err.to_string()))?;
        Ok(())
    }
}

/// Resolve the pinned ref to a commit. The value may be a branch, tag,
/// or full SHA; `prepare_clone_bare` writes both `refs/heads/<name>` and
/// `refs/remotes/origin/<name>`, so the as-is lookup covers branches,
/// tags and SHAs. We fall back to `refs/remotes/origin/<ref>` for the
/// case a ref only exists under the remote-tracking namespace.
fn resolve_ref(
    cache_repo: &gix::Repository,
    service: &str,
    repo: &str,
    reference: &str,
) -> Result<gix::ObjectId, LocalError> {
    let not_found = |detail: String| LocalError::GitRefNotFound {
        service: service.to_owned(),
        repo: repo.to_owned(),
        reference: reference.to_owned(),
        detail,
    };
    if let Ok(id) = cache_repo.rev_parse_single(reference) {
        return Ok(id.detach());
    }
    let remote_ref = format!("refs/remotes/origin/{reference}");
    cache_repo
        .rev_parse_single(remote_ref.as_str())
        .map(|id| id.detach())
        .map_err(|err| not_found(err.to_string()))
}

/// Build the thin instance repo (alternates + detached HEAD) and check
/// the tree out. Re-materialization: if the checkout already exists we
/// remove it and rebuild — simpler and provably correct against a dirty
/// or stale worktree than checking out over it, and the cache holds the
/// objects so the rebuild copies nothing across repos.
fn checkout(
    cache: &Path,
    dest: &Path,
    service: &str,
    commit_id: &gix::ObjectId,
    tree_id: &gix::ObjectId,
) -> Result<(), LocalError> {
    let commit_hex = commit_id.to_string();
    let err = |detail: String| LocalError::GitCheckoutFailed {
        service: service.to_owned(),
        commit: commit_hex.clone(),
        dest: dest.display().to_string(),
        detail,
    };

    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| err(e.to_string()))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| err(e.to_string()))?;

    let instance_repo = gix::init(dest).map_err(|e| err(e.to_string()))?;
    let git_dir = instance_repo.path().to_path_buf();
    std::fs::create_dir_all(git_dir.join("objects/info")).map_err(|e| err(e.to_string()))?;
    std::fs::write(
        git_dir.join("objects/info/alternates"),
        format!("{}\n", cache.join("objects").display()),
    )
    .map_err(|e| err(e.to_string()))?;
    std::fs::write(git_dir.join("HEAD"), format!("{commit_hex}\n"))
        .map_err(|e| err(e.to_string()))?;

    // Reopen so the alternates take effect; resolve the tree through the
    // shared object store the alternates now expose.
    let instance_repo = gix::open(dest).map_err(|e| err(e.to_string()))?;
    let index_state = instance_repo
        .index_from_tree(tree_id)
        .map_err(|e| err(e.to_string()))?;
    let mut index = index_state.into_parts().0;
    let opts = gix::worktree::state::checkout::Options {
        fs: gix::fs::Capabilities::probe(instance_repo.git_dir()),
        destination_is_initially_empty: true,
        ..Default::default()
    };
    let outcome = gix::worktree::state::checkout(
        &mut index,
        dest,
        instance_repo.objects.clone(),
        &gix::progress::Discard,
        &gix::progress::Discard,
        &gix::interrupt::IS_INTERRUPTED,
        opts,
    )
    .map_err(|e| err(e.to_string()))?;

    // Invariant 4: a partial checkout is a failure, not silence.
    if !outcome.errors.is_empty() || !outcome.collisions.is_empty() {
        return Err(err(format!(
            "{} write errors, {} collisions",
            outcome.errors.len(),
            outcome.collisions.len()
        )));
    }
    Ok(())
}

/// Open options that load git installation config (credential helpers)
/// and disable the interactive credential prompt.
fn gix_open_opts() -> gix::open::Options {
    use gix::sec::trust::DefaultForLevel;
    let mut opts = gix::open::Options::default_for_level(gix::sec::Trust::Full);
    opts.permissions.config.git_binary = true;
    opts.config_overrides([NO_PROMPT])
}

fn checkout_err(
    service: &str,
    commit: &str,
    dest: &Path,
    err: &dyn std::error::Error,
) -> LocalError {
    LocalError::GitCheckoutFailed {
        service: service.to_owned(),
        commit: commit.to_owned(),
        dest: dest.display().to_string(),
        detail: err.to_string(),
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
