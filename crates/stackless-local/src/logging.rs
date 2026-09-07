//! A service runner with bounded logs. It runs outside the controller.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stackless_core::lockfile::FileLock;

pub const GENERATION_BYTES: u64 = 1024 * 1024;
pub const GENERATIONS: u32 = 3;
const INPUT_LIMIT: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Launch {
    pub log: PathBuf,
    pub command: String,
}

impl Launch {
    pub fn encode(&self) -> std::io::Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > INPUT_LIMIT {
            return Err(std::io::Error::other("service launch input exceeds 1 MiB"));
        }
        Ok(bytes)
    }
}

fn ordinary(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(()),
        Ok(_) => Err(std::io::Error::other("log path is not an ordinary file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn directory(root: &Path, instance: &str) -> std::io::Result<PathBuf> {
    if !stackless_core::types::dns_safe(instance) {
        return Err(std::io::Error::other("invalid log namespace"));
    }
    std::fs::create_dir_all(root)?;
    let mut path = root.canonicalize()?;
    for child in ["logs", instance] {
        path.push(child);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::other(
                    "log directory is not an ordinary directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new().mode(0o700).create(&path)?;
            }
            Err(error) => return Err(error),
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(path)
}

fn generation(path: &Path, index: u32) -> PathBuf {
    if index == 0 {
        path.to_owned()
    } else {
        path.with_extension(format!("log.{index}"))
    }
}

fn read_file(path: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("log is not an ordinary file"));
    }
    Ok(file)
}

struct RotatingLog {
    path: PathBuf,
    file: File,
    bytes: u64,
    _lock: FileLock,
}

impl RotatingLog {
    fn open(path: &Path) -> std::io::Result<Self> {
        let lock_path = path.with_extension("log.lock");
        ordinary(&lock_path)?;
        let lock = FileLock::acquire_with_wait(&lock_path, Duration::ZERO)
            .map_err(std::io::Error::other)?;
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))?;
        // Validate every destination before touching any existing generation.
        for index in 0..=GENERATIONS {
            ordinary(&generation(path, index))?;
        }
        for index in 0..GENERATIONS {
            let path = generation(path, index);
            let mut file = match read_file(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if file.metadata()?.len() > GENERATION_BYTES {
                file.seek(SeekFrom::End(-(GENERATION_BYTES as i64)))?;
                let mut tail = Vec::with_capacity(GENERATION_BYTES as usize);
                file.take(GENERATION_BYTES).read_to_end(&mut tail)?;
                stackless_core::security::private_log(&path, false)?.write_all(&tail)?;
            }
        }
        match std::fs::remove_file(generation(path, GENERATIONS)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let file = stackless_core::security::private_log(path, true)?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            path: path.to_owned(),
            file,
            bytes,
            _lock: lock,
        })
    }

    fn write(&mut self, mut bytes: &[u8]) -> std::io::Result<()> {
        while !bytes.is_empty() {
            if self.bytes == GENERATION_BYTES {
                for index in (0..GENERATIONS - 1).rev() {
                    let from = generation(&self.path, index);
                    let to = generation(&self.path, index + 1);
                    ordinary(&from)?;
                    ordinary(&to)?;
                    match std::fs::rename(from, to) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                self.file = stackless_core::security::private_log(&self.path, false)?;
                self.bytes = 0;
            }
            let count = bytes.len().min((GENERATION_BYTES - self.bytes) as usize);
            self.file.write_all(&bytes[..count])?;
            self.bytes += count as u64;
            bytes = &bytes[count..];
        }
        Ok(())
    }
}

