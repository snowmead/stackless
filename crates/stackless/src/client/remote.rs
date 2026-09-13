//! Authenticated controller RPC through OpenSSH. SSH owns authentication and host verification.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use stackless_core::source_archive::{MAX_BYTES, MAX_ENTRIES, SourceArchive};
use stackless_core::{def::StackDef, paths::Paths};
use stackless_daemon::{
    DaemonClient, DaemonError,
    rpc::{Request as DaemonRequest, ResponseBody},
};

use super::UpArgs;
use crate::{
    controller::{Command, Reply, Request},
    error::Error,
};

const WIRE_VERSION: u32 = 2;
const MAX_WIRE_BYTES: usize = 128 * 1024 * 1024;
pub(super) const SUBMISSION_KIND: &str = "controller-submission";

fn invalid(detail: impl Into<String>) -> Error {
    Error::BadArgument {
        argument: "remote controller".into(),
        detail: detail.into(),
    }
}

fn unavailable(detail: impl Into<String>) -> Error {
    DaemonError::Unreachable {
        detail: detail.into(),
    }
    .into()
}

#[derive(Debug, Clone)]
pub(crate) struct SshController {
    target: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRequest {
    protocol: u32,
    request: Request,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReply {
    protocol: u32,
    reply: Reply,
}

impl SshController {
    pub fn new(target: String) -> Result<Self, Error> {
        let target = target.strip_prefix("ssh://").unwrap_or(&target);
        if target.is_empty()
            || target.len() > 255
            || target.starts_with('-')
            || target.matches('@').count() > 1
            || target.split('@').any(str::is_empty)
            || !target
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"@._-".contains(&byte))
        {
            return Err(invalid(
                "use ssh://HOST_ALIAS or ssh://USER@HOST; configure ports and keys in OpenSSH config",
            ));
        }
        Ok(Self {
            target: target.into(),
        })
    }

    pub fn call<T: DeserializeOwned>(&self, request: Request) -> Result<T, Error> {
        let request = match request {
            Request::Submit {
                id,
                command:
                    Command::Up {
                        mut args,
                        definition,
                        cwd,
                    },
            } => {
                let mut paths = super::parse_sources(&args.sources)?;
                if let Some(text) = &definition {
                    let def = StackDef::parse(text)?;
                    let base = args
                        .file
                        .as_deref()
                        .and_then(Path::parent)
                        .unwrap_or(&cwd)
                        .canonicalize()
                        .map_err(Error::Runtime)?;
                    for (name, service) in def.services {
                        if let Some(path) = service.source.path
                            && !paths.contains_key(&name)
                        {
                            let root = base.join(path).canonicalize().map_err(Error::Runtime)?;
                            if !root.starts_with(&base) {
                                return Err(invalid(
                                    "source.path escapes the definition directory; authorize it with --source",
                                ));
                            }
                            paths.insert(name, root.display().to_string());
                        }
                    }
                }
                let sources = paths
                    .into_iter()
                    .map(|(name, path)| {
                        Ok((
                            name,
                            SourceArchive::capture(Path::new(&path))
                                .map_err(|error| invalid(error.to_string()))?,
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>, Error>>()?;
                args.sources.clear();
                args.file = None;
                validate_upload(&args, definition.as_deref(), &sources)?;
                Request::SubmitRemoteUp {
                    id,
                    args,
                    definition,
                    sources,
                }
            }
            other => other,
        };
        let timeout = request.response_timeout();
        let bytes = serde_json::to_vec(&WireRequest {
            protocol: WIRE_VERSION,
            request,
        })
        .map_err(|_| invalid("cannot encode request"))?;
        if bytes.len() > MAX_WIRE_BYTES {
            return Err(invalid("request exceeds 128 MiB"));
        }
        // The SDK is synchronous and can be called by a Tokio application. Own this
        // subprocess runtime on another thread instead of nesting block_on.
        let response = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(Error::Runtime)?;
                    runtime.block_on(self.exchange(bytes, timeout))
                })
                .join()
                .map_err(|_| unavailable("SSH transport thread failed"))?
        })?;
        let wire: WireReply = serde_json::from_slice(&response)
            .map_err(|_| unavailable("remote host returned an incompatible controller reply"))?;
        if wire.protocol != WIRE_VERSION {
            return Err(invalid(
                "remote controller protocol version differs; install matching clients and controller",
            ));
        }
        match wire.reply {
            Reply::Ok { value } => {
                serde_json::from_value(value).map_err(|_| invalid("incompatible controller result"))
            }
            Reply::Err { error } => Err(Error::Controller(error)),
        }
    }

    async fn exchange(&self, bytes: Vec<u8>, timeout: Duration) -> Result<Vec<u8>, Error> {
        self.exchange_using(std::ffi::OsStr::new("ssh"), bytes, timeout)
            .await
    }

    async fn exchange_using(
        &self,
        program: &std::ffi::OsStr,
        bytes: Vec<u8>,
        timeout: Duration,
    ) -> Result<Vec<u8>, Error> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut command = tokio::process::Command::new(program);
        command
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ConnectTimeout=10",
                "--",
                &self.target,
                "stackless daemon remote-control",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| unavailable("cannot start OpenSSH"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| unavailable("SSH stdin is unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| unavailable("SSH stdout is unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| unavailable("SSH stderr is unavailable"))?;
        let transaction = async {
            let writer = async move {
                stdin.write_all(&bytes).await?;
                stdin.shutdown().await?;
                drop(stdin);
                Ok::<_, std::io::Error>(())
            };
            let reader = async {
                let mut reply = Vec::new();
                stdout
                    .take((MAX_WIRE_BYTES + 1) as u64)
                    .read_to_end(&mut reply)
                    .await?;
                Ok::<_, std::io::Error>(reply)
            };
            let errors = async {
                let mut buffer = Vec::new();
                stderr.take(4096).read_to_end(&mut buffer).await?;
                Ok::<_, std::io::Error>(())
            };
            tokio::try_join!(writer, reader, errors, child.wait())
        };
        let (_, reply, (), status) = tokio::time::timeout(timeout, transaction)
            .await
            .map_err(|_| {
                unavailable(format!(
                    "SSH controller request exceeded {} seconds; reconnect using the operation ID",
                    timeout.as_secs()
                ))
            })?
            .map_err(|_| unavailable("SSH controller connection failed"))?;
        if !status.success() {
            return Err(unavailable(
                "SSH request failed; check SSH login, host keys, and the remote controller service",
            ));
        }
        if reply.len() > MAX_WIRE_BYTES {
            return Err(unavailable("controller reply exceeds 128 MiB"));
        }
        Ok(reply)
    }
}

/// SSH invokes this fixed command. It exposes lifecycle RPC, never route or process RPC.
pub(crate) fn serve_stdio() -> Result<(), Error> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((MAX_WIRE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(Error::Runtime)?;
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(invalid("request exceeds 128 MiB"));
    }
    let wire: WireRequest =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid controller request"))?;
    if wire.protocol != WIRE_VERSION {
        return Err(invalid("controller protocol version differs"));
    }
    if matches!(
        wire.request,
        Request::Submit {
            command: Command::Up { .. } | Command::Gc { .. },
            ..
        }
    ) {
        return Err(invalid(
            "remote requests must carry definitions; controller filesystem paths and internal garbage collection are not accepted",
        ));
    }
    let mut connection = DaemonClient::connect()?;
    let timeout = wire.request.response_timeout();
    let body = connection.call_with_timeout(
        DaemonRequest::Control {
            request: serde_json::to_value(wire.request)
                .map_err(|_| invalid("cannot encode request"))?,
        },
        timeout,
    )?;
    let ResponseBody::Control { response } = body else {
        return Err(unavailable("remote daemon has no lifecycle controller"));
    };
    let reply: Reply = serde_json::from_value(response)
        .map_err(|_| unavailable("remote daemon returned an incompatible reply"))?;
    serde_json::to_writer(
        std::io::stdout().lock(),
        &WireReply {
            protocol: WIRE_VERSION,
            reply,
        },
    )
    .map_err(|_| unavailable("cannot write controller reply"))?;
    std::io::stdout().flush().map_err(Error::Runtime)
}

pub(crate) fn validate_upload(
    args: &UpArgs,
    definition: Option<&str>,
    sources: &BTreeMap<String, SourceArchive>,
) -> Result<(), Error> {
    if !args.sources.is_empty() || args.file.is_some() {
        return Err(invalid(
            "remote requests cannot select controller filesystem paths",
        ));
    }
    let mut bytes = 0usize;
    let mut entries = 0usize;
    for (name, archive) in sources {
        stackless_core::types::DnsName::try_new(name.clone())
            .map_err(|_| invalid("invalid uploaded workload name"))?;
        bytes = bytes.saturating_add(
            archive
                .validate()
                .map_err(|error| invalid(error.to_string()))?,
        );
        entries = entries.saturating_add(archive.files.len() + archive.directories.len());
        if bytes > MAX_BYTES || entries > MAX_ENTRIES {
            return Err(invalid(
                "combined source uploads exceed 64 MiB or 100000 entries",
            ));
        }
    }
    if let Some(text) = definition {
        let def = StackDef::parse(text)?;
        if sources.keys().any(|name| !def.services.contains_key(name)) {
            return Err(invalid("source upload names an unknown workload"));
        }
        if def
            .services
            .iter()
            .any(|(name, service)| service.source.path.is_some() && !sources.contains_key(name))
        {
            return Err(invalid(
                "source.path requires an uploaded source; controller filesystem paths are not accepted",
            ));
        }
    }
    Ok(())
}

pub(crate) fn materialize_definition(
    paths: &Paths,
    id: &str,
    mut args: UpArgs,
    definition: Option<String>,
    sources: BTreeMap<String, SourceArchive>,
) -> Result<Command, Error> {
    validate_upload(&args, definition.as_deref(), &sources)?;
    let digest = stackless_core::engine::revision::digest(&(id, &args, &definition, &sources))
        .map_err(|fault| Error::substrate(fault, None))?;
    let root = paths.state_dir().join("submissions");
    private_dir(&root)?;
    let root = root.canonicalize().map_err(Error::Runtime)?;
    let dir = root.join(&digest);
    if !dir.exists() {
        let staging = root.join(format!(".partial-{digest}"));
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(Error::Runtime)?;
        }
        private_dir(&staging)?;
        if let Some(text) = &definition {
            write_new(&staging.join("stackless.toml"), text.as_bytes())?;
        }
        for (name, archive) in &sources {
            let source = staging.join("sources").join(name);
            private_dir(&source)?;
            archive
                .extract(&source)
                .map_err(|error| invalid(error.to_string()))?;
            stackless_git::initialize_snapshot_source(
                &source,
                &archive
                    .files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<Vec<_>>(),
            )
            .map_err(|error| invalid(error.to_string()))?;
        }
        write_new(&staging.join(".ready"), digest.as_bytes())?;
        std::fs::File::open(&staging)
            .and_then(|directory| directory.sync_all())
            .map_err(Error::Runtime)?;
        std::fs::rename(&staging, &dir).map_err(Error::Runtime)?;
        std::fs::File::open(&root)
            .and_then(|directory| directory.sync_all())
            .map_err(Error::Runtime)?;
    }
    if !std::fs::symlink_metadata(&dir)
        .map_err(Error::Runtime)?
        .is_dir()
        || std::fs::read_to_string(dir.join(".ready")).map_err(Error::Runtime)? != digest
    {
        return Err(invalid(
            "remote source snapshot has an invalid completion marker",
        ));
    }
    if let Some(text) = &definition {
        let file = dir.join("stackless.toml");
        if !std::fs::symlink_metadata(&file)
            .map_err(Error::Runtime)?
            .is_file()
            || std::fs::read_to_string(&file).map_err(Error::Runtime)? != *text
        {
            return Err(invalid("remote definition snapshot was changed"));
        }
        args.file = Some(file);
    }
    for (name, archive) in sources {
        let source = dir.join("sources").join(&name);
        if !std::fs::symlink_metadata(&source)
            .map_err(Error::Runtime)?
            .is_dir()
            || SourceArchive::capture(&source).map_err(|error| invalid(error.to_string()))?
                != archive
        {
            return Err(invalid("remote source snapshot was changed"));
        }
        args.sources.push(format!("{name}={}", source.display()));
    }
    // Uploaded worktrees are immutable inputs. Hooks and workloads use owned snapshots.
    if !args.sources.is_empty() {
        args.dirty = true;
    }
    Ok(Command::Up {
        args,
        definition,
        cwd: dir,
    })
}

