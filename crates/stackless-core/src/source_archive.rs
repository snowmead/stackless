//! Bounded source transfer. Every filesystem lookup stays relative to an open directory.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Dir, FileType, Mode, OFlags, fstat, mkdirat, open, openat};
use serde::{Deserialize, Serialize};

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 100_000;

#[derive(Debug, thiserror::Error)]
#[error("invalid source archive: {0}")]
pub struct ArchiveError(String);

fn invalid(detail: impl Into<String>) -> ArchiveError {
    ArchiveError(detail.into())
}
fn io(error: impl std::fmt::Display) -> ArchiveError {
    invalid(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchive {
    pub directories: Vec<String>,
    pub files: Vec<SourceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFile {
    pub path: String,
    pub executable: bool,
    /// Base64 keeps binary source files within a bounded JSON frame.
    pub contents: String,
}

pub fn excluded(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".stackless-builds"
            | ".stackless-commands"
            | ".projects"
            | ".stackless.env"
            | ".ssh"
            | ".aws"
            | ".azure"
            | ".config"
            | ".docker"
            | ".codex"
            | ".agents"
            | ".netrc"
            | ".vercel-token"
            | ".render-api-key"
            | ".cloudflare-api-token"
            | ".railway-token"
            | ".wordpress-com-token"
            | ".laravel-cloud-token"
            | ".gitlab-token"
    ) || name == ".env"
        || name.starts_with(".env.")
}

fn components(path: &str) -> Result<Vec<&str>, ArchiveError> {
    let parts: Vec<_> = path.split('/').collect();
    if path.is_empty()
        || path.len() > 4096
        || parts.len() > 100
        || path.contains('\\')
        || path.contains('\0')
        || parts
            .iter()
            .any(|part| part.is_empty() || matches!(*part, "." | "..") || excluded(part))
    {
        return Err(invalid(
            "source path is absolute, protected, or escapes its root",
        ));
    }
    Ok(parts)
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

impl SourceArchive {
    pub fn capture(path: &Path) -> Result<Self, ArchiveError> {
        let canonical = path.canonicalize().map_err(io)?;
        if canonical.components().any(|component| matches!(component, Component::Normal(name) if excluded(&name.to_string_lossy()))) {
            return Err(invalid("source root is a protected directory"));
        }
        let fd = open(&canonical, directory_flags(), Mode::empty()).map_err(io)?;
        Self::read_directory(&fd)
    }

    /// Resolve a configured source root through directory descriptors. A checkout
    /// cannot redirect the upload to an absolute path, parent, or symlink target.
    pub fn capture_beneath(root: &Path, relative: &Path) -> Result<Self, ArchiveError> {
        let mut fd = open(root, directory_flags(), Mode::empty()).map_err(io)?;
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(name) if !excluded(&name.to_string_lossy()) => {
                    fd = openat(&fd, name, directory_flags(), Mode::empty()).map_err(io)?;
                }
                _ => {
                    return Err(invalid(
                        "source root escapes its checkout or names a protected directory",
                    ));
                }
            }
        }
        Self::read_directory(&fd)
    }

    fn read_directory(fd: &OwnedFd) -> Result<Self, ArchiveError> {
        let mut archive = Self {
            directories: vec![],
            files: vec![],
        };
        let mut bytes = 0;
        archive.walk(fd, "", &mut bytes)?;
        archive.directories.sort();
        archive.files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(archive)
    }

    fn walk(
        &mut self,
        parent: &OwnedFd,
        prefix: &str,
        bytes: &mut usize,
    ) -> Result<(), ArchiveError> {
        for entry in Dir::read_from(parent).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| invalid("source filename is not UTF-8"))?;
            if matches!(name, "." | "..") || excluded(name) {
                continue;
            }
            if self.directories.len() + self.files.len() >= MAX_ENTRIES {
                return Err(invalid("source exceeds 100000 entries"));
            }
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            components(&path)?;
            // NONBLOCK prevents a substituted FIFO from hanging capture. NOFOLLOW rejects links.
            let fd = openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io)?;
            let stat = fstat(&fd).map_err(io)?;
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => {
                    self.directories.push(path.clone());
                    self.walk(&fd, &path, bytes)?;
                }
                FileType::RegularFile => {
                    let mut contents = Vec::new();
                    File::from(fd)
                        .take((MAX_BYTES - *bytes + 1) as u64)
                        .read_to_end(&mut contents)
                        .map_err(io)?;
                    *bytes += contents.len();
                    if *bytes > MAX_BYTES {
                        return Err(invalid("source exceeds 64 MiB"));
                    }
                    self.files.push(SourceFile {
                        path,
                        executable: stat.st_mode & 0o111 != 0,
                        contents: STANDARD.encode(contents),
                    });
                }
                _ => return Err(invalid("source contains a link or special file")),
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<usize, ArchiveError> {
        if self.files.len() + self.directories.len() > MAX_ENTRIES {
            return Err(invalid("source exceeds 100000 entries"));
        }
        let mut seen = BTreeSet::new();
        let mut bytes = 0usize;
        for path in self
            .directories
            .iter()
            .chain(self.files.iter().map(|file| &file.path))
        {
            components(path)?;
            if !seen.insert(path) {
                return Err(invalid("source contains duplicate paths"));
            }
        }
        for file in &self.files {
            if file.contents.len() > MAX_BYTES.div_ceil(3) * 4 {
                return Err(invalid("source exceeds 64 MiB"));
            }
            bytes = bytes.saturating_add(
                STANDARD
                    .decode(&file.contents)
                    .map_err(|_| invalid("source file is not valid base64"))?
                    .len(),
            );
            if bytes > MAX_BYTES {
                return Err(invalid("source exceeds 64 MiB"));
            }
        }
        Ok(bytes)
    }

    /// Extract only into a newly created staging directory. Publish the directory atomically.
    pub fn extract(&self, root: &Path) -> Result<(), ArchiveError> {
        self.validate()?;
        let fd = open(root, directory_flags(), Mode::empty()).map_err(io)?;
        for path in &self.directories {
            descend(&fd, &components(path)?)?;
        }
        for file in &self.files {
            let parts = components(&file.path)?;
            let (name, parents) = parts
                .split_last()
                .ok_or_else(|| invalid("empty source path"))?;
            let parent = descend(&fd, parents)?;
            let mode = if file.executable {
                Mode::from_raw_mode(0o700)
            } else {
                Mode::from_raw_mode(0o600)
            };
            let output = openat(
                &parent,
                *name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                mode,
            )
            .map_err(io)?;
            let mut output = File::from(output);
            output
                .write_all(
                    &STANDARD
                        .decode(&file.contents)
                        .map_err(|_| invalid("invalid base64"))?,
                )
                .map_err(io)?;
            output.sync_all().map_err(io)?;
        }
        Ok(())
    }
}

