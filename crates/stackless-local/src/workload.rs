//! Docker workloads on an isolated network, with journaled create and start boundaries.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum, EndpointSettings,
    HostConfig, HostConfigLogConfig, Mount, MountType, NetworkCreateRequest, NetworkingConfig,
    PortBinding,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, InspectContainerOptions, InspectNetworkOptions,
    LogsOptions, RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use stackless_core::engine::StepKind;
use stackless_core::state::{Checkpoint, Ownership, ResourceIntent, ResourcePhase, Store};
use stackless_core::substrate::{
    InstanceContext, Observation, StepContext, StepResource, Substrate, SubstrateFault,
};
use stackless_core::types::{ProxyHost, TcpPort};
use stackless_daemon::rpc::Request;

use crate::{LocalSubstrate, SUBSTRATE_NAME};

pub const KIND: &str = "docker-workload";
pub const INGRESS_KIND: &str = "docker-ingress";
pub const NETWORK_KIND: &str = "docker-network";
const OWNER: &str = "dev.stackless.owner";
const KEY: &str = "dev.stackless.resource";
const PORT: u16 = 8080;
// Multi-platform manifest pinned after the ingress isolation smoke.
const INGRESS_IMAGE: &str =
    "nginx@sha256:a8b39bd9cf0f83869a2162827a0caf6137ddf759d50a171451b335cecc87d236";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerCheckpoint {
    pub owner_id: String,
    pub key: String,
    pub name: String,
    pub container_id: Option<String>,
    pub image_id: String,
    pub finite: bool,
    pub deadline: i64,
    pub exit_code: Option<i64>,
    pub port: Option<TcpPort>,
    pub hosts: Vec<ProxyHost>,
    pub config_dir: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct NetworkCheckpoint {
    owner_id: String,
    name: String,
    id: Option<String>,
}

fn fault(code: &str, message: impl Into<String>) -> SubstrateFault {
    SubstrateFault {
        code: code.into(),
        message: message.into(),
        remediation: "inspect the recorded Docker resource and retry the operation".into(),
        context: Box::default(),
    }
}
fn api(error: impl std::fmt::Display) -> SubstrateFault {
    fault("local.docker.api", error.to_string())
}
fn state(error: stackless_core::state::StateError) -> SubstrateFault {
    SubstrateFault::from_fault(&error)
}
fn encode(value: &impl Serialize) -> Result<String, SubstrateFault> {
    serde_json::to_string(value).map_err(api)
}
fn labels(owner: &str, key: &str) -> HashMap<String, String> {
    [(OWNER.into(), owner.into()), (KEY.into(), key.into())].into()
}
fn missing(error: &bollard::errors::Error) -> bool {
    matches!(
        error,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

pub fn connect() -> Result<Docker, SubstrateFault> {
    if std::env::var("DOCKER_HOST").is_ok_and(|host| !host.starts_with("unix://")) {
        return Err(fault(
            "sandbox.remote_docker",
            "container workloads require a Docker engine on the controller host",
        ));
    }
    crate::container::ContainerRunner::connect()
        .map(|runner| runner.docker.with_timeout(Duration::from_secs(15)))
        .map_err(api)
}

async fn inspect(
    docker: &Docker,
    payload: &DockerCheckpoint,
) -> Result<Option<ContainerInspectResponse>, SubstrateFault> {
    let identity = payload.container_id.as_deref().unwrap_or(&payload.name);
    match docker
        .inspect_container(identity, None::<InspectContainerOptions>)
        .await
    {
        Ok(container) => {
            let configured = container
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref());
            if configured.and_then(|map| map.get(OWNER)) != Some(&payload.owner_id)
                || configured.and_then(|map| map.get(KEY)) != Some(&payload.key)
            {
                return Err(fault(
                    "ownership.refused",
                    format!("Docker container {identity} does not belong to the recorded owner"),
                ));
            }
            Ok(Some(container))
        }
        Err(error) if missing(&error) => Ok(None),
        Err(error) => Err(api(error)),
    }
}

async fn image_id(docker: &Docker, image: &str) -> Result<String, SubstrateFault> {
    match docker.inspect_image(image).await {
        Ok(info) => return info.id.ok_or_else(|| api("Docker image has no ID")),
        Err(error) if missing(&error) => (),
        Err(error) => return Err(api(error)),
    }
    let mut pull = docker.create_image(
        Some(CreateImageOptions {
            from_image: Some(image.into()),
            ..Default::default()
        }),
        None,
        None,
    );
    while let Some(result) = pull.next().await {
        result.map_err(api)?;
    }
    docker
        .inspect_image(image)
        .await
        .map_err(api)?
        .id
        .ok_or_else(|| api("pulled image has no ID"))
}

async fn network(
    docker: &Docker,
    ctx: &StepContext<'_>,
    root: &Path,
) -> Result<String, SubstrateFault> {
    let lock_path = root
        .join("locks")
        .join(format!("docker-network-{}", ctx.instance.id));
    let started = Instant::now();
    let _lock = loop {
        match stackless_core::lockfile::FileLock::try_acquire(&lock_path) {
            Ok(lock) => break lock,
            Err(stackless_core::lockfile::LockError::Held { .. })
                if started.elapsed() < Duration::from_secs(30) =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await
            }
            Err(error) => return Err(api(error)),
        }
    };
    let key = "docker:network";
    let initial = NetworkCheckpoint {
        owner_id: ctx.instance.id.into(),
        name: format!("{}-network", ctx.instance.resource_namespace),
        id: None,
    };
    let record = ctx
        .store
        .resource_intent(ResourceIntent {
            owner_id: ctx.instance.id,
            key,
            step_id: stackless_core::state::INSTANCE_RESOURCE_STEP,
            provider: SUBSTRATE_NAME,
            ownership: Ownership::Owned,
            resource_kind: NETWORK_KIND,
            resource_id: &initial.name,
            payload: &encode(&initial)?,
            dependencies: &[],
        })
        .map_err(state)?;
    let mut payload: NetworkCheckpoint = serde_json::from_str(&record.payload).map_err(api)?;
    let found = match docker
        .inspect_network(
            payload.id.as_deref().unwrap_or(&payload.name),
            None::<InspectNetworkOptions>,
        )
        .await
    {
        Ok(found) => Some(found),
        Err(error) if missing(&error) => None,
        Err(error) => return Err(api(error)),
    };
    if let Some(found) = found {
        if found.labels.as_ref().and_then(|map| map.get(OWNER)) != Some(&payload.owner_id)
            || found.internal != Some(true)
            || found
                .options
                .as_ref()
                .and_then(|map| map.get("com.docker.network.bridge.gateway_mode_ipv4"))
                .map(String::as_str)
                != Some("isolated")
        {
            return Err(fault(
                "sandbox.network_changed",
                "recorded Docker network no longer has the required owner and isolation settings",
            ));
        }
        payload.id = found.id;
    } else {
        if record.phase != ResourcePhase::Intent {
            return Err(fault(
                "sandbox.network_missing",
                "recorded Docker network was removed",
            ));
        }
        let created = docker
            .create_network(NetworkCreateRequest {
                name: payload.name.clone(),
                driver: Some("bridge".into()),
                internal: Some(true),
                enable_ipv6: Some(false),
                options: Some(
                    [(
                        "com.docker.network.bridge.gateway_mode_ipv4".into(),
                        "isolated".into(),
                    )]
                    .into(),
                ),
                labels: Some(labels(ctx.instance.id, key)),
                ..Default::default()
            })
            .await
            .map_err(api)?;
        payload.id = Some(created.id);
    }
    if record.phase == ResourcePhase::Intent {
        ctx.store
            .resource_created(
                ctx.instance.id,
                key,
                payload
                    .id
                    .as_deref()
                    .ok_or_else(|| api("network ID is missing"))?,
                &encode(&payload)?,
            )
            .map_err(state)?;
        ctx.store
            .resource_ready(ctx.instance.id, key)
            .map_err(state)?;
    }
    Ok(payload.name)
}

fn hardened_config(
    image: &str,
    network: &str,
    env: &BTreeMap<String, String>,
    workspace: Option<&Path>,
) -> ContainerCreateBody {
    let uid = 65532;
    let gid = 65532;
    ContainerCreateBody {
        image: Some(image.into()),
        user: Some(format!("{uid}:{gid}")),
        working_dir: workspace.map(|_| "/app".into()),
        env: Some(
            env.iter()
                .map(|(key, value)| format!("{key}={value}"))
                .chain(["HOME=/tmp".into()])
                .collect(),
        ),
        host_config: Some(HostConfig {
            network_mode: Some(network.into()),
            readonly_rootfs: Some(true),
            privileged: Some(false),
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges:true".into()]),
            memory: Some(512 * 1024 * 1024),
            memory_swap: Some(512 * 1024 * 1024),
            nano_cpus: Some(1_000_000_000),
            pids_limit: Some(128),
            init: Some(true),
            ipc_mode: Some("private".into()),
            dns: Some(vec!["127.0.0.1".into()]),
            tmpfs: Some(
                [(
                    "/tmp".into(),
                    "rw,noexec,nosuid,nodev,size=64m,mode=1777".into(),
                )]
                .into(),
            ),
            mounts: workspace.map(|path| {
                vec![Mount {
                    typ: Some(MountType::BIND),
                    source: Some(path.display().to_string()),
                    target: Some("/app".into()),
                    read_only: Some(false),
                    ..Default::default()
                }]
            }),
            log_config: Some(HostConfigLogConfig {
                typ: Some("json-file".into()),
                config: Some(
                    [
                        ("max-size".into(), "10m".into()),
                        ("max-file".into(), "3".into()),
                    ]
                    .into(),
                ),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

async fn ensure_container(
    docker: &Docker,
    ctx: &StepContext<'_>,
    key: &str,
    kind: &str,
    mut payload: DockerCheckpoint,
    mut config: ContainerCreateBody,
    parents: &[&str],
) -> Result<DockerCheckpoint, SubstrateFault> {
    let record = ctx
        .store
        .resource_intent(ResourceIntent {
            owner_id: ctx.instance.id,
            key,
            step_id: &ctx.step.id,
            provider: SUBSTRATE_NAME,
            ownership: Ownership::Owned,
            resource_kind: kind,
            resource_id: &payload.name,
            payload: &encode(&payload)?,
            dependencies: parents,
        })
        .map_err(state)?;
    payload = serde_json::from_str(&record.payload).map_err(api)?;
    let found = inspect(docker, &payload).await?;
    let container = if let Some(found) = found {
        found
    } else {
        if record.phase != ResourcePhase::Intent {
            return Err(fault(
                "sandbox.container_missing",
                "recorded container disappeared; recovery will not repeat its execution",
            ));
        }
        config.labels = Some(labels(ctx.instance.id, key));
        let created = docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(payload.name.clone()),
                    ..Default::default()
                }),
                config,
            )
            .await
            .map_err(api)?;
        payload.container_id = Some(created.id);
        ctx.store
            .resource_created(
                ctx.instance.id,
                key,
                payload
                    .container_id
                    .as_deref()
                    .ok_or_else(|| api("container ID missing"))?,
                &encode(&payload)?,
            )
            .map_err(state)?;
        inspect(docker, &payload)
            .await?
            .ok_or_else(|| api("created container is missing"))?
    };
    payload.container_id = container.id.clone();
    let id = payload
        .container_id
        .as_deref()
        .ok_or_else(|| api("container ID is missing"))?;
    if record.phase == ResourcePhase::Intent {
        ctx.store
            .resource_created(ctx.instance.id, key, id, &encode(&payload)?)
            .map_err(state)?;
    }
    let status = container.state.as_ref().and_then(|state| state.status);
    if status == Some(ContainerStateStatusEnum::CREATED) {
        docker
            .start_container(id, None::<StartContainerOptions>)
            .await
            .map_err(api)?;
    } else if !payload.finite && status != Some(ContainerStateStatusEnum::RUNNING) {
        return Err(fault(
            "sandbox.workload_stopped",
            "recorded service container has stopped",
        ));
    }
    Ok(payload)
}

impl LocalSubstrate {
    pub(crate) async fn container_execute(
        &self,
        ctx: &StepContext<'_>,
        command: Option<&str>,
    ) -> Result<StepResource, SubstrateFault> {
        let docker = connect()?;
        let network = network(&docker, ctx, &self.state_root).await?;
        let image = ctx.def.services[&ctx.step.node]
            .image
            .as_deref()
            .ok_or_else(|| api("container image missing"))?;
        let image = image_id(&docker, image).await?;
        let hash = stackless_core::engine::revision::digest(&(
            ctx.operation_id,
            &ctx.step.id,
            self.step_revision(ctx)?,
        ))?;
        let key = format!("container:{hash}");
        let finite = ctx.step.kind != StepKind::Start;
        let mut env = self.resolved_env(ctx, &ctx.step.node)?;
        if !finite {
            env.insert("PORT".into(), PORT.to_string());
        }
        let workspace = self.source_dir(ctx, &ctx.step.node)?;
        let spec = &ctx.def.services[&ctx.step.node];
        let has_source = ctx.source_overrides.contains_key(&ctx.step.node)
            || !spec.source.repo.is_empty()
            || spec.source.path.is_some();
        let mut config = hardened_config(
            &image,
            &network,
            &env,
            has_source.then_some(workspace.as_path()),
        );
        if let Some(command) = command {
            config.entrypoint = Some(vec!["/bin/sh".into(), "-c".into()]);
            config.cmd = Some(vec![command.into()]);
        }
        config.networking_config = Some(NetworkingConfig {
            endpoints_config: Some(
                [(
                    network.clone(),
                    EndpointSettings {
                        aliases: (!finite).then(|| vec![format!("sl-{}", ctx.step.node)]),
                        ..Default::default()
                    },
                )]
                .into(),
            ),
        });
        let initial = DockerCheckpoint {
            owner_id: ctx.instance.id.into(),
            key: key.clone(),
            name: format!("{}-{}", ctx.instance.resource_namespace, &hash[..16]),
            container_id: None,
            image_id: image,
            finite,
            deadline: Store::now_secs() + ctx.def.services[&ctx.step.node].timeout_secs as i64,
            exit_code: None,
            port: None,
            config_dir: None,
            hosts: self.service_hosts(ctx.def, ctx.instance.name, &ctx.step.node),
        };
        let mut parents = ctx.parent_resources.to_vec();
        parents.push("docker:network");
        let mut payload =
            ensure_container(&docker, ctx, &key, KIND, initial, config, &parents).await?;
        if finite {
            loop {
                let found = inspect(&docker, &payload).await?.ok_or_else(|| {
                    fault(
                        "job.result_unknown",
                        "job container disappeared before its exit status was recorded",
                    )
                })?;
                let status = found.state.ok_or_else(|| api("container state missing"))?;
                if status.status == Some(ContainerStateStatusEnum::EXITED) {
                    payload.exit_code = status.exit_code;
                    self.capture_container_logs(
                        &docker,
                        &payload,
                        ctx.instance.resource_namespace,
                        &ctx.step.node,
                    )
                    .await?;
                    if payload.exit_code != Some(0) {
                        let mut error = fault(
                            "job.failed",
                            format!("{} exited with status {:?}", ctx.step.id, payload.exit_code),
                        );
                        error.context.service = Some(ctx.step.node.clone());
                        error.context.log_path = Some(
                            self.spawner(ctx.instance.resource_namespace)
                                .log_path(&ctx.step.node)
                                .display()
                                .to_string(),
                        );
                        error.context.log_tail =
                            Some(self.spawner(ctx.instance.resource_namespace).log_tail(
                                &ctx.step.node,
                                stackless_core::fault::FAILURE_LOG_TAIL_LINES,
                            ));
                        error.context.exit_status = payload.exit_code.map(|code| code.to_string());
                        return Err(error);
                    }
                    break;
                }
                if ctx.is_cancelled() || Store::now_secs() >= payload.deadline {
                    docker
                        .stop_container(
                            payload
                                .container_id
                                .as_deref()
                                .ok_or_else(|| api("container ID missing"))?,
                            Some(StopContainerOptions {
                                t: Some(2),
                                ..Default::default()
                            }),
                        )
                        .await
                        .map_err(api)?;
                    return Err(fault(
                        if ctx.is_cancelled() {
                            "operation.cancelled"
                        } else {
                            "job.timeout"
                        },
                        format!("{} stopped before completion", ctx.step.id),
                    ));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        } else if !payload.hosts.is_empty() {
            payload.port = Some(
                self.ensure_ingress(&docker, ctx, &network, &payload, &key)
                    .await?,
            );
            let mut daemon = self.daemon()?;
            for host in &payload.hosts {
                daemon
                    .call(Request::RouteSet {
                        host: host.clone(),
                        port: payload.port.ok_or_else(|| api("ingress port missing"))?,
                    })
                    .map_err(|error| SubstrateFault::from_fault(&error))?;
            }
        }
        if ctx
            .store
            .resource(ctx.instance.id, &key)
            .map_err(state)?
            .is_some_and(|record| record.phase != ResourcePhase::Ready)
        {
            ctx.store
                .resource_created(
                    ctx.instance.id,
                    &key,
                    payload
                        .container_id
                        .as_deref()
                        .ok_or_else(|| api("container ID missing"))?,
                    &encode(&payload)?,
                )
                .map_err(state)?;
        }
        ctx.store
            .resource_ready(ctx.instance.id, &key)
            .map_err(state)?;
        Ok(StepResource {
            resource_kind: KIND.into(),
            resource_id: payload
                .container_id
                .clone()
                .ok_or_else(|| api("container ID missing"))?,
            payload: encode(&payload)?,
        })
    }

    async fn capture_container_logs(
        &self,
        docker: &Docker,
        payload: &DockerCheckpoint,
        namespace: &str,
        service: &str,
    ) -> Result<(), SubstrateFault> {
        use std::io::Write;
        let path = self.spawner(namespace).log_path(service);
        crate::logging::clear_generations(&path).map_err(api)?;
        let mut file = stackless_core::security::private_log(&path, false).map_err(api)?;
        let mut stream = docker.logs(
            payload
                .container_id
                .as_deref()
                .ok_or_else(|| api("container ID missing"))?,
            Some(LogsOptions {
                stdout: true,
                stderr: true,
                tail: "200".into(),
                ..Default::default()
            }),
        );
        let mut bytes = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(api)?.into_bytes();
            bytes += chunk.len();
            if bytes > 1024 * 1024 {
                break;
            }
            file.write_all(&chunk).map_err(api)?;
        }
        Ok(())
    }
}

impl LocalSubstrate {
    async fn ensure_ingress(
        &self,
        docker: &Docker,
        ctx: &StepContext<'_>,
        network: &str,
        app: &DockerCheckpoint,
        app_key: &str,
    ) -> Result<TcpPort, SubstrateFault> {
        let found = inspect(docker, app)
            .await?
            .ok_or_else(|| api("workload disappeared before ingress allocation"))?;
        let address = found
            .network_settings
            .as_ref()
            .and_then(|settings| settings.networks.as_ref())
            .and_then(|networks| networks.get(network))
            .and_then(|endpoint| endpoint.ip_address.as_deref())
            .ok_or_else(|| api("workload has no isolated network address"))?;
        let address: std::net::Ipv4Addr = address.parse().map_err(api)?;
        let image = image_id(docker, INGRESS_IMAGE).await?;
        let key = format!("ingress:{app_key}");
        let hash = stackless_core::engine::revision::digest(&key)?;
        let directory = self
            .state_root
            .join("ingress")
            .join(ctx.instance.resource_namespace)
            .join(&hash);
        let config_path = directory.join("nginx.conf");
        let payload = DockerCheckpoint {
            owner_id: ctx.instance.id.into(),
            key: key.clone(),
            name: format!("{}-ingress-{}", ctx.instance.resource_namespace, &hash[..8]),
            container_id: None,
            image_id: image.clone(),
            finite: false,
            deadline: 0,
            exit_code: None,
            port: None,
            hosts: app.hosts.clone(),
            config_dir: Some(directory.clone()),
        };
        let parents = [app_key, "docker:network"];
        ctx.store
            .resource_intent(ResourceIntent {
                owner_id: ctx.instance.id,
                key: &key,
                step_id: &ctx.step.id,
                provider: SUBSTRATE_NAME,
                ownership: Ownership::Owned,
                resource_kind: INGRESS_KIND,
                resource_id: &payload.name,
                payload: &encode(&payload)?,
                dependencies: &parents,
            })
            .map_err(state)?;
        std::fs::create_dir_all(&directory).map_err(api)?;
        let configuration = format!(
            r#"pid /tmp/nginx.pid;
error_log /dev/stderr warn;
events {{ worker_connections 256; }}
http {{
  access_log off;
  client_body_temp_path /tmp/client;
  proxy_temp_path /tmp/proxy;
  fastcgi_temp_path /tmp/fastcgi;
  uwsgi_temp_path /tmp/uwsgi;
  scgi_temp_path /tmp/scgi;
  map $http_upgrade $connection_upgrade {{ default upgrade; '' close; }}
  server {{
    listen 8080;
    client_max_body_size 10m;
    location / {{
      proxy_pass http://{address}:8080;
      proxy_http_version 1.1;
      proxy_set_header Host $http_host;
      proxy_set_header Upgrade $http_upgrade;
      proxy_set_header Connection $connection_upgrade;
    }}
  }}
}}
"#
        );
        std::fs::write(&config_path, configuration).map_err(api)?;
        let mut config = hardened_config(&image, "bridge", &BTreeMap::new(), None);
        config.working_dir = Some("/tmp".into());
        config.entrypoint = Some(vec!["/usr/sbin/nginx".into()]);
        config.cmd = Some(vec![
            "-g".into(),
            "daemon off;".into(),
            "-c".into(),
            "/etc/stackless/nginx.conf".into(),
        ]);
        config.exposed_ports = Some(vec![format!("{PORT}/tcp")]);
        config.networking_config = Some(NetworkingConfig {
            endpoints_config: Some(
                [
                    (
                        "bridge".into(),
                        EndpointSettings {
                            gw_priority: Some(100),
                            ..Default::default()
                        },
                    ),
                    (network.into(), EndpointSettings::default()),
                ]
                .into(),
            ),
        });
        if let Some(host) = &mut config.host_config {
            host.memory = Some(128 * 1024 * 1024);
            host.memory_swap = host.memory;
            host.nano_cpus = Some(250_000_000);
            host.pids_limit = Some(32);
            host.mounts = Some(vec![Mount {
                typ: Some(MountType::BIND),
                source: Some(config_path.display().to_string()),
                target: Some("/etc/stackless/nginx.conf".into()),
                read_only: Some(true),
                ..Default::default()
            }]);
            host.port_bindings = Some(
                [(
                    format!("{PORT}/tcp"),
                    Some(vec![PortBinding {
                        host_ip: Some("127.0.0.1".into()),
                        host_port: Some("0".into()),
                    }]),
                )]
                .into(),
            );
        }
        let mut payload =
            ensure_container(docker, ctx, &key, INGRESS_KIND, payload, config, &parents).await?;
        let found = inspect(docker, &payload)
            .await?
            .ok_or_else(|| api("ingress container disappeared"))?;
        let binding = found
            .network_settings
            .as_ref()
            .and_then(|settings| settings.ports.as_ref())
            .and_then(|ports| ports.get(&format!("{PORT}/tcp")))
            .and_then(Option::as_ref)
            .and_then(|bindings| bindings.first())
            .filter(|binding| binding.host_ip.as_deref() == Some("127.0.0.1"))
            .and_then(|binding| binding.host_port.as_deref());
        let Some(binding) = binding else {
            let log_name = format!("{}-ingress", ctx.step.node);
            self.capture_container_logs(
                docker,
                &payload,
                ctx.instance.resource_namespace,
                &log_name,
            )
            .await?;
            let mut error = api("ingress has no loopback port binding");
            error.context.log_tail = Some(
                self.spawner(ctx.instance.resource_namespace)
                    .log_tail(&log_name, stackless_core::fault::FAILURE_LOG_TAIL_LINES),
            );
            return Err(error);
        };
        let port = TcpPort::try_new(binding.parse().map_err(api)?).map_err(api)?;
        payload.port = Some(port);
        if ctx
            .store
            .resource(ctx.instance.id, &key)
            .map_err(state)?
            .is_some_and(|record| record.phase != ResourcePhase::Ready)
        {
            ctx.store
                .resource_created(
                    ctx.instance.id,
                    &key,
                    payload
                        .container_id
                        .as_deref()
                        .ok_or_else(|| api("ingress ID missing"))?,
                    &encode(&payload)?,
                )
                .map_err(state)?;
        }
        ctx.store
            .resource_ready(ctx.instance.id, &key)
            .map_err(state)?;
        Ok(port)
    }
}

pub async fn observe(
    instance: &InstanceContext<'_>,
    checkpoint: &Checkpoint,
) -> Result<Observation, SubstrateFault> {
    let docker = connect()?;
    if checkpoint.resource_kind == NETWORK_KIND {
        let payload: NetworkCheckpoint = serde_json::from_str(&checkpoint.payload).map_err(api)?;
        match docker
            .inspect_network(
                payload.id.as_deref().unwrap_or(&payload.name),
                None::<InspectNetworkOptions>,
            )
            .await
        {
            Ok(found) => {
                if payload.owner_id != instance.id
                    || found
                        .labels
                        .as_ref()
                        .and_then(|map| map.get(OWNER))
                        .map(String::as_str)
                        != Some(instance.id)
                {
                    return Err(fault(
                        "ownership.refused",
                        "Docker network owner does not match",
                    ));
                }
                return Ok(Observation::Present);
            }
            Err(error) if missing(&error) => return Ok(Observation::Gone),
            Err(error) => return Err(api(error)),
        }
    }
    let payload: DockerCheckpoint = serde_json::from_str(&checkpoint.payload).map_err(api)?;
    if payload.owner_id != instance.id {
        return Err(fault(
            "ownership.refused",
            "Docker workload owner does not match",
        ));
    }
    let found = inspect(&docker, &payload).await?;
    if let Some(found) = found {
        if !payload.finite
            && found.state.as_ref().and_then(|state| state.status)
                != Some(ContainerStateStatusEnum::RUNNING)
        {
            return Ok(Observation::Drifted {
                settings: vec![stackless_core::substrate::SettingDrift {
                    setting: "runtime".into(),
                    expected: "running".into(),
                    actual: "stopped".into(),
                }],
            });
        }
        return Ok(Observation::Present);
    }
    Ok(stackless_core::substrate::present_or_gone(
        payload.config_dir.as_ref().is_some_and(|dir| dir.exists()),
    ))
}

pub async fn destroy(
    instance: &InstanceContext<'_>,
    checkpoint: &Checkpoint,
) -> Result<(), SubstrateFault> {
    let docker = connect()?;
    if checkpoint.resource_kind == NETWORK_KIND {
        let payload: NetworkCheckpoint = serde_json::from_str(&checkpoint.payload).map_err(api)?;
        if observe(instance, checkpoint).await? == Observation::Gone {
            return Ok(());
        }
        return docker
            .remove_network(payload.id.as_deref().unwrap_or(&payload.name))
            .await
            .map_err(api);
    }
    let payload: DockerCheckpoint = serde_json::from_str(&checkpoint.payload).map_err(api)?;
    if payload.owner_id != instance.id {
        return Err(fault(
            "ownership.refused",
            "Docker workload owner does not match",
        ));
    }
    if let Some(found) = inspect(&docker, &payload).await? {
        docker
            .remove_container(
                found
                    .id
                    .as_deref()
                    .ok_or_else(|| api("container ID missing"))?,
                Some(RemoveContainerOptions {
                    force: true,
                    v: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(api)?;
    }
    if let Some(directory) = payload.config_dir {
        match std::fs::remove_dir_all(directory) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(api(error)),
        }
    }
    Ok(())
}

impl LocalSubstrate {
    pub(crate) async fn wait_container_healthy(
        &self,
        ctx: &StepContext<'_>,
        checkpoint: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        let Some(health) = &ctx.def.services[&ctx.step.node].health else {
            return Ok(());
        };
        let payload: DockerCheckpoint = serde_json::from_str(&checkpoint.payload).map_err(api)?;
        let host = payload
            .hosts
            .first()
            .ok_or_else(|| api("HTTP workload has no ingress host"))?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(api)?;
        let url = format!("http://127.0.0.1:{}{}", self.proxy_port.get(), health.path);
        let started = Instant::now();
        let mut detail = String::from("no response");
        loop {
            if ctx.is_cancelled() {
                return Err(fault("operation.cancelled", "health observation cancelled"));
            }
            if started.elapsed().as_secs() >= ctx.def.services[&ctx.step.node].timeout_secs {
                let docker = connect()?;
                let mut error = fault("local.health.failed", detail);
                let mut logs = Vec::new();
                for record in ctx.store.resources(ctx.instance.id).map_err(state)? {
                    if record.resource_kind == INGRESS_KIND
                        && record.dependencies.contains(&payload.key)
                    {
                        let ingress: DockerCheckpoint =
                            serde_json::from_str(&record.payload).map_err(api)?;
                        let name = format!("{}-ingress", ctx.step.node);
                        self.capture_container_logs(
                            &docker,
                            &ingress,
                            ctx.instance.resource_namespace,
                            &name,
                        )
                        .await?;
                        logs.push(
                            self.spawner(ctx.instance.resource_namespace)
                                .log_tail(&name, stackless_core::fault::FAILURE_LOG_TAIL_LINES),
                        );
                    }
                }
                error.context.log_tail = Some(logs.join("\n"));
                return Err(error);
            }
            if observe(ctx.instance, checkpoint).await? != Observation::Present {
                return Err(fault(
                    "sandbox.workload_stopped",
                    "workload stopped before it became healthy",
                ));
            }
            match crate::health::probe(&client, &url, host.as_str(), health).await {
                Ok(()) => return Ok(()),
                Err(error) => detail = error,
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub(crate) async fn retire_ingress(
        &self,
        ctx: &StepContext<'_>,
        previous: &Checkpoint,
    ) -> Result<(), SubstrateFault> {
        let app: DockerCheckpoint = serde_json::from_str(&previous.payload).map_err(api)?;
        for resource in ctx.store.resources(ctx.instance.id).map_err(state)? {
            if resource.resource_kind == INGRESS_KIND
                && resource.phase != ResourcePhase::Absent
                && resource.dependencies.contains(&app.key)
            {
                let checkpoint = resource.checkpoint(ctx.instance.name);
                destroy(ctx.instance, &checkpoint).await?;
                if observe(ctx.instance, &checkpoint).await? != Observation::Gone {
                    return Err(fault(
                        "engine.teardown_survivors",
                        "old ingress survived replacement",
                    ));
                }
                ctx.store
                    .resource_absent(ctx.instance.id, &resource.key)
                    .map_err(state)?;
            }
        }
        Ok(())
    }
}

impl LocalSubstrate {
    pub(crate) async fn restore_container_routes(
        &self,
        store: &Store,
        instance: &InstanceContext<'_>,
    ) -> Result<(), SubstrateFault> {
        let resources = store.resources(instance.id).map_err(state)?;
        for checkpoint in instance
            .checkpoints
            .iter()
            .filter(|cp| cp.resource_kind == KIND && cp.step_id.starts_with("start:"))
        {
            let payload: DockerCheckpoint =
                serde_json::from_str(&checkpoint.payload).map_err(api)?;
            let Some(port) = payload.port else {
                continue;
            };
            if observe(instance, checkpoint).await? != Observation::Present {
                continue;
            }
            let Some(ingress) = resources.iter().find(|resource| {
                resource.resource_kind == INGRESS_KIND
                    && resource.phase == ResourcePhase::Ready
                    && resource.dependencies.contains(&payload.key)
            }) else {
                continue;
            };
            let ingress_payload: DockerCheckpoint =
                serde_json::from_str(&ingress.payload).map_err(api)?;
            if ingress_payload.port != Some(port)
                || observe(instance, &ingress.checkpoint(instance.name)).await?
                    != Observation::Present
            {
                continue;
            }
            let mut daemon = self.daemon()?;
            for host in payload.hosts {
                daemon
                    .call(Request::RouteSet { host, port })
                    .map_err(|error| SubstrateFault::from_fault(&error))?;
            }
        }
        Ok(())
    }

    pub(crate) async fn refresh_container_logs(
        &self,
        instance: &InstanceContext<'_>,
        checkpoint: &Checkpoint,
        service: &str,
    ) -> Result<(), SubstrateFault> {
        let payload: DockerCheckpoint = serde_json::from_str(&checkpoint.payload).map_err(api)?;
        if payload.owner_id != instance.id {
            return Err(fault(
                "ownership.refused",
                "Docker log owner does not match",
            ));
        }
        let docker = connect()?;
        if inspect(&docker, &payload).await?.is_none() {
            return Err(fault(
                "sandbox.container_missing",
                "cannot fetch logs from a missing container",
            ));
        }
        self.capture_container_logs(&docker, &payload, instance.resource_namespace, service)
            .await
    }
}

pub async fn job_exit(
    instance: &InstanceContext<'_>,
    checkpoint: &Checkpoint,
) -> Result<Option<i64>, SubstrateFault> {
    let payload: DockerCheckpoint = serde_json::from_str(&checkpoint.payload).map_err(api)?;
    if payload.owner_id != instance.id {
        return Err(fault(
            "ownership.refused",
            "Docker job owner does not match",
        ));
    }
    if !payload.finite {
        return Ok(None);
    }
    let docker = connect()?;
    Ok(inspect(&docker, &payload)
        .await?
        .and_then(|found| found.state)
        .filter(|state| state.status == Some(ContainerStateStatusEnum::EXITED))
        .and_then(|state| state.exit_code))
}
