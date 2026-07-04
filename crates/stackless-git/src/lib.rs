//! Pure-Rust git operations for stackless, backed by `grit-lib`.
//!
//! Sole owner of the `grit-lib` dependency. Two consumers build on the
//! primitives here:
//!
//! - `stackless-local` source materialization (ARCHITECTURE.md §8): one bare
//!   cache repo per source URL ([`fetch_bare`]), shared across instances; per
//!   instance a thin checkout whose objects are borrowed from the cache via
//!   `objects/info/alternates` ([`checkout_detached`]). grit-lib's `Odb`
//!   honors that alternates file, so instances duplicate no objects.
//! - `stackless-vercel` / `stackless-render` cloud prepare: a self-contained
//!   shallow clone + checkout ([`clone_checkout`]).
//!
//! Transport is dispatched by URL scheme: `https`/`http` over grit-lib's
//! `ureq`-backed smart-HTTP client, and `file://`/local paths over the
//! local object-transfer path. Credential prompting is non-interactive by
//! design (grit-lib's helper provider never blocks on a TTY): a missing
//! credential fails fast so callers can map it to a typed fault.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use grit_lib::config::ConfigSet;
use grit_lib::credentials::{Credential, CredentialProvider, HelperCredentialProvider};
use grit_lib::error::Result as GritResult;
use grit_lib::fetch::NoProgress;
use grit_lib::repo::{self, Repository};
use grit_lib::rev_parse::{peel_to_tree, resolve_revision_as_commit_without_index_dwim};
use grit_lib::transfer::{self, FetchOptions};
use grit_lib::transport::http::http_fetch;
use grit_lib::transport::http::ureq_client::UreqHttpClient;
use secrecy::{ExposeSecret, SecretString};

const GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";
const GITHUB_TOKEN_USER: &str = "x-access-token";

/// Initial branch written into a freshly-initialized repo's `HEAD`. Overwritten
/// by [`checkout_detached`]; otherwise just the default ref name.
const DEFAULT_BRANCH: &str = "main";

/// Errors from the git primitives. Callers map these onto their own faults; the
/// operation (clone/fetch/resolve/checkout) is known at the call site.
/// Fixed author/committer for dirty snapshots — content-addressed, not time-based.
const SNAPSHOT_IDENT: &str = "stackless-dirty <dirty@stackless.local> 1700000000 +0000";

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error(
        "unsupported repository URL scheme: {0} (only https/http and local/file paths are supported)"
    )]
    UnsupportedScheme(String),
    #[error("{path} is not a git working tree")]
    InvalidWorkTree { path: PathBuf },
    #[error(transparent)]
    Lib(#[from] grit_lib::error::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Resolved git authentication: an optional `GITHUB_TOKEN` override for GitHub
/// HTTPS repos, falling back to the operator's git credential helpers.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    github_token: Option<SecretString>,
}

impl Credentials {
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

    /// Build an HTTP client wired with our credential provider, scoped to
    /// `git_dir` so the operator's git config cascade (credential helpers from
    /// `gh auth setup-git`, proxies, etc.) is honored.
    fn http_client(&self, git_dir: &Path) -> UreqHttpClient {
        let config = ConfigSet::load(Some(git_dir), true).unwrap_or_else(|_| ConfigSet::new());
        let provider = StacklessCredentials {
            github_token: self.github_token.clone(),
            helper: HelperCredentialProvider::new(config),
        };
        UreqHttpClient::with_credentials(Box::new(provider))
    }

    #[cfg(test)]
    fn has_github_token(&self) -> bool {
        self.github_token.is_some()
    }
}

/// Credential provider: inject the `GITHUB_TOKEN` for GitHub HTTPS, otherwise
/// delegate to the operator's configured git credential helpers.
struct StacklessCredentials {
    github_token: Option<SecretString>,
    helper: HelperCredentialProvider,
}

impl CredentialProvider for StacklessCredentials {
    fn fill(&self, input: &Credential) -> GritResult<Credential> {
        if let Some(token) = &self.github_token
            && is_github_https(input)
        {
            let mut cred = input.clone();
            cred.username = Some(GITHUB_TOKEN_USER.to_owned());
            cred.password = Some(token.expose_secret().to_owned());
            return Ok(cred);
        }
        self.helper.fill(input)
    }