/// Read a bounded suffix across rotations. Open descriptors survive concurrent renames.
pub(crate) fn tail(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut chunks = Vec::new();
    let mut remaining = limit;
    let mut seen = std::collections::HashSet::new();
    for index in 0..GENERATIONS {
        let mut file = match read_file(&generation(path, index)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let meta = file.metadata()?;
        if !seen.insert((meta.dev(), meta.ino())) {
            continue;
        }
        let count = meta.len().min(remaining as u64);
        file.seek(SeekFrom::Start(meta.len() - count))?;
        let mut bytes = Vec::with_capacity(count as usize);
        file.take(count).read_to_end(&mut bytes)?;
        remaining -= bytes.len();
        chunks.push(bytes);
        if remaining == 0 {
            break;
        }
    }
    Ok(chunks.into_iter().rev().flatten().collect())
}

/// Container log snapshots replace host logs. Remove obsolete host generations.
pub(crate) fn clear_generations(path: &Path) -> std::io::Result<()> {
    for index in 1..=GENERATIONS {
        ordinary(&generation(path, index))?;
    }
    for index in 1..=GENERATIONS {
        match std::fs::remove_file(generation(path, index)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn kill_group() {
    let pid = rustix::process::getpid();
    if rustix::process::getpgrp() == pid {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// Internal CLI entrypoint. Only a separately spawned process group may call this.
pub fn run() -> std::io::Result<()> {
    if rustix::process::getpgrp() != rustix::process::getpid() {
        return Err(std::io::Error::other(
            "service runner must own its process group",
        ));
    }
    let mut input = Vec::new();
    std::io::stdin()
        .lock()
        .take(INPUT_LIMIT + 1)
        .read_until(b'\n', &mut input)?;
    if input.len() as u64 > INPUT_LIMIT || input.last() != Some(&b'\n') {
        return Err(std::io::Error::other(
            "service launch gate closed or input exceeded limit",
        ));
    }
    let launch: Launch = serde_json::from_slice(&input)?;
    let mut log = RotatingLog::open(&launch.log)?;
    // Group signals still reach the service. Keep the collector alive to drain
    // shutdown output until the service exits or the controller escalates.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _entered = runtime.enter();
    let _term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let _interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let (mut reader, writer) = std::io::pipe()?;
    let flags = rustix::fs::fcntl_getfl(&reader)?;
    rustix::fs::fcntl_setfl(&reader, flags | rustix::fs::OFlags::NONBLOCK)?;
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", &launch.command])
        .stdin(std::process::Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .spawn()?;
    let done = Arc::new(AtomicBool::new(false));
    let finished = done.clone();
    let collector = std::thread::spawn(move || {
        let mut collect = || -> std::io::Result<()> {
            let mut buffer = [0; 8192];
            let mut drain_deadline = None;
            loop {
                if finished.load(Ordering::Acquire) {
                    let deadline = drain_deadline
                        .get_or_insert_with(|| std::time::Instant::now() + Duration::from_secs(2));
                    if std::time::Instant::now() >= *deadline {
                        return Ok(());
                    }
                }
                match reader.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(count) => log.write(&buffer[..count])?,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        if collect().is_err() {
            kill_group();
        }
    });
    let result = child.wait();
    done.store(true, Ordering::Release);
    let _ = collector.join();
    // A service exit ends the whole generation, including descendants holding output pipes.
    kill_group();
    result.map(|_| ())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn workload_helper() {
        // Reexecuted by Spawner unit tests with a pipe containing the committed launch.
        if rustix::process::getpgrp() == rustix::process::getpid() {
            let _ = run();
        }
    }

    #[test]
    fn rotation_retains_newest_bytes_and_short_live_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.log");
        let mut log = RotatingLog::open(&path).unwrap();
        for value in b'a'..=b'd' {
            log.write(&vec![value; GENERATION_BYTES as usize]).unwrap();
        }
        log.write(b"\nshort-live-output\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"\nshort-live-output\n");
        assert!(
            tail(&path, 64)
                .unwrap()
                .ends_with(b"dddd\nshort-live-output\n")
        );
        for index in 0..GENERATIONS {
            assert!(std::fs::metadata(generation(&path, index)).unwrap().len() <= GENERATION_BYTES);
        }
        assert!(!generation(&path, GENERATIONS).exists());
        assert!(RotatingLog::open(&path).is_err());
    }

    #[test]
    fn legacy_large_logs_are_capped_and_symlinks_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.log");
        std::fs::write(&path, vec![b'x'; 2 * GENERATION_BYTES as usize]).unwrap();
        std::fs::write(generation(&path, 3), b"old").unwrap();
        drop(RotatingLog::open(&path).unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), GENERATION_BYTES);
        assert!(!generation(&path, 3).exists());
        let foreign = dir.path().join("foreign");
        std::fs::write(&foreign, b"retained").unwrap();
        std::os::unix::fs::symlink(&foreign, generation(&path, 2)).unwrap();
        assert!(RotatingLog::open(&path).is_err());
        assert_eq!(std::fs::read(foreign).unwrap(), b"retained");
    }
}