fn descend(parent: &impl AsFd, parts: &[&str]) -> Result<OwnedFd, ArchiveError> {
    let mut fd = openat(parent, ".", directory_flags(), Mode::empty()).map_err(io)?;
    for name in parts {
        match mkdirat(&fd, *name, Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(rustix::io::Errno::EXIST) => (),
            Err(error) => return Err(io(error)),
        }
        fd = openat(&fd, *name, directory_flags(), Mode::empty()).map_err(io)?;
    }
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn capture_and_extract_preserve_files_but_exclude_credentials() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let output = root.path().join("output");
        std::fs::create_dir_all(source.join("empty")).unwrap();
        std::fs::create_dir(&output).unwrap();
        std::fs::write(source.join("app"), [0, 255, 128]).unwrap();
        std::fs::set_permissions(source.join("app"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        for name in [
            ".env",
            ".env.local",
            ".stackless.env",
            ".netrc",
            ".vercel-token",
            ".render-api-key",
            ".cloudflare-api-token",
            ".railway-token",
            ".wordpress-com-token",
            ".laravel-cloud-token",
            ".gitlab-token",
        ] {
            std::fs::write(source.join(name), "credential-canary").unwrap();
        }
        std::fs::create_dir(source.join(".stackless-builds")).unwrap();
        std::fs::write(
            source.join(".stackless-builds/fly.toml"),
            "APP_SECRET=private",
        )
        .unwrap();
        std::fs::create_dir(source.join(".stackless-commands")).unwrap();
        std::fs::write(
            source.join(".stackless-commands/output"),
            "prepare-secret-canary",
        )
        .unwrap();
        let archive = SourceArchive::capture(&source).unwrap();
        assert_eq!(archive.files.len(), 1);
        archive.extract(&output).unwrap();
        assert_eq!(std::fs::read(output.join("app")).unwrap(), [0, 255, 128]);
        assert!(output.join("empty").is_dir());
        assert_eq!(
            std::fs::metadata(output.join("app"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn links_and_path_traversal_cannot_read_or_write_outside_the_source() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let outside = root.path().join("outside");
        std::fs::write(&outside, "private-canary").unwrap();
        symlink(&outside, source.join("app")).unwrap();
        assert!(SourceArchive::capture(&source).is_err());
        for path in [
            "../outside",
            "/outside",
            "a/../../outside",
            ".ssh/key",
            "a/.env",
            "a\\outside",
        ] {
            let archive = SourceArchive {
                directories: vec![],
                files: vec![SourceFile {
                    path: path.into(),
                    executable: false,
                    contents: STANDARD.encode("overwrite"),
                }],
            };
            assert!(archive.extract(&source).is_err());
        }
        let archive = SourceArchive {
            directories: vec![],
            files: vec![SourceFile {
                path: "app".into(),
                executable: false,
                contents: STANDARD.encode("overwrite"),
            }],
        };
        assert!(archive.extract(&source).is_err());
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "private-canary");
    }
    #[test]
    fn configured_root_cannot_escape_checkout_or_follow_directory_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("checkout");
        let outside = dir.path().join("private");
        std::fs::create_dir_all(checkout.join("app")).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), "private-canary").unwrap();
        std::fs::write(checkout.join("app/index.html"), "app").unwrap();
        symlink(&outside, checkout.join("redirect")).unwrap();
        for relative in [
            Path::new("../private"),
            outside.as_path(),
            Path::new("redirect"),
            Path::new("redirect/subdir"),
        ] {
            assert!(SourceArchive::capture_beneath(&checkout, relative).is_err());
        }
        let archive = SourceArchive::capture_beneath(&checkout, Path::new("./app")).unwrap();
        assert_eq!(archive.files.len(), 1);
        assert_eq!(archive.files[0].path, "index.html");
        assert_eq!(STANDARD.decode(&archive.files[0].contents).unwrap(), b"app");
    }
}
