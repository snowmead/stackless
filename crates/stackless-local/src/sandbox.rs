//! Copy application files into a separate container workspace. Never mount the caller's checkout.

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase};
use stackless_core::substrate::{StepContext, StepResource, SubstrateFault};

use crate::{LocalSubstrate, MaterializePayload, SUBSTRATE_NAME};

pub const WORKSPACE_KIND: &str = "sandbox-workspace";
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_FILES: usize = 100_000;

fn fault(message: impl Into<String>) -> SubstrateFault {
    SubstrateFault { code: "sandbox.source_invalid".into(), message: message.into(),
        remediation: "use an application source directory containing ordinary files and no credential files or links".into(), context: Box::default() }
}

fn excluded(name: &str) -> bool {
    stackless_core::source_archive::excluded(name)
}

fn copy_tree(
    source: &Path,
    dest: &Path,
    files: &mut usize,
    bytes: &mut u64,
    depth: usize,
) -> Result<(), SubstrateFault> {
    if depth > 100 {
        return Err(fault("source directory nesting exceeds 100 levels"));
    }
    fs::create_dir_all(dest).map_err(|error| fault(error.to_string()))?;
    fs::set_permissions(dest, fs::Permissions::from_mode(0o777))
        .map_err(|error| fault(error.to_string()))?;
    for entry in fs::read_dir(source).map_err(|error| fault(error.to_string()))? {
        let entry = entry.map_err(|error| fault(error.to_string()))?;
        if excluded(&entry.file_name().to_string_lossy()) {
            continue;
        }
        *files += 1;
        if *files > MAX_FILES {
            return Err(fault("source tree exceeds 100000 entries"));
        }
        let metadata = entry
            .file_type()
            .map_err(|error| fault(error.to_string()))?;
        let destination = dest.join(entry.file_name());
        if metadata.is_dir() {
            copy_tree(&entry.path(), &destination, files, bytes, depth + 1)?;
        } else if metadata.is_file() {
            let mut input = fs::OpenOptions::new()
                .read(true)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                .open(entry.path())
                .map_err(|error| fault(error.to_string()))?;
            let stat = input.metadata().map_err(|error| fault(error.to_string()))?;
            *bytes = bytes.saturating_add(stat.len());
            if *bytes > MAX_BYTES {
                return Err(fault("source tree exceeds 512 MiB"));
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if stat.permissions().mode() & 0o111 != 0 {
                    0o755
                } else {
                    0o644
                })
                .open(&destination)
                .map_err(|error| fault(error.to_string()))?;
            std::io::copy(&mut input, &mut output).map_err(|error| fault(error.to_string()))?;
            output
                .set_permissions(fs::Permissions::from_mode(
                    if stat.permissions().mode() & 0o111 != 0 {
                        0o777
                    } else {
                        0o666
                    },
                ))
                .map_err(|error| fault(error.to_string()))?;
        } else {
            return Err(fault(format!(
                "source entry {} is a link or special file",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

impl LocalSubstrate {
    pub(crate) fn sandbox_source(
        &self,
        ctx: &StepContext<'_>,
        source: StepResource,
    ) -> Result<StepResource, SubstrateFault> {
        let original: MaterializePayload =
            serde_json::from_str(&source.payload).map_err(|error| fault(error.to_string()))?;
        let mut base =
            fs::canonicalize(&original.path).map_err(|error| fault(error.to_string()))?;
        if base
            .components()
            .any(|component| excluded(&component.as_os_str().to_string_lossy()))
        {
            return Err(fault("container source cannot be a credential directory"));
        }
        if let Some(root) = &ctx.def.services[&ctx.step.node].source.root {
            if Path::new(root)
                .components()
                .any(|component| excluded(&component.as_os_str().to_string_lossy()))
            {
                return Err(fault(
                    "container source root cannot select credential files",
                ));
            }
            let resolved =
                fs::canonicalize(base.join(root)).map_err(|error| fault(error.to_string()))?;
            if !resolved.starts_with(&base) {
                return Err(fault("source root escapes the source tree"));
            }
            base = resolved;
        }
        let hash = stackless_core::engine::revision::digest(&(
            ctx.operation_id,
            &ctx.step.id,
            &source.payload,
        ))?;
        let dest = self
            .state_root
            .join("sandbox")
            .join(ctx.instance.resource_namespace)
            .join(&hash);
        let key = format!("workspace:{hash}");
        let payload = MaterializePayload {
            path: dest.display().to_string(),
            root_applied: true,
            overridden: false,
            commit: original.commit,
        };
        let serialized =
            serde_json::to_string(&payload).map_err(|error| fault(error.to_string()))?;
        let state_fault =
            |error: stackless_core::state::StateError| SubstrateFault::from_fault(&error);
        let record = ctx
            .store
            .resource_intent(ResourceIntent {
                owner_id: ctx.instance.id,
                key: &key,
                step_id: &ctx.step.id,
                provider: SUBSTRATE_NAME,
                ownership: Ownership::Owned,
                resource_kind: WORKSPACE_KIND,
                resource_id: &payload.path,
                payload: &serialized,
                dependencies: ctx.parent_resources,
            })
            .map_err(state_fault)?;
        if record.phase == ResourcePhase::Intent {
            if dest
                .try_exists()
                .map_err(|error| fault(error.to_string()))?
            {
                fs::remove_dir_all(&dest).map_err(|error| fault(error.to_string()))?;
            }
            if let Some(parent) = dest.parent() {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .map_err(|error| fault(error.to_string()))?;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                    .map_err(|error| fault(error.to_string()))?;
            }
            copy_tree(&base, &dest, &mut 0, &mut 0, 0)?;
            ctx.store
                .resource_created(ctx.instance.id, &key, &payload.path, &serialized)
                .map_err(state_fault)?;
            ctx.store
                .resource_ready(ctx.instance.id, &key)
                .map_err(state_fault)?;
        }
        Ok(StepResource {
            resource_kind: WORKSPACE_KIND.into(),
            resource_id: payload.path,
            payload: serialized,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn copies_application_files_without_operator_credentials() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir_all(source.join(".projects")).unwrap();
        for name in ["app.py", ".env", ".stackless.env", ".projects/credentials"] {
            fs::write(source.join(name), "canary").unwrap();
        }
        let dest = root.path().join("dest");
        copy_tree(&source, &dest, &mut 0, &mut 0, 0).unwrap();
        assert!(dest.join("app.py").exists());
        assert!(!dest.join(".env").exists());
        assert!(!dest.join(".stackless.env").exists());
        assert!(!dest.join(".projects").exists());
    }
    #[test]
    fn refuses_links_to_host_files() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        let secret = root.path().join("operator-secret");
        fs::write(&secret, "canary").unwrap();
        std::os::unix::fs::symlink(&secret, source.join("app.py")).unwrap();
        let dest = root.path().join("dest");
        assert!(copy_tree(&source, &dest, &mut 0, &mut 0, 0).is_err());
        assert!(!dest.join("app.py").exists());
    }
}