    fn approve(&self, cred: &Credential) -> GritResult<()> {
        self.helper.approve(cred)
    }

    fn reject(&self, cred: &Credential) -> GritResult<()> {
        self.helper.reject(cred)
    }
}

fn is_github_https(cred: &Credential) -> bool {
    let host = cred
        .host
        .as_deref()
        .map(|h| h.split(':').next().unwrap_or(h));
    cred.protocol.as_deref() == Some("https")
        && matches!(host, Some("github.com") | Some("www.github.com"))
}

/// Ensure `git_dir` is an initialized bare repo, then fetch `refspecs` from
/// `url` into it. Covers both the initial clone (a fresh bare repo) and a
/// refresh of an existing one — grit-lib's fetch copies only objects not
/// already present, so a no-op refresh writes nothing.
pub fn fetch_bare(
    git_dir: &Path,
    url: &str,
    refspecs: &[&str],
    depth: Option<u32>,
    creds: &Credentials,
) -> Result<(), GitError> {
    if !git_dir.join("objects").is_dir() {
        repo::init_bare_clone_minimal(git_dir, DEFAULT_BRANCH, "files")?;
    }
    let opts = fetch_options(refspecs, depth);
    fetch_dispatch(git_dir, url, &opts, creds)
}

/// Resolve `reference` (branch, tag, or full/abbrev SHA) to a full commit hex,
/// using objects in `git_dir` and any alternates it points at.
pub fn resolve_commit(git_dir: &Path, reference: &str) -> Result<String, GitError> {
    let repo = Repository::open(git_dir, None)?;
    let oid = resolve_revision_as_commit_without_index_dwim(&repo, reference)?;
    Ok(oid.to_string())
}

/// Build a thin instance checkout at `dest`: a non-bare repo whose objects are
/// borrowed from `cache_git_dir` via `objects/info/alternates`, `HEAD` detached
/// at `commit`, and the commit's tree written into the working tree.
///
/// Re-materialization rebuilds from scratch: any existing checkout at `dest` is
/// removed first (provably correct against a dirty or stale worktree, and the
/// cache holds the objects so the rebuild copies nothing across repos).
pub fn checkout_detached(dest: &Path, cache_git_dir: &Path, commit: &str) -> Result<(), GitError> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    repo::init_repository(dest, false, DEFAULT_BRANCH, None, "files")?;
    let git_dir = dest.join(".git");

    // Borrow objects from the shared cache.
    let info = git_dir.join("objects/info");
    std::fs::create_dir_all(&info)?;
    std::fs::write(
        info.join("alternates"),
        format!("{}\n", cache_git_dir.join("objects").display()),
    )?;
    // Detached HEAD at the pinned commit (materialize's `observe` reads this).
    std::fs::write(git_dir.join("HEAD"), format!("{commit}\n"))?;

    // Reopen so the alternates take effect, then check the tree out.
    checkout_tree(&git_dir, dest, commit)
}

