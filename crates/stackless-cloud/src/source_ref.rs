//! Operator-side source-ref checkout helpers (Render/Vercel verify checkouts).

use std::path::Path;

use stackless_core::substrate::SubstrateFault;

/// Whether a verify checkout at `path` still matches `commit` (`.git/HEAD`).
pub fn present(path: &str, commit: &str) -> bool {
    let path = Path::new(path);
    if !path.exists() {
        return false;
    }
    std::fs::read_to_string(path.join(".git/HEAD"))
        .map(|head| head.trim() == commit)
        .unwrap_or(false)
}

/// Remove a verify checkout directory. Missing path is success (idempotent).
pub fn destroy(path: &str) -> Result<(), SubstrateFault> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(SubstrateFault {
            code: stackless_core::fault::codes::LOCAL_GIT_CHECKOUT_FAILED,
            message: format!("cannot remove verify checkout {path}: {err}"),
            remediation: format!("remove {path} by hand, then re-run `stackless down`"),
            context: Box::default(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_requires_matching_head() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "abc\n").unwrap();
        assert!(present(dir.path().to_str().unwrap(), "abc"));
        assert!(!present(dir.path().to_str().unwrap(), "other"));
    }

    #[test]
    fn destroy_missing_is_ok() {
        destroy("/tmp/stackless-source-ref-missing-test-path").unwrap();
    }
}
