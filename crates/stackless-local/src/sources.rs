//! Pin source revisions per operation and retain every owned checkout until teardown.

use std::path::PathBuf;

use stackless_core::state::{Ownership, ResourceIntent, ResourcePhase};
use stackless_core::substrate::{StepContext, StepResource, Substrate, SubstrateFault};

use crate::{LocalSubstrate, MaterializePayload, SUBSTRATE_NAME, fault, materialize::Materializer};

fn source_fault(error: impl std::fmt::Display) -> SubstrateFault {
    SubstrateFault {
        code: "local.git.checkout_failed".into(),
        message: error.to_string(),
        remediation: "inspect the source checkout and retry the operation".into(),
        context: Box::default(),
    }
}

impl LocalSubstrate {
    pub(crate) async fn materialize_source(
        &self,
        ctx: &StepContext<'_>,
    ) -> Result<StepResource, SubstrateFault> {
        let service = &ctx.step.node;
        let spec = ctx
            .def
            .services
            .get(service)
            .ok_or_else(|| source_fault("source workload is missing"))?;
        let hash = stackless_core::engine::revision::digest(&(
            &ctx.step.id,
            ctx.operation_id,
            self.step_revision(ctx)?,
        ))?;
        let key = format!("source:{hash}");
        let state_fault =
            |error: stackless_core::state::StateError| SubstrateFault::from_fault(&error);
        let existing = ctx
            .store
            .resource(ctx.instance.id, &key)
            .map_err(state_fault)?;
        let (kind, mut payload, ownership) = if let Some(record) = &existing {
            let payload: MaterializePayload =
                serde_json::from_str(&record.payload).map_err(source_fault)?;
            if matches!(record.phase, ResourcePhase::Created | ResourcePhase::Ready) {
                return Ok(StepResource {
                    resource_kind: record.resource_kind.clone(),
                    resource_id: record.resource_id.clone(),
                    payload: record.payload.clone(),
                });
            }
            (record.resource_kind.as_str(), payload, record.ownership)
        } else if let Some(path) = ctx.source_overrides.get(service) {
            let canonical = std::fs::canonicalize(path).map_err(source_fault)?;
            if !canonical.is_dir() {
                return Err(source_fault("source path is not a directory"));
            }
            if ctx.dirty {
                let dest = Materializer::new(&self.state_root)
                    .source_dir(ctx.instance.resource_namespace, service)
                    .join("snapshots")
                    .join(&hash);
                (
                    "source-dirty",
                    MaterializePayload {
                        root_applied: false,
                        path: dest.display().to_string(),
                        overridden: true,
                        commit: None,
                    },
                    Ownership::Owned,
                )
            } else {
                (
                    "source-override",
                    MaterializePayload {
                        root_applied: false,
                        path: canonical.display().to_string(),
                        overridden: true,
                        commit: None,
                    },
                    Ownership::Borrowed,
                )
            }
        } else if spec.source.repo.is_empty() {
            let dest = Materializer::new(&self.state_root)
                .source_dir(ctx.instance.resource_namespace, service)
                .join("empty");
            (
                "source-empty",
                MaterializePayload {
                    root_applied: false,
                    path: dest.display().to_string(),
                    overridden: false,
                    commit: None,
                },
                Ownership::Owned,
            )
        } else {
            let root = self.state_root.clone();
            let service = service.clone();
            let repo = spec.source.repo.clone();
            let reference = spec.source.reference.clone();
            let secrets = self.secrets.clone();
            let commit = tokio::task::spawn_blocking(move || {
                Materializer::new(&root)
                    .with_auth(crate::git_auth::GitAuth::from_secrets(&secrets))
                    .resolve(&service, &repo, &reference)
            })
            .await
            .map_err(source_fault)?
            .map_err(fault)?;
            let dest = Materializer::new(&self.state_root)
                .source_dir(ctx.instance.resource_namespace, &ctx.step.node)
                .join(&commit);
            (
                "source",
                MaterializePayload {
                    root_applied: false,
                    path: dest.display().to_string(),
                    overridden: false,
                    commit: Some(commit),
                },
                Ownership::Owned,
            )
        };
        ctx.store
            .resource_intent(ResourceIntent {
                owner_id: ctx.instance.id,
                key: &key,
                step_id: &ctx.step.id,
                provider: SUBSTRATE_NAME,
                ownership,
                resource_kind: kind,
                resource_id: &payload.path,
                payload: &serde_json::to_string(&payload).map_err(source_fault)?,
                dependencies: ctx.parent_resources,
            })
            .map_err(state_fault)?;
        let dest = PathBuf::from(&payload.path);
        match kind {
            "source" => {
                let root = self.state_root.clone();
                let service = service.clone();
                let repo = spec.source.repo.clone();
                let commit = payload
                    .commit
                    .clone()
                    .ok_or_else(|| source_fault("git source has no pinned commit"))?;
                tokio::task::spawn_blocking(move || {
                    Materializer::new(&root).checkout(&service, &repo, &commit, &dest)
                })
                .await
                .map_err(source_fault)?
                .map_err(fault)?;
            }
            "source-dirty" => {
                let source = ctx
                    .source_overrides
                    .get(service)
                    .ok_or_else(|| source_fault("snapshot source pin is missing"))?
                    .clone();
                payload.commit = Some(
                    tokio::task::spawn_blocking(move || {
                        stackless_git::snapshot_worktree(&dest, std::path::Path::new(&source))
                    })
                    .await
                    .map_err(source_fault)?
                    .map_err(source_fault)?,
                );
            }
            "source-empty" => std::fs::create_dir_all(&dest).map_err(source_fault)?,
            "source-override" => (),
            _ => return Err(source_fault("unknown source inventory kind")),
        }
        let serialized = serde_json::to_string(&payload).map_err(source_fault)?;
        ctx.store
            .resource_created(ctx.instance.id, &key, &payload.path, &serialized)
            .map_err(state_fault)?;
        ctx.store
            .resource_ready(ctx.instance.id, &key)
            .map_err(state_fault)?;
        Ok(StepResource {
            resource_kind: kind.into(),
            resource_id: payload.path,
            payload: serialized,
        })
    }
}
