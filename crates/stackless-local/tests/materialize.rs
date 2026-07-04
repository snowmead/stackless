//! Integration coverage for grit-lib source materialization (ARCHITECTURE.md
//! §8): cache clone, alternates checkout, cache reuse across instances,
//! and refresh-to-pinned-commit. The source repo is built with grit-lib
//! so nothing leaves the machine; the HTTPS public-repo path is a
//! separate `#[ignore]`d test below. The `state_root` seam injects a
//! tempdir so the real `~/.local/state` is never touched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stackless_core::fault::Fault;
use stackless_local::materialize;

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
fn materialize_cache_checkout_reuse_and_refresh() {
    let state = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let root = state.path();

    let repo = work.path().join("src-repo");
    let head = stackless_git::build_repo(&repo, &[COMMIT_1, COMMIT_2]).expect("build fixture repo");
    let url = format!("file://{}", repo.display());

    // (a) cache clone from a local path URL, (b) checkout + detached HEAD.
    let (dest, commit) = materialize::Materializer::new(root)
        .materialize("inst-a", "svc", &url, "main")
        .expect("first materialize");
    assert_eq!(commit, head, "resolved the pinned commit");
    assert!(dest.join("README.md").exists(), "checkout produced files");
    assert!(dest.join("second.txt").exists());
    let head_file = std::fs::read_to_string(dest.join(".git/HEAD")).unwrap();
    assert_eq!(head_file.trim(), head, "detached HEAD at the pinned commit");
    assert!(
        materialize::observe(&dest, &commit),
        "observe sees the fresh checkout as Present"
    );

    // The shared cache exists and is per-URL, not per-instance.
    let cache_root = root.join("cache/git");
    let caches: Vec<_> = std::fs::read_dir(&cache_root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(caches.len(), 1, "exactly one cache for the one URL");
    let cache = caches[0].clone();
    let cache_before = std::fs::metadata(cache.join("objects"))
        .unwrap()
        .modified()
        .unwrap();

    // (c) a second instance reuses the same cache — no re-clone.
    let (dest_b, commit_b) = materialize::Materializer::new(root)
        .materialize("inst-b", "svc", &url, "main")
        .expect("second materialize");
    assert_eq!(commit_b, head);
    assert!(dest_b.join("README.md").exists());
    let caches_after: Vec<_> = std::fs::read_dir(&cache_root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(caches_after.len(), 1, "still one cache: it was reused");
    assert_eq!(caches_after[0], cache, "the same cache dir was used");
    let cache_after = std::fs::metadata(cache.join("objects"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        cache_before, cache_after,
        "the cache objects dir was not re-created"
    );
    assert_ne!(dest, dest_b, "instances own separate checkouts");

    // (d) refresh-to-pinned-commit after the worktree was dirtied.
    std::fs::write(dest.join("README.md"), "DIRTY\n").unwrap();
    std::fs::remove_file(dest.join("second.txt")).unwrap();
    let (dest_again, commit_again) = materialize::Materializer::new(root)
        .materialize("inst-a", "svc", &url, "main")
        .expect("re-materialize");
    assert_eq!(dest_again, dest);
    assert_eq!(commit_again, head);
    assert_eq!(
        std::fs::read_to_string(dest.join("README.md")).unwrap(),
        "hello\n",
        "dirtied file restored to the pinned content"
    );
    assert!(
        dest.join("second.txt").exists(),
        "deleted file restored by the rebuild"
    );

    // destroy removes the instance checkout; the shared cache stays.
    materialize::destroy(&dest).unwrap();
    assert!(!dest.exists(), "checkout removed");
    assert!(!materialize::observe(&dest, &commit), "now observes Gone");
    assert!(cache.join("objects").is_dir(), "cache survives teardown");
    materialize::destroy(&dest).unwrap(); // idempotent

    // A bad ref surfaces as a ref-not-found fault, not a panic.
    let err = materialize::Materializer::new(root)
        .materialize("inst-c", "svc", &url, "no-such-ref")
        .unwrap_err();
    assert_eq!(err.code(), "local.git.ref_not_found");
}

#[test]
fn dirty_snapshot_writes_instance_owned_tree_and_teardown_removes_it() {
    let state = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let head = stackless_git::build_repo(src.path(), &[COMMIT_1]).expect("fixture repo");
    std::fs::write(src.path().join("README.md"), "dirty\n").unwrap();

    let dest = state.path().join("sources/inst-a/api");
    let commit = stackless_git::snapshot_worktree(&dest, src.path()).expect("snapshot");
    assert_ne!(commit, head);
    assert_eq!(
        std::fs::read_to_string(dest.join("README.md")).unwrap(),
        "dirty\n"
    );
    assert!(materialize::observe(&dest, &commit));

    materialize::destroy(&dest).unwrap();
    assert!(!dest.exists());
}

/// HTTPS path against a small public repo. Ignored by default (network);
/// run with `cargo test -p stackless-local -- --ignored`.
#[test]
#[ignore = "requires network access to github.com"]
fn materialize_public_https() {
    let state = tempfile::tempdir().unwrap();
    let (dest, commit) = materialize::Materializer::new(state.path())
        .materialize(
            "inst-https",
            "hello",
            "https://github.com/octocat/Hello-World",
            "master",
        )
        .expect("clone a public repo over HTTPS");
    assert_eq!(commit.len(), 40, "resolved a full commit sha");
    assert!(dest.join("README").exists(), "checked out the repo");
}
