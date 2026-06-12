//! Cross-process file locks keyed by path (ARCHITECTURE.md §2/§3).
//!
//! `create_new` with stale-holder detection by PID + start time — the
//! same liveness identity op locks use. Daemon spawn, Stripe Projects,
//! and git cache writers share this helper.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::process::ProcessStamp;
use crate::state::Store;
use crate::types::{Pid, ProcessStartTime};

const DEFAULT_POLL: Duration = Duration::from_millis(100);

/// A held lock; released when dropped.
#[derive(Debug)]
pub struct FileLock {
    path: PathBuf,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut file) => {
                    let me = ProcessStamp::current();
                    let _ = writeln!(file, "{} {}", me.pid.get(), me.start_time.get());
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(_) => {
                    let stale = std::fs::read_to_string(path)
                        .ok()
                        .and_then(|content| {
                            let mut parts = content.split_whitespace();
                            let pid = parts.next()?.parse().ok()?;
                            let start_time = parts.next()?.parse().ok()?;
                            Some(ProcessStamp {
                                pid: Pid::from_os(pid),
                                start_time: ProcessStartTime::from_os(start_time),
                            })
                        })
                        .is_none_or(|stamp| !stamp.is_alive());
                    if stale {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    return Err(LockError::Held {
                        path: path.to_path_buf(),
                    });
                }
            }
        }
        Err(LockError::Held {
            path: path.to_path_buf(),
        })
    }

    /// Block until the lock is acquired, a stale holder is taken over, or
    /// `budget` elapses.
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
}
