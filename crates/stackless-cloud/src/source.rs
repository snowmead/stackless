//! One durable Git or empty snapshot per operation. Hooks cannot change upload bytes.

use serde::{Deserialize, Serialize};
use stackless_core::{
    engine::revision::digest,
    source_archive::SourceArchive,
    state::{Checkpoint, Ownership, ResourceIntent, ResourcePhase},
    substrate::{InstanceContext, Observation, StepContext, StepResource, SubstrateFault},
};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Component, Path, PathBuf},
};

pub const KIND: &str = "source-snapshot";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    Git,
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub owner: String,
    pub operation: String,
    pub key: String,
    #[serde(default)]
    pub kind: SourceKind,
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub commit: Option<String>,
    pub digest: Option<String>,
    pub root: PathBuf,
    pub path: PathBuf,
}

fn fail(error: impl std::fmt::Display) -> SubstrateFault {
    SubstrateFault {
        code: "source.snapshot_failed".into(),
        message: error.to_string(),
        remediation:
            "inspect the owned source snapshot; do not replace a recorded commit during recovery"
                .into(),
        context: Box::default(),
    }
}
fn state(error: stackless_core::state::StateError) -> SubstrateFault {
    SubstrateFault::from_fault(&error)
}
fn private_dir(path: &Path) -> Result<(), SubstrateFault> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(fail)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(path).map_err(fail)?;
    Ok(())
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), SubstrateFault> {
    let parent = path
        .parent()
        .ok_or_else(|| fail("snapshot has no parent directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(fail)?;
    serde_json::to_writer(&mut file, value).map_err(fail)?;
    file.flush().map_err(fail)?;
    file.as_file().sync_all().map_err(fail)?;
    file.persist(path).map_err(fail)?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(fail)
}

impl Snapshot {
    fn resource(&self) -> Result<StepResource, SubstrateFault> {
        Ok(StepResource {
            resource_kind: KIND.into(),
            resource_id: self.root.display().to_string(),
            payload: serde_json::to_string(self).map_err(fail)?,
        })
    }
    pub fn commit(&self) -> Result<&str, SubstrateFault> {
        self.commit
            .as_deref()
            .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| fail("snapshot has no recorded Git commit"))
    }
    /// Read only the sealed archive. The hook working directory is not an upload input.
    pub fn archive(&self, relative: Option<&str>) -> Result<SourceArchive, SubstrateFault> {
        match self.kind {
            SourceKind::Git => {
                self.commit()?;
            }
            SourceKind::Empty if !self.repo.is_empty() || self.commit.is_some() => {
                return Err(fail("empty snapshot contains a Git identity"));
            }
            SourceKind::Empty => (),
        }
        let file = std::fs::File::open(self.root.join("archive.json")).map_err(fail)?;
        let mut archive: SourceArchive = serde_json::from_reader(file).map_err(fail)?;
        archive.validate().map_err(fail)?;
        if self.kind == SourceKind::Empty
            && (!archive.files.is_empty() || !archive.directories.is_empty())
        {
            return Err(fail("empty snapshot contains source files"));
        }
        if self.digest.as_deref() != Some(&digest(&archive)?) {
            return Err(fail("source archive differs from its recorded digest"));
        }
        let mut parts = Vec::new();
        for component in Path::new(relative.unwrap_or(".")).components() {
            match component {
                Component::CurDir => (),
                Component::Normal(part) => {
                    let part = part
                        .to_str()
                        .ok_or_else(|| fail("source root is not UTF-8"))?;
                    if stackless_core::source_archive::excluded(part) || part.contains('\\') {
                        return Err(fail("source root selects a protected path"));
                    }
                    parts.push(part);
                }
                _ => return Err(fail("source root must stay inside the snapshot")),
            }
        }
        if !parts.is_empty() {
            let root = parts.join("/");
            if !archive.directories.contains(&root) {
                return Err(fail(format!(
                    "source root {root:?} is absent from the snapshot"
                )));
            }
            let prefix = format!("{root}/");
            archive.directories = archive
                .directories
                .into_iter()
                .filter_map(|p| p.strip_prefix(&prefix).map(str::to_owned))
                .collect();
            archive.files = archive
                .files
                .into_iter()
                .filter_map(|mut f| {
                    let path = f.path.strip_prefix(&prefix)?.to_owned();
                    f.path = path;
                    Some(f)
                })
                .collect();
        }
        Ok(archive)
    }
    pub fn working_directory_at(&self, relative: Option<&str>) -> Result<PathBuf, SubstrateFault> {
        self.archive(relative)?;
        let base = self.working_directory()?;
        let selected = std::fs::canonicalize(base.join(relative.unwrap_or("."))).map_err(fail)?;
        if !selected.starts_with(std::fs::canonicalize(&base).map_err(fail)?) {
            return Err(fail("hook working directory escapes the source snapshot"));
        }
        Ok(selected)
    }
    pub fn working_directory(&self) -> Result<PathBuf, SubstrateFault> {
        let archive = self.archive(None)?;
        if !self.path.exists() {
            let staging = self.root.join("work-staging");
            if staging.exists() {
                std::fs::remove_dir_all(&staging).map_err(fail)?;
            }
            private_dir(&staging)?;
            archive.extract(&staging).map_err(fail)?;
            std::fs::rename(staging, &self.path).map_err(fail)?;
        }
        if !std::fs::symlink_metadata(&self.path)
            .map_err(fail)?
            .is_dir()
        {
            return Err(fail("source working directory is not a directory"));
        }
        Ok(self.path.clone())
    }
}

pub fn recorded(prior: &[Checkpoint], service: &str) -> Result<Snapshot, SubstrateFault> {
    let checkpoint = prior
        .iter()
        .find(|cp| cp.step_id == format!("materialize:{service}") && cp.resource_kind == KIND)
        .ok_or_else(|| fail(format!("{service} has no durable source snapshot")))?;
    serde_json::from_str(&checkpoint.payload).map_err(fail)
}

pub async fn materialize(
    ctx: &StepContext<'_>,
    base: &Path,
    provider: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<StepResource, SubstrateFault> {
    let spec = ctx
        .def
        .services
        .get(&ctx.step.node)
        .ok_or_else(|| fail("source workload is missing"))?;
    let source_root = spec
        .source_root(&ctx.step.node, provider)
        .map_err(|error| SubstrateFault::from_fault(&error))?;
    if spec.source.path.is_some() {
        return Err(fail("cloud snapshots require Git or an empty source"));
    }
    let kind = if spec.source.repo.is_empty() {
        SourceKind::Empty
    } else {
        SourceKind::Git
    };
    if kind == SourceKind::Empty && source_root.as_deref().is_some_and(|root| root != ".") {
        return Err(fail("an empty source cannot select a subdirectory"));
    }
    let hash = digest(&(
        ctx.instance.id,
        ctx.operation_id,
        &ctx.step.id,
        &spec.source,
    ))?;
    let key = format!("cloud-source:{hash}");
    let root = std::fs::canonicalize(base)
        .map_err(fail)?
        .join(".stackless-sources")
        .join(ctx.instance.id)
        .join(&hash);
    let mut snapshot = Snapshot {
        owner: ctx.instance.id.into(),
        operation: ctx.operation_id.into(),
        key: key.clone(),
        kind,
        repo: spec.source.repo.clone(),
        reference: spec.source.reference.clone(),
        commit: None,
        digest: None,
        path: root.join("work"),
        root,
    };
    if let Some(record) = ctx.store.resource(ctx.instance.id, &key).map_err(state)? {
        let saved: Snapshot = serde_json::from_str(&record.payload).map_err(fail)?;
        if record.ownership != Ownership::Owned
            || record.provider != provider
            || record.resource_kind != KIND
            || saved.owner != snapshot.owner
            || saved.operation != snapshot.operation
            || saved.kind != kind
            || saved.key != key
            || saved.root != snapshot.root
            || saved.repo != snapshot.repo
            || saved.reference != snapshot.reference
            || saved.path != snapshot.path
        {
            return Err(fail("source ownership or operation inputs changed"));
        }
        if matches!(record.phase, ResourcePhase::Created | ResourcePhase::Ready) {
            saved.working_directory_at(source_root.as_deref())?;
            return saved.resource();
        }
        if record.phase == ResourcePhase::Absent {
            return Err(fail("source operation was already destroyed"));
        }
        snapshot = saved;
    }
    let intent = snapshot.resource()?;
    ctx.store
        .resource_intent(ResourceIntent {
            owner_id: ctx.instance.id,
            key: &key,
            step_id: &ctx.step.id,
            provider,
            ownership: Ownership::Owned,
            resource_kind: KIND,
            resource_id: &intent.resource_id,
            payload: &intent.payload,
            dependencies: ctx.parent_resources,
        })
        .map_err(state)?;
    let credentials = stackless_git::Credentials::from_secrets(secrets);
    let snapshot = tokio::task::spawn_blocking(move || seal(snapshot, credentials))
        .await
        .map_err(fail)??;
    snapshot.working_directory_at(source_root.as_deref())?;
    let resource = snapshot.resource()?;
    ctx.store
        .resource_created(
            ctx.instance.id,
            &key,
            &resource.resource_id,
            &resource.payload,
        )
        .map_err(state)?;
    ctx.store
        .resource_ready(ctx.instance.id, &key)
        .map_err(state)?;
    Ok(resource)
}

fn seal(
    mut snapshot: Snapshot,
    credentials: stackless_git::Credentials,
) -> Result<Snapshot, SubstrateFault> {
    private_dir(&snapshot.root)?;
    let manifest = snapshot.root.join("snapshot.json");
    if manifest.exists() {
        let saved: Snapshot =
            serde_json::from_reader(std::fs::File::open(&manifest).map_err(fail)?).map_err(fail)?;
        let mut unsealed = saved.clone();
        unsealed.commit = None;
        unsealed.digest = None;
        if unsealed != snapshot {
            return Err(fail("sealed snapshot belongs to another intent"));
        }
        snapshot = saved;
    } else {
        let checkout = snapshot.root.join("checkout");
        if checkout.exists() {
            std::fs::remove_dir_all(&checkout).map_err(fail)?;
        }
        private_dir(&checkout)?;
        let objects = snapshot.root.join("git");
        if objects.exists() {
            std::fs::remove_dir_all(&objects).map_err(fail)?;
        }
        let archive = if snapshot.kind == SourceKind::Git {
            stackless_git::fetch_bare(
                &objects,
                &snapshot.repo,
                &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
                None,
                &credentials,
            )
            .map_err(fail)?;
            let commit =
                stackless_git::resolve_commit(&objects, &snapshot.reference).map_err(fail)?;
            stackless_git::checkout_detached(&checkout, &objects, &commit).map_err(fail)?;
            snapshot.commit = Some(commit);
            SourceArchive::capture_beneath(&checkout, Path::new(".")).map_err(fail)?
        } else {
            SourceArchive {
                directories: Vec::new(),
                files: Vec::new(),
            }
        };
        snapshot.digest = Some(digest(&archive)?);
        atomic_json(&snapshot.root.join("archive.json"), &archive)?;
        atomic_json(&manifest, &snapshot)?;
    }
    snapshot.working_directory()?;
    let checkout = snapshot.root.join("checkout");
    if checkout.exists() {
        std::fs::remove_dir_all(checkout).map_err(fail)?;
    }
    let objects = snapshot.root.join("git");
    if objects.exists() {
        std::fs::remove_dir_all(objects).map_err(fail)?;
    }
    Ok::<_, SubstrateFault>(snapshot)
}

fn owned(
    base: &Path,
    instance: &InstanceContext<'_>,
    cp: &Checkpoint,
) -> Result<Snapshot, SubstrateFault> {
    let snapshot: Snapshot = serde_json::from_str(&cp.payload).map_err(fail)?;
    let hash = snapshot
        .key
        .strip_prefix("cloud-source:")
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| fail("invalid source ownership key"))?;
    let expected = std::fs::canonicalize(base)
        .map_err(fail)?
        .join(".stackless-sources")
        .join(instance.id)
        .join(hash);
    if snapshot.owner != instance.id
        || snapshot.root != expected
        || snapshot.path != expected.join("work")
        || cp.resource_id != expected.display().to_string()
    {
        return Err(fail("source path differs from its ownership record"));
    }
    Ok(snapshot)
}

pub fn observe(
    base: &Path,
    instance: &InstanceContext<'_>,
    cp: &Checkpoint,
) -> Result<Observation, SubstrateFault> {
    let snapshot = owned(base, instance, cp)?;
    match std::fs::symlink_metadata(&snapshot.root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Observation::Gone),
        Err(e) => Err(fail(e)),
        Ok(meta) if !meta.is_dir() => Err(fail("owned source root is not a directory")),
        Ok(_) if snapshot.digest.is_none() => Ok(Observation::Present),
        Ok(_) => {
            snapshot.archive(None)?;
            Ok(Observation::Present)
        }
    }
}
pub fn destroy(
    base: &Path,
    instance: &InstanceContext<'_>,
    cp: &Checkpoint,
) -> Result<(), SubstrateFault> {
    let snapshot = owned(base, instance, cp)?;
    match std::fs::remove_dir_all(snapshot.root) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(fail(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stackless_core::{
        def::StackDef,
        engine::{Step, StepKind},
        state::Store,
    };

    async fn capture(
        store: &Store,
        base: &Path,
        repo: &Path,
        reference: &str,
        operation: &str,
        name: &str,
    ) -> StepResource {
        let def = StackDef::parse(&format!("[stack]\nname='test'\n[services.web]\nsource={{repo={:?},ref={reference:?}}}\nhealth={{path='/'}}\n", repo.display().to_string())).unwrap();
        let record = store.instance(name).unwrap().unwrap();
        let instance = InstanceContext::from_record(&record, &[]);
        let step = Step {
            id: "materialize:web".into(),
            kind: StepKind::Materialize,
            node: "web".into(),
        };
        let ctx = StepContext {
            operation_id: operation,
            store,
            instance: &instance,
            def: &def,
            step: &step,
            source_overrides: &BTreeMap::new(),
            dirty: false,
            prior: &[],
            parent_resources: &[],
            cancelled: None,
        };
        materialize(&ctx, base, "test", &BTreeMap::new())
            .await
            .unwrap()
    }
    fn instance(store: &Store, name: &str) {
        store
            .create_instance(name, "test", "definition", &BTreeMap::new(), "", false)
            .unwrap();
    }
    fn snapshot(resource: &StepResource) -> Snapshot {
        serde_json::from_str(&resource.payload).unwrap()
    }
    fn checkpoint(resource: StepResource, name: &str) -> Checkpoint {
        Checkpoint {
            instance: name.into(),
            step_id: "materialize:web".into(),
            resource_kind: resource.resource_kind,
            resource_id: resource.resource_id,
            payload: resource.payload,
            recorded_at: 0,
        }
    }

    #[tokio::test]
    async fn restart_uses_saved_bytes_after_branch_moves_and_repository_disappears() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let first = stackless_git::build_repo(
            &repo,
            &[&[("app/data.txt", "original"), (".env", "private")]],
        )
        .unwrap();
        let db = dir.path().join("state.db");
        let store = Store::open(&db).unwrap();
        instance(&store, "one");
        let resource = capture(&store, dir.path(), &repo, "main", "op-one", "one").await;
        let saved = snapshot(&resource);
        assert_eq!(saved.commit().unwrap(), first);
        assert_eq!(saved.archive(None).unwrap().files.len(), 1);
        let second =
            stackless_git::build_repo(&repo, &[&[("app/data.txt", "new branch")]]).unwrap();
        assert_ne!(first, second);
        // The same operation never resolves the moved branch again.
        assert_eq!(
            capture(&store, dir.path(), &repo, "main", "op-one", "one")
                .await
                .payload,
            resource.payload
        );
        let updated = snapshot(&capture(&store, dir.path(), &repo, "main", "op-two", "one").await);
        assert_eq!(updated.commit().unwrap(), second);
        // A full commit reference resolves from the fetched object graph.
        let pinned = snapshot(&capture(&store, dir.path(), &repo, &second, "op-sha", "one").await);
        assert_eq!(pinned.commit().unwrap(), second);
        drop(store);
        std::fs::remove_dir_all(&repo).unwrap();
        std::fs::remove_dir_all(&saved.path).unwrap();
        let reopened = Store::open(&db).unwrap();
        let resumed =
            snapshot(&capture(&reopened, dir.path(), &repo, "main", "op-one", "one").await);
        assert_eq!(resumed, saved);
        assert_eq!(
            std::fs::read(saved.path.join("app/data.txt")).unwrap(),
            b"original"
        );
        assert!(saved.archive(Some("../outside")).is_err());
        assert!(saved.archive(Some("/app")).is_err());
        assert!(saved.archive(Some(".env")).is_err());
    }

    #[tokio::test]
    async fn hooks_use_snapshot_root_but_cannot_change_upload_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        stackless_git::build_repo(&repo, &[&[("app/data.txt", "original")]]).unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        instance(&store, "one");
        let resource = capture(&store, dir.path(), &repo, "main", "op-one", "one").await;
        let saved = snapshot(&resource);
        let prior = [checkpoint(resource, "one")];
        let def = StackDef::parse("[stack]\nname='test'\n[services.web]\nsource={repo='https://unavailable.invalid',root='app'}\nprepare='cat data.txt > seen.txt; printf modified > data.txt'\nhealth={path='/'}\n").unwrap();
        std::fs::remove_dir_all(repo).unwrap();
        let record = store.instance("one").unwrap().unwrap();
        store.grant_host_execution(&record.instance_id).unwrap();
        let instance = InstanceContext::from_record(&record, &[]);
        let step = Step {
            id: "prepare:web".into(),
            kind: StepKind::Prepare,
            node: "web".into(),
        };
        let ctx = StepContext {
            operation_id: "op-one",
            store: &store,
            instance: &instance,
            def: &def,
            step: &step,
            source_overrides: &BTreeMap::new(),
            dirty: false,
            prior: &prior,
            parent_resources: &[],
            cancelled: None,
        };
        crate::prepare::run_snapshot_prepare(
            &ctx,
            dir.path(),
            &Default::default(),
            &BTreeMap::new(),
            "test",
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(saved.path.join("app/seen.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            std::fs::read(saved.path.join("app/data.txt")).unwrap(),
            b"modified"
        );
        let archive = saved.archive(Some("app")).unwrap();
        assert_eq!(archive.files.len(), 1);
        assert_eq!(archive.files[0].contents, "b3JpZ2luYWw=");
        assert_eq!(archive.files[0].path, "data.txt");
    }

    #[tokio::test]
    async fn source_ownership_blocks_sibling_deletion_and_corruption_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        stackless_git::build_repo(&repo, &[&[("file", "source")]]).unwrap();
        let store = Store::open(&dir.path().join("state.db")).unwrap();
        instance(&store, "one");
        instance(&store, "two");
        let one = checkpoint(
            capture(&store, dir.path(), &repo, "main", "op", "one").await,
            "one",
        );
        let two = checkpoint(
            capture(&store, dir.path(), &repo, "main", "op", "two").await,
            "two",
        );
        let first = store.instance("one").unwrap().unwrap();
        let second = store.instance("two").unwrap().unwrap();
        let first = InstanceContext::from_record(&first, &[]);
        let second = InstanceContext::from_record(&second, &[]);
        assert!(destroy(dir.path(), &first, &two).is_err());
        destroy(dir.path(), &first, &one).unwrap();
        assert_eq!(
            observe(dir.path(), &first, &one).unwrap(),
            Observation::Gone
        );
        assert_eq!(
            observe(dir.path(), &second, &two).unwrap(),
            Observation::Present
        );
        let payload: Snapshot = serde_json::from_str(&two.payload).unwrap();
        std::fs::write(payload.root.join("archive.json"), b"{}").unwrap();
        assert!(observe(dir.path(), &second, &two).is_err());
        destroy(dir.path(), &second, &two).unwrap();
        assert_eq!(
            observe(dir.path(), &second, &two).unwrap(),
            Observation::Gone
        );
    }
    #[tokio::test]
    async fn sealed_archive_recovers_before_database_registration() {
        sealed_recovery(false).await;
    }
    #[tokio::test]
    async fn empty_archive_recovers_before_database_registration() {
        sealed_recovery(true).await;
    }
    async fn sealed_recovery(empty: bool) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let commit = stackless_git::build_repo(&repo, &[&[("file", "saved")]]).unwrap();
        let db = dir.path().join("state.db");
        let store = Store::open(&db).unwrap();
        instance(&store, "one");
        let owner = store.instance("one").unwrap().unwrap().instance_id;
        let source = stackless_core::def::Source {
            repo: if empty {
                String::new()
            } else {
                repo.display().to_string()
            },
            reference: "main".into(),
            ..Default::default()
        };
        let hash = digest(&(&owner, "op", "materialize:web", &source)).unwrap();
        let root = std::fs::canonicalize(dir.path())
            .unwrap()
            .join(".stackless-sources")
            .join(&owner)
            .join(&hash);
        let pending = Snapshot {
            owner: owner.clone(),
            operation: "op".into(),
            key: format!("cloud-source:{hash}"),
            kind: if empty {
                SourceKind::Empty
            } else {
                SourceKind::Git
            },
            repo: source.repo,
            reference: source.reference,
            commit: None,
            digest: None,
            path: root.join("work"),
            root,
        };
        let resource = pending.resource().unwrap();
        store
            .resource_intent(ResourceIntent {
                owner_id: &owner,
                key: &pending.key,
                step_id: "materialize:web",
                provider: "test",
                ownership: Ownership::Owned,
                resource_kind: KIND,
                resource_id: &resource.resource_id,
                payload: &resource.payload,
                dependencies: &[],
            })
            .unwrap();
        let sealed = seal(pending, Default::default()).unwrap();
        assert_eq!(
            store.resource(&owner, &sealed.key).unwrap().unwrap().phase,
            ResourcePhase::Intent
        );
        drop(store);
        std::fs::remove_dir_all(&repo).unwrap();
        let reopened = Store::open(&db).unwrap();
        let recovered = snapshot(
            &capture(
                &reopened,
                dir.path(),
                if empty { Path::new("") } else { &repo },
                "main",
                "op",
                "one",
            )
            .await,
        );
        if empty {
            assert!(recovered.commit.is_none());
            assert!(recovered.archive(None).unwrap().files.is_empty());
            std::fs::write(recovered.path.join("hook-output"), b"not source").unwrap();
            assert!(recovered.archive(None).unwrap().files.is_empty());
            std::fs::remove_dir_all(&recovered.path).unwrap();
            recovered.working_directory().unwrap();
            assert!(!recovered.path.join("hook-output").exists());
            // An empty receipt must still validate its sealed archive.
            std::fs::write(recovered.root.join("archive.json"), b"broken").unwrap();
            let record = reopened.instance("one").unwrap().unwrap();
            assert!(
                observe(
                    dir.path(),
                    &InstanceContext::from_record(&record, &[]),
                    &checkpoint(recovered.resource().unwrap(), "one")
                )
                .is_err()
            );
        } else {
            assert_eq!(recovered.commit().unwrap(), commit);
        }
        assert_eq!(recovered, sealed);
        let records = reopened.resources(&owner).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].phase, ResourcePhase::Ready);
    }
}