/// Shallow clone `url` at `reference` and check it out into `dest`. Self-
/// contained (no shared cache), mirroring `git clone --depth 1 --branch <ref>`.
/// Snapshot a dirty working tree into instance-owned space at `dest`.
///
/// Tracked files (including unstaged modifications), plus untracked
/// non-ignored files, are written as a synthetic commit whose hash is a
/// pure content address. Gitlinks (submodules) are omitted. If `dest`
/// already holds a snapshot at the same content hash, the working tree is
/// left unchanged.
pub fn snapshot_worktree(dest: &Path, work_tree: &Path) -> Result<String, GitError> {
    use grit_lib::index::{Index, entry_from_stat};
    use grit_lib::objects::{CommitData, ObjectKind, serialize_commit};
    use grit_lib::odb::Odb;
    use grit_lib::write_tree::write_tree_from_index;

    let work_tree = work_tree.canonicalize()?;
    if !work_tree.join(".git").is_dir() {
        return Err(GitError::InvalidWorkTree {
            path: work_tree.clone(),
        });
    }

    let src_git = work_tree.join(".git");
    let src_repo = Repository::open(&src_git, Some(&work_tree))?;
    let src_index = match Index::load(&src_repo.index_path()) {
        Ok(index) => index,
        Err(grit_lib::error::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            Index::new()
        }
        Err(err) => return Err(GitError::Lib(err)),
    };

    let files = collect_dirty_files(&src_repo, &src_index, &work_tree)?;

    if !dest.join(".git").is_dir() {
        repo::init_repository(dest, false, DEFAULT_BRANCH, None, "files")?;
    }
    let dest_git = dest.join(".git");
    let dest_repo = Repository::open(&dest_git, Some(dest))?;
    let dest_odb = Odb::new(&dest_git.join("objects"));

    let old_tree = read_head_tree(&dest_repo)?;

    let mut snap_index = Index::new();
    for file in &files {
        let bytes = read_worktree_bytes(&file.abs, file.mode)?;
        let oid = dest_odb.write(ObjectKind::Blob, &bytes)?;
        snap_index.add_or_replace(entry_from_stat(
            &file.abs,
            file.rel.as_bytes(),
            oid,
            file.mode,
        )?);
    }
    snap_index.sort();
    let tree = write_tree_from_index(&dest_odb, &snap_index, "")?;

    let commit = CommitData {
        tree,
        parents: Vec::new(),
        author: SNAPSHOT_IDENT.to_owned(),
        committer: SNAPSHOT_IDENT.to_owned(),
        author_raw: Vec::new(),
        committer_raw: Vec::new(),
        encoding: None,
        message: "stackless dirty snapshot\n".into(),
        raw_message: None,
    };
    let commit_oid = dest_odb.write(ObjectKind::Commit, &serialize_commit(&commit))?;
    let commit_hex = commit_oid.to_string();

    if read_detached_head(&dest_git)? == Some(commit_hex.clone()) {
        return Ok(commit_hex);
    }

    let dest_repo = Repository::open(&dest_git, Some(dest))?;
    grit_lib::porcelain::checkout::checkout_between_trees(&dest_repo, old_tree.as_ref(), &tree)?;
    std::fs::write(dest_git.join("HEAD"), format!("{commit_hex}\n"))?;
    Ok(commit_hex)
}

pub fn clone_checkout(
    url: &str,
    reference: &str,
    dest: &Path,
    creds: &Credentials,
) -> Result<(), GitError> {
    repo::init_repository(dest, false, reference, None, "files")?;
    let git_dir = dest.join(".git");
    let refspecs = [
        format!("+refs/heads/{reference}:refs/heads/{reference}"),
        format!("+refs/tags/{reference}:refs/tags/{reference}"),
    ];
    let opts = fetch_options(&refspecs, Some(1));
    fetch_dispatch(&git_dir, url, &opts, creds)?;
    checkout_tree(&git_dir, dest, reference)
}

