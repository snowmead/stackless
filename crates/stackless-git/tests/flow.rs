//! End-to-end coverage of the grit-lib primitives against a throwaway on-disk
//! repo: bare cache fetch from a `file://` URL, ref resolution, and a thin
//! instance checkout that borrows objects from the cache via alternates. The
//! fixture repo is built with grit-lib too — no `git` CLI dependency.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stackless_git::{Credentials, build_repo, checkout_detached, fetch_bare, resolve_commit};

const CACHE_REFSPECS: &[&str] = &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"];

/// Two-commit source-repo fixture (cumulative file sets per commit).
const COMMIT_1: &[(&str, &str)] = &[
    ("README.md", "hello\n"),
    ("Cargo.toml", "[package]\nname=\"x\"\n"),
];
const COMMIT_2: &[(&str, &str)] = &[
    ("README.md", "hello\n"),
    ("Cargo.toml", "[package]\nname=\"x\"\n"),
    ("second.txt", "more\n"),
];

#[test]
fn fetch_resolve_and_checkout_through_alternates() {
    let work = tempfile::tempdir().unwrap();
    let repo = work.path().join("src-repo");
    let head = build_repo(&repo, &[COMMIT_1, COMMIT_2]).expect("build fixture repo");
    let url = format!("file://{}", repo.display());

    let cache = work.path().join("cache");
    fetch_bare(&cache, &url, CACHE_REFSPECS, None, &Credentials::default()).expect("fetch bare");
    assert!(cache.join("objects").is_dir(), "bare cache initialized");

    let commit = resolve_commit(&cache, "main").expect("resolve main");
    assert_eq!(commit, head, "resolved the pinned commit");

    let dest = work.path().join("checkout");
    checkout_detached(&dest, &cache, &commit).expect("checkout");

    assert_eq!(
        std::fs::read_to_string(dest.join("README.md")).unwrap(),
        "hello\n"
    );
    assert!(dest.join("second.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dest.join(".git/HEAD"))
            .unwrap()
            .trim(),
        head,
        "detached HEAD at the pinned commit"
    );
    // The instance borrows objects from the cache rather than duplicating them.
    assert!(
        dest.join(".git/objects/info/alternates").is_file(),
        "instance points at the cache via alternates"
    );

    // A bad ref is an error, not a panic.
    assert!(resolve_commit(&cache, "no-such-ref").is_err());
}

/// HTTPS path against a small public repo. Ignored by default (network).
#[test]
#[ignore = "requires network access to github.com"]
fn https_fetch_and_checkout() {
    let work = tempfile::tempdir().unwrap();
    let cache = work.path().join("cache");
    fetch_bare(
        &cache,
        "https://github.com/octocat/Hello-World",
        CACHE_REFSPECS,
        None,
        &Credentials::default(),
    )
    .expect("fetch over https");
    let commit = resolve_commit(&cache, "master").expect("resolve master");
    assert_eq!(commit.len(), 40, "resolved a full commit sha");

    let dest = work.path().join("checkout");
    checkout_detached(&dest, &cache, &commit).expect("checkout");
    assert!(dest.join("README").exists(), "checked out the repo");
}