/// The accepted request contains this directory's identity before extraction.
/// This also removes partial uploads that never reached instance admission.
pub(crate) fn remove_submission(
    paths: &Paths,
    id: &str,
    args: &UpArgs,
    definition: &Option<String>,
    sources: &BTreeMap<String, SourceArchive>,
) -> Result<(), Error> {
    let digest = stackless_core::engine::revision::digest(&(id, args, definition, sources))
        .map_err(|fault| Error::substrate(fault, None))?;
    let root = paths.state_dir().join("submissions");
    match std::fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) if metadata.is_dir() => (),
        Err(error) => return Err(Error::Runtime(error)),
        _ => {
            return Err(invalid(
                "submission root is no longer an ordinary directory",
            ));
        }
    }
    for name in [&digest, &format!(".partial-{digest}")] {
        let path = root.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Ok(metadata) if metadata.is_dir() => (),
            Err(error) => return Err(Error::Runtime(error)),
            _ => {
                return Err(invalid(
                    "submission path is no longer an ordinary directory",
                ));
            }
        }
        std::fs::remove_dir_all(&path).map_err(Error::Runtime)?;
    }
    std::fs::File::open(&root)
        .and_then(|directory| directory.sync_all())
        .map_err(Error::Runtime)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(Error::Runtime)?;
    file.write_all(bytes).map_err(Error::Runtime)?;
    file.sync_all().map_err(Error::Runtime)
}