/// Build a working git repo at `path` on branch `main`, creating one commit per
/// entry in `commits` (each the full set of `(relative_path, contents)` present
/// at that commit), and return the final HEAD commit hex.
///
/// Fixture support for tests that need a source repo to materialize from,
/// without shelling out to the `git` CLI. A fixed identity and timestamp keep
/// the resulting commit hashes deterministic. Behind the `test-support` feature
/// so it stays out of the production surface.
#[cfg(feature = "test-support")]
pub fn build_repo(path: &Path, commits: &[&[(&str, &str)]]) -> Result<String, GitError> {
    use grit_lib::index::{Index, MODE_REGULAR, entry_from_stat};
    use grit_lib::objects::{CommitData, ObjectId, ObjectKind, serialize_commit};
    use grit_lib::odb::Odb;
    use grit_lib::write_tree::write_tree_from_index;

    // Name <email> <unix-ts> <tz>, in git's author/committer wire format.
    const IDENT: &str = "stackless-test <test@stackless.local> 1700000000 +0000";

    repo::init_repository(path, false, DEFAULT_BRANCH, None, "files")?;
    let git_dir = path.join(".git");
    let odb = Odb::new(&git_dir.join("objects"));

    let mut parent: Option<ObjectId> = None;
    let mut head = ObjectId::zero();
    for (i, files) in commits.iter().enumerate() {
        let mut index = Index::new();
        for (rel, contents) in files.iter() {
            let abs = path.join(rel);
            if let Some(dir) = abs.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&abs, contents)?;
            let oid = odb.write(ObjectKind::Blob, contents.as_bytes())?;
            index.add_or_replace(entry_from_stat(&abs, rel.as_bytes(), oid, MODE_REGULAR)?);
        }
        index.sort();
        let tree = write_tree_from_index(&odb, &index, "")?;
        let commit = CommitData {
            tree,
            parents: parent.into_iter().collect(),
            author: IDENT.to_owned(),
            committer: IDENT.to_owned(),
            author_raw: Vec::new(),
            committer_raw: Vec::new(),
            encoding: None,
            message: format!("commit {}\n", i + 1),
            raw_message: None,
        };
        head = odb.write(ObjectKind::Commit, &serialize_commit(&commit))?;
        parent = Some(head);
    }
    grit_lib::refs::write_ref(&git_dir, "refs/heads/main", &head)?;
    Ok(head.to_string())
}

struct DirtyFile {
    rel: String,
    abs: PathBuf,
    mode: u32,
}

fn collect_dirty_files(
    repo: &Repository,
    index: &grit_lib::index::Index,
    work_tree: &Path,
) -> Result<Vec<DirtyFile>, GitError> {
    use grit_lib::index::{MODE_GITLINK, MODE_REGULAR};
    use grit_lib::porcelain::status::{IgnoredMode, collect_untracked_and_ignored};
    use std::collections::BTreeMap;

    let mut files: BTreeMap<String, DirtyFile> = BTreeMap::new();

    for entry in &index.entries {
        if entry.stage() != 0 || entry.mode == MODE_GITLINK {
            continue;
        }
        let rel = String::from_utf8_lossy(&entry.path).to_string();
        let abs = work_tree.join(&rel);
        if std::fs::symlink_metadata(&abs).is_err() {
            continue;
        }
        let mode = mode_from_path(&abs, entry.mode)?;
        files.insert(rel.clone(), DirtyFile { rel, abs, mode });
    }

    let (untracked, _) =
        collect_untracked_and_ignored(repo, index, work_tree, IgnoredMode::No, true, &[])?;
    for rel in untracked {
        let abs = work_tree.join(&rel);
        let Ok(meta) = std::fs::symlink_metadata(&abs) else {
            continue;
        };
        if meta.is_dir() {
            continue;
        }
        let mode = mode_from_path(&abs, MODE_REGULAR)?;
        files.insert(rel.clone(), DirtyFile { rel, abs, mode });
    }

    Ok(files.into_values().collect())
}

fn mode_from_path(abs: &Path, index_mode: u32) -> Result<u32, GitError> {
    use grit_lib::index::{MODE_SYMLINK, normalize_mode};

    if index_mode == MODE_SYMLINK {
        return Ok(MODE_SYMLINK);
    }
    let meta = std::fs::symlink_metadata(abs)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(normalize_mode(meta.mode()))
    }
    #[cfg(not(unix))]
    {
        use grit_lib::index::{MODE_EXECUTABLE, MODE_REGULAR};
        if meta.file_type().is_symlink() {
            Ok(MODE_SYMLINK)
        } else if meta.permissions().readonly() {
            Ok(MODE_REGULAR)
        } else {
            Ok(MODE_EXECUTABLE)
        }
    }
}

fn read_worktree_bytes(abs: &Path, mode: u32) -> Result<Vec<u8>, GitError> {
    use grit_lib::index::MODE_SYMLINK;

    if mode == MODE_SYMLINK {
        let target = std::fs::read_link(abs)?;
        return Ok(target.as_os_str().as_encoded_bytes().to_vec());
    }
    Ok(std::fs::read(abs)?)
}

