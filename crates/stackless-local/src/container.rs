//! The container runner (§3): datastores run as containers with
//! per-instance volumes. Pull image, inject env, map an ephemeral
//! loopback port, mount the volume, health-check with the engine's own
//! readiness (postgres: `pg_isready`), destroy and confirm gone.
//! Everything is labeled with the instance name — the §0 seam that
//! lets the boundary phase wrap instances in networks later.
//!
//! Socket resolution is ours (§8): `DOCKER_HOST` if set, else the
//! standard unix sockets — contexts are a docker-CLI concept.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, InspectContainerOptions, ListVolumesOptions,
    RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use rand::RngExt;
use rand::distr::Alphanumeric;
use stackless_core::fault::{Fault, codes};
use stackless_core::types::{ContainerId, TcpPort};

pub const INSTANCE_LABEL: &str = "stackless.instance";
pub const DATASTORE_LABEL: &str = "stackless.datastore";

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("cannot reach the Docker engine: {detail}")]
    Engine { detail: String },

    #[error("datastore {datastore:?} ({image}) failed to {action}: {detail}")]
    Operation {
        datastore: String,
        image: String,
        action: &'static str,
        detail: String,
    },

    #[error("datastore {datastore:?} did not become ready within {budget_secs}s: {detail}")]
    NotReady {
        datastore: String,
        budget_secs: u64,
        detail: String,
    },
}

impl Fault for ContainerError {
    fn code(&self) -> &'static str {
        match self {
            Self::Engine { .. } => codes::LOCAL_DOCKER_ENGINE,
            Self::Operation { .. } => codes::LOCAL_DATASTORE_FAILED,
            Self::NotReady { .. } => codes::LOCAL_DATASTORE_NOT_READY,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Engine { .. } => "start Docker (or set DOCKER_HOST) and re-run `up`".into(),
            Self::Operation { .. } => {
                "check `docker ps -a` and the Docker daemon logs, then re-run `up`".into()
            }
            Self::NotReady { datastore, .. } => format!(
                "inspect the container: `docker logs stackless-<instance>-{datastore}`; fix and re-run `up`"
            ),
        }
    }
}

/// A connected Docker engine handle reused across provision/observe/destroy.
#[derive(Debug, Clone)]
pub struct ContainerRunner {
    docker: Docker,
}

impl ContainerRunner {
    pub fn connect() -> Result<Self, ContainerError> {
        if std::env::var_os("DOCKER_HOST").is_some() {
            return Docker::connect_with_defaults()
                .map(Self::from_docker)
                .map_err(|err| ContainerError::Engine {
                    detail: err.to_string(),
                });
        }
        // The standard sockets, most-specific first (Docker Desktop's
        // per-user socket, then the system path).
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let candidates = [
            home.map(|h| h.join(".docker/run/docker.sock")),
            Some(std::path::PathBuf::from("/var/run/docker.sock")),
        ];
        let mut last_err = String::from("no docker socket found");
        for candidate in candidates.into_iter().flatten() {
            if !candidate.exists() {
                continue;
            }
            match Docker::connect_with_unix(
                &candidate.display().to_string(),
                120,
                bollard::API_DEFAULT_VERSION,
            ) {
                Ok(docker) => return Ok(Self::from_docker(docker)),
                Err(err) => last_err = err.to_string(),
            }
        }
        Err(ContainerError::Engine { detail: last_err })
    }

    fn from_docker(docker: Docker) -> Self {
        Self { docker }
    }

    pub fn container_name(instance: &str, datastore: &str) -> String {
        format!("stackless-{instance}-{datastore}")
    }

    /// Provision a postgres datastore: image pull → volume → container with
    /// instance labels → start → readiness → mapped port.
    pub async fn provision_postgres(
        &self,
        instance: &str,
        datastore: &str,
        version: &str,
    ) -> Result<ProvisionedDatastore, ContainerError> {
        let image = format!("postgres:{version}");
        let name = Self::container_name(instance, datastore);
        let op = |action: &'static str, detail: String| ContainerError::Operation {
            datastore: datastore.to_owned(),
            image: image.clone(),
            action,
            detail,
        };