pub(super) fn record_submission(
    paths: &Paths,
    store: &stackless_core::state::Store,
    owner: &str,
    cwd: &Path,
) -> Result<(), Error> {
    use stackless_core::state::{INSTANCE_RESOURCE_STEP, Ownership, ResourceIntent, ResourcePhase};
    let root = paths.state_dir().join("submissions");
    if !root.exists() {
        return Ok(());
    }
    let root = root.canonicalize().map_err(Error::Runtime)?;
    if cwd.parent() != Some(root.as_path()) {
        return Ok(());
    }
    let digest = cwd
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| invalid("invalid submission directory identity"))?;
    if std::fs::read_to_string(cwd.join(".ready")).map_err(Error::Runtime)? != digest {
        return Err(invalid("submission is not complete"));
    }
    let key = format!("submission:{digest}");
    let id = cwd.display().to_string();
    let payload = serde_json::json!({"owner": owner, "digest": digest}).to_string();
    let record = store.resource_intent(ResourceIntent {
        owner_id: owner,
        key: &key,
        step_id: INSTANCE_RESOURCE_STEP,
        provider: "controller",
        ownership: Ownership::Owned,
        resource_kind: SUBMISSION_KIND,
        resource_id: &id,
        payload: &payload,
        dependencies: &[],
    })?;
    if record.phase != ResourcePhase::Ready {
        store.resource_created(owner, &key, &id, &payload)?;
        store.resource_ready(owner, &key)?;
    }
    Ok(())
}