fn read_detached_head(git_dir: &Path) -> Result<Option<String>, GitError> {
    let head = match std::fs::read_to_string(git_dir.join("HEAD")) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(GitError::Io(err)),
    };
    let trimmed = head.trim();
    if trimmed.len() == 40 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(Some(trimmed.to_owned()))
    } else {
        Ok(None)
    }
}

fn read_head_tree(repo: &Repository) -> Result<Option<grit_lib::objects::ObjectId>, GitError> {
    let Some(head) = read_detached_head(&repo.git_dir)? else {
        return Ok(None);
    };
    let commit = resolve_revision_as_commit_without_index_dwim(repo, &head)?;
    Ok(Some(peel_to_tree(repo, commit)?))
}

/// Open the repo at `git_dir` (work tree `work_tree`), resolve `spec` to a
/// commit, and write its tree into the working tree.
fn checkout_tree(git_dir: &Path, work_tree: &Path, spec: &str) -> Result<(), GitError> {
    let repo = Repository::open(git_dir, Some(work_tree))?;
    let commit = resolve_revision_as_commit_without_index_dwim(&repo, spec)?;
    let tree = peel_to_tree(&repo, commit)?;
    grit_lib::porcelain::checkout::checkout_between_trees(&repo, None, &tree)?;
    Ok(())
}

fn fetch_options<S: AsRef<str>>(refspecs: &[S], depth: Option<u32>) -> FetchOptions {
    FetchOptions {
        refspecs: refspecs.iter().map(|s| s.as_ref().to_owned()).collect(),
        depth,
        ..FetchOptions::default()
    }
}

/// Fetch into `git_dir`, choosing the transport from the URL scheme: local
/// object-transfer for `file://`/paths, smart-HTTP for `http(s)://`.
fn fetch_dispatch(
    git_dir: &Path,
    url: &str,
    opts: &FetchOptions,
    creds: &Credentials,
) -> Result<(), GitError> {
    if let Some(remote_git_dir) = local_repo_path(url) {
        transfer::fetch_local(git_dir, &remote_git_dir, opts)?;
        return Ok(());
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        let client = creds.http_client(git_dir);
        http_fetch(&client, git_dir, url, opts, &mut NoProgress)?;
        return Ok(());
    }
    Err(GitError::UnsupportedScheme(url.to_owned()))
}

/// For a `file://` or bare local path, the on-disk git directory to fetch from
/// (`<path>/.git` for a working repo, `<path>` for a bare one). `None` for any
/// URL carrying a non-local scheme.
fn local_repo_path(url: &str) -> Option<PathBuf> {
    let raw = if let Some(rest) = url.strip_prefix("file://") {
        rest
    } else if url.contains("://") {
        return None;
    } else {
        url
    };
    let path = Path::new(raw);
    if path.join("objects").is_dir() {
        Some(path.to_path_buf())
    } else {
        // Working repo (or absent — fetch_local then errors clearly).
        Some(path.join(".git"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(protocol: &str, host: &str) -> Credential {
        Credential {
            protocol: Some(protocol.to_owned()),
            host: Some(host.to_owned()),
            ..Credential::parse("")
        }
    }

    #[test]
    fn wraps_token_from_secrets_map() {
        let mut secrets = BTreeMap::new();
        secrets.insert(GITHUB_TOKEN_ENV.to_owned(), "ghp_test".to_owned());
        assert!(Credentials::from_secrets(&secrets).has_github_token());
    }

    #[test]
    fn github_https_host_detection() {
        assert!(is_github_https(&cred("https", "github.com")));
        assert!(is_github_https(&cred("https", "www.github.com")));
        assert!(is_github_https(&cred("https", "github.com:443")));
        assert!(!is_github_https(&cred("https", "gitlab.com")));
        assert!(!is_github_https(&cred("http", "github.com")));
    }

    #[test]
    fn non_local_scheme_has_no_local_path() {
        assert!(local_repo_path("https://github.com/o/r").is_none());
        assert!(local_repo_path("ssh://git@host/r").is_none());
        assert_eq!(
            local_repo_path("file:///tmp/x"),
            Some(PathBuf::from("/tmp/x/.git"))
        );
    }
}
