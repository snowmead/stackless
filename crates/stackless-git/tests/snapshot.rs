//! Dirty worktree snapshot coverage.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use stackless_git::snapshot_worktree;

const COMMIT_1: &[(&str, &str)] = &[("README.md", "hello\n")];

#[test]
fn snapshot_includes_modified_untracked_and_excludes_deleted() {
    let src = tempfile::tempdir().unwrap();
    stackless_git::build_repo(src.path(), &[COMMIT_1]).expect("fixture repo");

    std::fs::write(src.path().join("README.md"), "dirty\n").unwrap();
    std::fs::write(src.path().join("new.txt"), "fresh\n").unwrap();

    let dest = tempfile::tempdir().unwrap();
    let first = snapshot_worktree(dest.path(), src.path()).expect("first snapshot");
    assert_eq!(
        std::fs::read_to_string(dest.path().join("README.md")).unwrap(),
        "dirty\n"
    );
    assert_eq!(
        std::fs::read_to_string(dest.path().join("new.txt")).unwrap(),
        "fresh\n"
    );

    let second = snapshot_worktree(dest.path(), src.path()).expect("re-snapshot");
    assert_eq!(first, second, "unchanged tree is a no-op");
}

#[test]
fn snapshot_respects_gitignore() {
    let src = tempfile::tempdir().unwrap();
    stackless_git::build_repo(src.path(), &[COMMIT_1]).expect("fixture repo");
    std::fs::write(src.path().join(".gitignore"), "ignored/\n").unwrap();
    std::fs::create_dir_all(src.path().join("ignored")).unwrap();
    std::fs::write(src.path().join("ignored/secret.txt"), "nope\n").unwrap();
    std::fs::write(src.path().join("tracked-new.txt"), "yes\n").unwrap();

    let dest = tempfile::tempdir().unwrap();
    snapshot_worktree(dest.path(), src.path()).expect("snapshot");
    assert!(dest.path().join("tracked-new.txt").exists());
    assert!(!dest.path().join("ignored/secret.txt").exists());
}

#[test]
fn snapshot_is_deterministic_for_same_content() {
    let src = tempfile::tempdir().unwrap();
    stackless_git::build_repo(src.path(), &[COMMIT_1]).expect("fixture repo");
    std::fs::write(src.path().join("README.md"), "same\n").unwrap();

    let dest_a = tempfile::tempdir().unwrap();
    let dest_b = tempfile::tempdir().unwrap();
    let hash_a = snapshot_worktree(dest_a.path(), src.path()).unwrap();
    let hash_b = snapshot_worktree(dest_b.path(), src.path()).unwrap();
    assert_eq!(hash_a, hash_b);
}

#[test]
fn snapshot_works_on_linked_worktree() {
    let main = tempfile::tempdir().unwrap();
    stackless_git::build_repo(main.path(), &[COMMIT_1]).expect("fixture repo");

    let wt = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            "linked",
            wt.path().to_str().unwrap(),
        ])
        .current_dir(main.path())
        .status()
        .expect("git worktree add");
    assert!(status.success(), "git worktree add failed");

    std::fs::write(wt.path().join("README.md"), "worktree dirty\n").unwrap();

    let dest = tempfile::tempdir().unwrap();
    snapshot_worktree(dest.path(), wt.path()).expect("snapshot linked worktree");
    assert_eq!(
        std::fs::read_to_string(dest.path().join("README.md")).unwrap(),
        "worktree dirty\n"
    );
}

#[test]
fn snapshot_rejects_non_repo() {
    let src = tempfile::tempdir().unwrap();
    std::fs::write(src.path().join("README.md"), "nope\n").unwrap();
    let dest = tempfile::tempdir().unwrap();
    let err = snapshot_worktree(dest.path(), src.path()).unwrap_err();
    assert!(err.to_string().contains("not a git working tree"));
}