pub(super) fn submission_path(
    state_root: &Path,
    owner: &str,
    resource: &stackless_core::state::ResourceRecord,
) -> Result<std::path::PathBuf, Error> {
    use stackless_core::state::{INSTANCE_RESOURCE_STEP, Ownership};
    let value: serde_json::Value = serde_json::from_str(&resource.payload)
        .map_err(|_| invalid("invalid submission ownership payload"))?;
    let digest = value["digest"]
        .as_str()
        .filter(|name| name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| invalid("invalid submission digest"))?;
    let root = state_root
        .join("submissions")
        .canonicalize()
        .map_err(Error::Runtime)?;
    let path = root.join(digest);
    if resource.ownership != Ownership::Owned
        || resource.owner_id != owner
        || value["owner"].as_str() != Some(owner)
        || resource.provider != "controller"
        || resource.step_id != INSTANCE_RESOURCE_STEP
        || resource.resource_kind != SUBMISSION_KIND
        || resource.key != format!("submission:{digest}")
        || Path::new(&resource.resource_id) != path
    {
        return Err(invalid("submission path does not match its owner"));
    }
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {
            if std::fs::read_to_string(path.join(".ready")).map_err(Error::Runtime)? != digest {
                return Err(invalid("submission marker changed"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        _ => {
            return Err(invalid(
                "submission path is no longer an ordinary directory",
            ));
        }
    }
    Ok(path)
}

fn private_dir(path: &Path) -> Result<(), Error> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(Error::Runtime)?;
    if !std::fs::symlink_metadata(path)
        .map_err(Error::Runtime)?
        .is_dir()
    {
        return Err(invalid("submission directory is not an ordinary directory"));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(Error::Runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_upload_cleanup_removes_only_its_request_and_partial_directory() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::new(root.path().join("state"));
        let args = args();
        let definition = None;
        let sources = BTreeMap::new();
        let first =
            materialize_definition(&paths, "one", args.clone(), None, sources.clone()).unwrap();
        let sibling =
            materialize_definition(&paths, "two", args.clone(), None, sources.clone()).unwrap();
        let Command::Up { cwd: first, .. } = first else {
            panic!("up");
        };
        let Command::Up { cwd: sibling, .. } = sibling else {
            panic!("up");
        };
        let digest = first.file_name().unwrap().to_str().unwrap();
        let partial = first.parent().unwrap().join(format!(".partial-{digest}"));
        std::fs::create_dir(&partial).unwrap();
        std::fs::write(partial.join("unfinished"), "private bytes").unwrap();
        remove_submission(&paths, "one", &args, &definition, &sources).unwrap();
        assert!(!first.exists());
        assert!(!partial.exists());
        assert!(sibling.join(".ready").is_file());
        // A lost database update repeats file deletion using the retained request.
        remove_submission(&paths, "one", &args, &definition, &sources).unwrap();
        assert!(sibling.is_dir());
    }

    #[test]
    fn expired_upload_cleanup_refuses_symlinked_paths() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::new(root.path().join("state"));
        let args = args();
        let sources = BTreeMap::new();
        let digest =
            stackless_core::engine::revision::digest(&("one", &args, &None::<String>, &sources))
                .unwrap();
        let submissions = paths.state_dir().join("submissions");
        std::fs::create_dir_all(&submissions).unwrap();
        let outside = root.path().join("borrowed");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), "borrowed").unwrap();
        let target = submissions.join(&digest);
        std::os::unix::fs::symlink(&outside, &target).unwrap();
        assert!(remove_submission(&paths, "one", &args, &None, &sources).is_err());
        assert!(outside.join("keep").is_file());
        std::fs::remove_file(target).unwrap();
        std::fs::remove_dir(&submissions).unwrap();
        std::os::unix::fs::symlink(&outside, &submissions).unwrap();
        assert!(remove_submission(&paths, "one", &args, &None, &sources).is_err());
        assert!(outside.join("keep").is_file());
    }

    fn args() -> UpArgs {
        UpArgs {
            name: Some("remote-demo".into()),
            file: None,
            on: Some("local".into()),
            sources: vec![],
            dirty: false,
            allow_host_execution: true,
            lease: None,
            confirm_paid: false,
        }
    }

    #[test]
    fn remote_host_cannot_inject_options_or_shell_commands() {
        for host in [
            "ssh://builder",
            "ssh://deploy@controller.example",
            "dev-alias",
        ] {
            assert!(SshController::new(host.into()).is_ok());
        }
        for host in [
            "",
            "-oProxyCommand=x",
            "host;id",
            "$(id)",
            "user@",
            "@host",
            "user@host:22",
            "ssh://host/path",
            "host\nother",
        ] {
            assert!(SshController::new(host.into()).is_err());
        }
    }

    #[test]
    fn uploads_are_atomic_recoverable_snapshots_and_never_select_host_files() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::new(root.path().join("state"));
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("app.py"), "print('uploaded')").unwrap();
        std::fs::write(source.join(".gitignore"), "app.py\n").unwrap();
        let uploads = BTreeMap::from([("web".into(), SourceArchive::capture(&source).unwrap())]);
        let definition = "[stack]\nname='demo'\n[services.web]\nkind='worker'\nrun='python3 app.py'\n[services.web.source]\npath='/caller-only/path'".to_owned();
        let first = materialize_definition(
            &paths,
            "submission-one",
            args(),
            Some(definition.clone()),
            uploads.clone(),
        )
        .unwrap();
        let Command::Up {
            args: normalized,
            cwd,
            ..
        } = first
        else {
            panic!("normalized up");
        };
        assert!(normalized.dirty);
        assert_eq!(
            std::fs::read_to_string(cwd.join("sources/web/app.py")).unwrap(),
            "print('uploaded')"
        );
        let snapshot = root.path().join("execution");
        stackless_git::snapshot_worktree(&snapshot, &cwd.join("sources/web")).unwrap();
        assert!(
            snapshot.join("app.py").exists(),
            "uploaded files remain tracked even when gitignore matches"
        );
        let again = materialize_definition(
            &paths,
            "submission-one",
            args(),
            Some(definition.clone()),
            uploads.clone(),
        )
        .unwrap();
        let Command::Up { cwd: again, .. } = again else {
            panic!("normalized up");
        };
        assert_eq!(again, cwd);
        std::fs::write(cwd.join("sources/web/app.py"), "modified").unwrap();
        assert!(
            materialize_definition(
                &paths,
                "submission-one",
                args(),
                Some(definition.clone()),
                uploads
            )
            .is_err()
        );
        assert!(validate_upload(&args(), Some(&definition), &BTreeMap::new()).is_err());
        let mut bad = args();
        bad.sources.push("web=/controller/private".into());
        assert!(validate_upload(&bad, None, &BTreeMap::new()).is_err());
    }

    #[tokio::test]
    async fn ssh_transport_closes_stdin_and_requires_host_key_verification() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("ssh-test");
        std::fs::write(&program, "#!/usr/bin/env python3\nimport json,sys\nassert '-T' in sys.argv\nassert 'BatchMode=yes' in sys.argv\nassert 'StrictHostKeyChecking=yes' in sys.argv\nassert sys.argv[-2:] == ['controller-host', 'stackless daemon remote-control']\nrequest=json.load(sys.stdin)\nassert request['protocol']==2\nprint(json.dumps({'protocol':2,'reply':{'value':True}}))\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let controller = SshController::new("controller-host".into()).unwrap();
        let request = serde_json::to_vec(&WireRequest {
            protocol: WIRE_VERSION,
            request: Request::List,
        })
        .unwrap();
        let reply = controller
            .exchange_using(program.as_os_str(), request, Duration::from_secs(30))
            .await
            .unwrap();
        let reply: WireReply = serde_json::from_slice(&reply).unwrap();
        assert!(matches!(reply.reply, Reply::Ok { value } if value == true));
    }

    #[tokio::test]
    async fn ssh_transport_honors_the_response_budget() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("ssh-delay");
        std::fs::write(&program, "#!/usr/bin/env python3\nimport sys,time\nsys.stdin.read()\ntime.sleep(0.2)\nprint('{}')\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let controller = SshController::new("controller-host".into()).unwrap();
        assert!(
            controller
                .exchange_using(program.as_os_str(), vec![], Duration::from_millis(50))
                .await
                .is_err()
        );
        assert!(
            controller
                .exchange_using(program.as_os_str(), vec![], Duration::from_secs(5))
                .await
                .is_ok()
        );
    }
}