        // Pull (no-op if present locally).
        use futures_util::TryStreamExt;
        self.docker
            .create_image(
                Some(CreateImageOptions {
                    from_image: Some(image.clone()),
                    ..Default::default()
                }),
                None,
                None,
            )
            .try_collect::<Vec<_>>()
            .await
            .map_err(|err| op("pull", err.to_string()))?;

        let labels = HashMap::from([
            (INSTANCE_LABEL.to_owned(), instance.to_owned()),
            (DATASTORE_LABEL.to_owned(), datastore.to_owned()),
        ]);

        // Reconcile against observation: a same-named container that the
        // journal does not know about is residue of a lost state store —
        // ours (by label) gets replaced; anything else is refused.
        match self
            .docker
            .inspect_container(&name, None::<InspectContainerOptions>)
            .await
        {
            Ok(existing) => {
                let ours = existing
                    .config
                    .as_ref()
                    .and_then(|c| c.labels.as_ref())
                    .and_then(|l| l.get(INSTANCE_LABEL))
                    .is_some_and(|value| value == instance);
                if !ours {
                    return Err(op(
                        "create container",
                        format!(
                            "a container named {name} exists but was not created by stackless for \
                             this instance; remove or rename it"
                        ),
                    ));
                }
                if let Some(id) = existing.id {
                    self.destroy(instance, datastore, &id).await?;
                }
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(err) => return Err(op("inspect existing", err.to_string())),
        }

        let volume_name = name.clone();
        self.docker
            .create_volume(bollard::models::VolumeCreateRequest {
                name: Some(volume_name.clone()),
                labels: Some(labels.clone()),
                ..Default::default()
            })
            .await
            .map_err(|err| op("create volume", err.to_string()))?;

        // An instance-minted credential: protected by leasing alone (§5) —
        // it dies with the instance and is useless outside it.
        let password: String = rand::rng()
            .sample_iter(Alphanumeric)
            .take(24)
            .map(char::from)
            .collect();

        let config = ContainerCreateBody {
            image: Some(image.clone()),
            env: Some(vec![format!("POSTGRES_PASSWORD={password}")]),
            labels: Some(labels),
            host_config: Some(HostConfig {
                binds: Some(vec![format!("{volume_name}:/var/lib/postgresql/data")]),
                port_bindings: Some(HashMap::from([(
                    "5432/tcp".to_owned(),
                    Some(vec![PortBinding {
                        host_ip: Some("127.0.0.1".to_owned()),
                        host_port: Some("0".to_owned()),
                    }]),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let created = self
            .docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(name.clone()),
                    ..Default::default()
                }),
                config,
            )
            .await
            .map_err(|err| op("create container", err.to_string()))?;
        self.docker
            .start_container(&created.id, None::<StartContainerOptions>)
            .await
            .map_err(|err| op("start", err.to_string()))?;

        // Readiness is built in per engine (§7): the real check, not
        // TCP-open — pg_isready inside the container.
        wait_pg_ready(&self.docker, &created.id, datastore).await?;

        let port = mapped_port(&self.docker, &created.id)
            .await
            .ok_or_else(|| op("read mapped port", "no 5432/tcp binding".into()))?;
        let url = format!("postgres://postgres:{password}@127.0.0.1:{port}/postgres");
        Ok(ProvisionedDatastore {
            container_id: ContainerId::try_new(created.id).map_err(|err| {
                op("read container id", err.to_string())
            })?,
            port: TcpPort::from_os(port),
            url,
        })
    }

    /// Is the recorded container still there and running?
    pub async fn observe(&self, container_id: &str) -> Result<bool, ContainerError> {
        match self
            .docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
        {
            Ok(inspect) => Ok(inspect
                .state
                .and_then(|state| state.running)
                .unwrap_or(false)),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(false),
            Err(err) => Err(ContainerError::Engine {
                detail: err.to_string(),
            }),
        }
    }

    /// Remove container and volume; both confirmed gone by the caller's
    /// observe round-trip.
    pub async fn destroy(
        &self,
        instance: &str,
        datastore: &str,
        container_id: &str,
    ) -> Result<(), ContainerError> {
        let _ = self
            .docker
            .stop_container(
                container_id,
                Some(StopContainerOptions {
                    t: Some(5),
                    ..Default::default()
                }),
            )
            .await;
        match self
            .docker
            .remove_container(
                container_id,
                Some(RemoveContainerOptions {
                    force: true,
                    v: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => {}
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => {}
            Err(err) => {
                return Err(ContainerError::Operation {
                    datastore: datastore.to_owned(),
                    image: String::new(),
                    action: "remove container",
                    detail: err.to_string(),
                });
            }
        }
        match self
            .docker
            .remove_volume(
                &Self::container_name(instance, datastore),
                None::<bollard::query_parameters::RemoveVolumeOptions>,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(()),
            Err(err) => Err(ContainerError::Operation {
                datastore: datastore.to_owned(),
                image: String::new(),
                action: "remove volume",
                detail: err.to_string(),
            }),
        }
    }

    /// Volume presence — the teardown survivor check covers state, not
    /// just runtime.
    pub async fn volume_exists(
        &self,
        instance: &str,
        datastore: &str,
    ) -> Result<bool, ContainerError> {
        let volumes = self
            .docker
            .list_volumes(None::<ListVolumesOptions>)
            .await
            .map_err(|err| ContainerError::Engine {
                detail: err.to_string(),
            })?;
        let name = Self::container_name(instance, datastore);
        Ok(volumes
            .volumes
            .unwrap_or_default()
            .iter()
            .any(|volume| volume.name == name))
    }
}

#[derive(Debug)]
pub struct ProvisionedDatastore {
    pub container_id: ContainerId,
    pub port: TcpPort,
    pub url: String,
}

pub fn connect() -> Result<Docker, ContainerError> {
    ContainerRunner::connect().map(|runner| runner.docker)
}

pub fn container_name(instance: &str, datastore: &str) -> String {
    ContainerRunner::container_name(instance, datastore)
}

pub async fn provision_postgres(
    instance: &str,
    datastore: &str,
    version: &str,
) -> Result<ProvisionedDatastore, ContainerError> {
    ContainerRunner::connect()?
        .provision_postgres(instance, datastore, version)
        .await
}

pub async fn observe(container_id: &str) -> Result<bool, ContainerError> {
    ContainerRunner::connect()?
        .observe(container_id)
        .await
}

pub async fn destroy(
    instance: &str,
    datastore: &str,
    container_id: &str,
) -> Result<(), ContainerError> {
    ContainerRunner::connect()?
        .destroy(instance, datastore, container_id)
        .await
}

pub async fn volume_exists(instance: &str, datastore: &str) -> Result<bool, ContainerError> {
    ContainerRunner::connect()?
        .volume_exists(instance, datastore)
        .await
}

async fn wait_pg_ready(
    docker: &Docker,
    container_id: &str,
    datastore: &str,
) -> Result<(), ContainerError> {
    use bollard::exec::CreateExecOptions;
    let budget = Duration::from_secs(60);
    let deadline = Instant::now() + budget;
    let mut detail = String::from("no check completed");
    while Instant::now() < deadline {
        let exec = docker
            .create_exec(
                container_id,
                CreateExecOptions {
                    cmd: Some(vec![
                        "pg_isready".to_owned(),
                        "-U".to_owned(),
                        "postgres".to_owned(),
                    ]),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await;
        if let Ok(exec) = exec
            && docker.start_exec(&exec.id, None).await.is_ok()
        {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Ok(inspect) = docker.inspect_exec(&exec.id).await
                && inspect.exit_code == Some(0)
            {
                return Ok(());
            }
            detail = "pg_isready not yet accepting connections".into();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(ContainerError::NotReady {
        datastore: datastore.to_owned(),
        budget_secs: budget.as_secs(),
        detail,
    })
}

async fn mapped_port(docker: &Docker, container_id: &str) -> Option<u16> {
    let inspect = docker
        .inspect_container(container_id, None::<InspectContainerOptions>)
        .await
        .ok()?;
    inspect
        .network_settings?
        .ports?
        .get("5432/tcp")?
        .as_ref()?
        .first()?
        .host_port
        .as_ref()?
        .parse()
        .ok()
}