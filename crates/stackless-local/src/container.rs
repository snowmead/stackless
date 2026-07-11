//! Legacy Docker datastore teardown.
//!
//! First-class `[datastores.*]` provisioning is gone, but journals from
//! older `up` runs may still hold `container` checkpoints. Observe and
//! destroy keep those resources reclaimable on `down`.

use bollard::Docker;
use bollard::query_parameters::{
    InspectContainerOptions, ListVolumesOptions, RemoveContainerOptions, StopContainerOptions,
};
use stackless_core::fault::{Fault, codes};

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("cannot reach the Docker engine: {detail}")]
    Engine { detail: String },

    #[error("datastore {datastore:?} failed to {action}: {detail}")]
    Operation {
        datastore: String,
        action: &'static str,
        detail: String,
    },
}

impl Fault for ContainerError {
    fn code(&self) -> &'static str {
        match self {
            Self::Engine { .. } => codes::LOCAL_DOCKER_ENGINE,
            Self::Operation { .. } => codes::LOCAL_DATASTORE_FAILED,
        }
    }

    fn remediation(&self) -> String {
        match self {
            Self::Engine { .. } => "start Docker (or set DOCKER_HOST) and re-run `down`".into(),
            Self::Operation { datastore, .. } => format!(
                "check `docker ps -a` / `docker volume ls` for stackless-*{datastore}*, clean up by hand, then re-run `down`"
            ),
        }
    }
}

/// A connected Docker engine handle used for legacy container teardown.
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
