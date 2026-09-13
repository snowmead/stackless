//! Cross-process file locks keyed by path (ARCHITECTURE.md §2/§3).
//!
//! The OS releases a lock when its file handle closes or its process exits.
//! Lock files remain in place: unlinking a held lock would allow another
//! process to lock a different inode at the same path.

use std::fs::{File, TryLockError};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::state::Store;

const DEFAULT_POLL: Duration = Duration::from_millis(100);

/// A held lock; released when dropped.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // A concurrent fork can briefly inherit this open file description.
        // Unlock explicitly so that child cannot delay a normal release.
        let _ = self._file.unlock();
    }
}

impl FileLock {
    /// Try once; returns [`LockError::Held`] if a live holder has the lock.
    pub fn try_acquire(path: &Path) -> Result<Self, LockError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| LockError::CreateParent {
                path: path.to_path_buf(),
                detail: err.to_string(),
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|source| LockError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(LockError::Held {
                path: path.to_path_buf(),
            }),
            Err(TryLockError::Error(source)) => Err(LockError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Wait for contention only. Filesystem errors return immediately.
    pub fn acquire_with_wait(path: &Path, budget: Duration) -> Result<Self, LockError> {
        let start = Instant::now();
        loop {
            match Self::try_acquire(path) {
                Ok(lock) => return Ok(lock),
                Err(LockError::Held { .. }) if start.elapsed() < budget => {
                    std::thread::sleep(DEFAULT_POLL);
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Wait for an existing command lock without creating one for an unused runtime.
    pub fn acquire_existing(path: &Path, budget: Duration) -> Result<Option<Self>, LockError> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => Self::acquire_with_wait(path, budget).map(Some),
            Ok(_) => Err(LockError::Io {
                path: path.into(),
                source: std::io::Error::other("lock is not an ordinary file"),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(LockError::Io {
                path: path.into(),
                source,
            }),
        }
    }

    /// Filesystem-safe digest of a path for lock file names.
    pub fn path_key(path: &Path) -> String {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        canonical.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// `{state_dir}/locks/stripe/{digest}.lock` for a definition dir.
    pub fn stripe_lock_path(definition_dir: &Path) -> PathBuf {
        Store::state_dir()
            .join("locks/stripe")
            .join(format!("{}.lock", Self::path_key(definition_dir)))
    }

    /// `{state_dir}/locks/git-cache/{cache_key}.lock`.
    pub fn git_cache_lock_path(cache_key: &str) -> PathBuf {
        Store::state_dir()
            .join("locks/git-cache")
            .join(format!("{cache_key}.lock"))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("lock {path} is held by a live process")]
    Held { path: PathBuf },
    #[error("could not create lock parent for {path}: {detail}")]
    CreateParent { path: PathBuf, detail: String },
    #[error("cannot lock {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn second_holder_blocks_until_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.lock");

        let lock = FileLock::try_acquire(&path).unwrap();
        assert!(matches!(
            FileLock::try_acquire(&path),
            Err(LockError::Held { .. })
        ));
        drop(lock);
        assert!(FileLock::try_acquire(&path).is_ok());
    }

    #[test]
    fn concurrent_waiters_serialize() {
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let n = 4;
        let start = Arc::new(Barrier::new(n));
        let mut handles = Vec::new();
        for _ in 0..n {
            let dir = Arc::clone(&dir);
            let start = Arc::clone(&start);
            handles.push(thread::spawn(move || {
                start.wait();
                let _lock = FileLock::acquire_with_wait(
                    &dir.path().join("queue.lock"),
                    Duration::from_secs(5),
                )
                .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
    }
    #[test]
    fn lock_crash_helper() {
        let Some(path) = std::env::var_os("STACKLESS_TEST_LOCK_FILE") else {
            return;
        };
        let path = PathBuf::from(path);
        let _lock = FileLock::try_acquire(&path).unwrap();
        std::fs::write(path.with_extension("ready"), "locked").unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }

    #[test]
    fn process_crash_releases_os_lock_without_unlinking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.lock");
        let mut holder = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lockfile::tests::lock_crash_helper"])
            .env("STACKLESS_TEST_LOCK_FILE", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.with_extension("ready").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let ready = path.with_extension("ready").exists();
        let excluded = matches!(FileLock::try_acquire(&path), Err(LockError::Held { .. }));
        holder.kill().unwrap();
        holder.wait().unwrap();
        assert!(ready);
        assert!(excluded);
        assert!(path.exists());
        assert!(FileLock::try_acquire(&path).is_ok());
    }
}
