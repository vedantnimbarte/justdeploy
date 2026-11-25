//! Pure builders for container definitions, kept free of I/O so they can be tested.

use super::{LABEL_DEPLOYMENT, LABEL_KIND, LABEL_PR, LABEL_PROJECT, LABEL_SERVICE, MANAGED};
use crate::{
    engines,
    models::{Deployment, Service, Volume},
    names,
};
use bollard::models::{
    ContainerCreateBody, EndpointPortConfig, EndpointPortConfigPublishModeEnum, EndpointSettings,
    EndpointSpec, HostConfig, HostConfigLogConfig, Limit, Mount, MountType,
    NetworkAttachmentConfig, NetworkingConfig, RestartPolicy, RestartPolicyNameEnum, ServiceSpec,
    ServiceSpecMode, ServiceSpecModeReplicated, TaskSpec, TaskSpecContainerSpec, TaskSpecLogDriver,
    TaskSpecPlacement, TaskSpecResources, TaskSpecRestartPolicy,
    TaskSpecRestartPolicyConditionEnum,
};
use std::collections::HashMap;

pub struct Release<'a> {
    pub project_name: &'a str,
    pub service: &'a Service,
    pub deployment: &'a Deployment,
    pub image: &'a str,
    pub env: &'a [(String, String)],
    /// Hostnames routed to this deployment; empty for private services.
    pub hosts: &'a [String],
    pub volumes: &'a [Volume],
    pub project_network: &'a str,
    /// The overlay Traefik reaches routed services on.
    pub proxy_network: &'a str,
    pub tls: bool,
    /// Copies to run.
    pub replicas: i64,
    /// Swarm placement constraints, like `node.id==abc`.
    pub constraints: Vec<String>,
    /// The container's arguments, when they differ from the image's.
    pub args: Option<Vec<String>>,
    /// Where OpenTelemetry SDKs send traces; only routed services can reach it.
    pub otlp_endpoint: &'a str,
}

impl Release<'_> {
    /// The Swarm service's name: one per deployment, so a new one starts beside
    /// the old and takes over once it's healthy.
    pub fn name(&self) -> String {
        names::container(self.project_name, &self.service.name, self.deployment.id)
    }

    pub fn routed(&self) -> bool {
        !self.hosts.is_empty() && self.service.port.is_some()
    }

    fn labels(&self) -> HashMap<String, String> {
        let service = self.service;
        let mut labels = HashMap::from([
            (MANAGED.to_string(), "true".to_string()),
            (LABEL_PROJECT.to_string(), service.project_id.to_string()),
            (LABEL_SERVICE.to_string(), service.id.to_string()),
            (LABEL_DEPLOYMENT.to_string(), self.deployment.id.to_string()),
            (LABEL_KIND.to_string(), self.deployment.kind.clone()),
        ]);
        if let Some(pr) = self.deployment.pr_number {
            labels.insert(LABEL_PR.to_string(), pr.to_string());
        }
        labels
    }

    pub fn spec(&self) -> ServiceSpec {
        let service = self.service;
        let pr = self.deployment.pr_number;

        // Copies carry the same jd labels as the service, so their containers
        // can be found on whichever server they land.
        let container_labels = self.labels();
        let mut labels = self.labels();
        if let (true, Some(port)) = (self.routed(), service.port) {
            let router = names::router(self.deployment.id);
            labels.extend(traefik_labels(
                &router,
                self.hosts,
                port,
                self.proxy_network,
                self.tls,
            ));
            // The newest deployment wins as soon as Traefik sees it, so traffic
            // moves over before the previous one is removed.
            labels.insert(
                format!("traefik.http.routers.{router}.priority"),
                self.deployment.created_at.timestamp().to_string(),
            );
        }

        let mut env: Vec<String> = self.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        if let (Some(port), true) = (service.port, service.kind != "database") {
            if !self.env.iter().any(|(k, _)| k == "PORT") {
                env.push(format!("PORT={port}"));
            }
        }
        // OpenTelemetry SDKs read these, so an instrumented app's traces reach
        // justdeploy without setup. Only routed services are on jd-mesh with it.
        if self.routed() {
            let tag = format!("jd.deployment={}", self.deployment.id);
            for (key, value) in [
                ("OTEL_EXPORTER_OTLP_ENDPOINT", self.otlp_endpoint),
                ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
                ("OTEL_SERVICE_NAME", &service.name),
                ("OTEL_RESOURCE_ATTRIBUTES", &tag),
            ] {
                match env.iter_mut().find(|e| e.split('=').next() == Some(key)) {
                    // The app's own attributes stay; the deployment is added.
                    Some(own) if key == "OTEL_RESOURCE_ATTRIBUTES" => {
                        own.push_str(&format!(",{tag}"));
                    }
                    Some(_) => {}
                    None => env.push(format!("{key}={value}")),
                }
            }
        }

        // The service name is an alias on the project network, so siblings reach
        // it as `api:8080` whichever deployment is live. Previews get none, so
        // they never answer for production.
        let mut networks = vec![NetworkAttachmentConfig {
            target: Some(self.project_network.to_string()),
            aliases: pr.is_none().then(|| vec![service.name.clone()]),
            ..Default::default()
        }];
        if self.routed() {
            networks.push(NetworkAttachmentConfig {
                target: Some(self.proxy_network.to_string()),
                ..Default::default()
            });
        }

        let endpoint_spec = match (service.public_port, service.port, pr) {
            (Some(host_port), Some(port), None) => Some(EndpointSpec {
                ports: Some(vec![EndpointPortConfig {
                    target_port: Some(port as i64),
                    published_port: Some(host_port as i64),
                    publish_mode: Some(EndpointPortConfigPublishModeEnum::INGRESS),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            _ => None,
        };

        let logs = bounded_logs();
        ServiceSpec {
            name: Some(self.name()),
            labels: Some(labels),
            task_template: Some(TaskSpec {
                container_spec: Some(TaskSpecContainerSpec {
                    image: Some(self.image.to_string()),
                    env: Some(env),
                    labels: Some(container_labels),
                    args: self.args.clone(),
                    mounts: Some(
                        self.volumes
                            .iter()
                            .map(|v| Mount {
                                target: Some(v.mount_path.clone()),
                                source: Some(v.name.clone()),
                                typ: Some(MountType::VOLUME),
                                ..Default::default()
                            })
                            .collect(),
                    ),
                    ..Default::default()
                }),
                resources: Some(TaskSpecResources {
                    limits: Some(Limit {
                        nano_cpus: service.cpu_millis.map(|m| m as i64 * 1_000_000),
                        memory_bytes: service.memory_mb.map(|m| m as i64 * 1024 * 1024),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                restart_policy: Some(TaskSpecRestartPolicy {
                    condition: Some(TaskSpecRestartPolicyConditionEnum::ANY),
                    delay: Some(5_000_000_000),
                    ..Default::default()
                }),
                placement: Some(TaskSpecPlacement {
                    constraints: Some(self.constraints.clone()),
                    ..Default::default()
                }),
                networks: Some(networks),
                log_driver: Some(TaskSpecLogDriver {
                    name: logs.typ,
                    options: logs.config,
                }),
                ..Default::default()
            }),
            mode: Some(ServiceSpecMode {
                replicated: Some(ServiceSpecModeReplicated {
                    replicas: Some(self.replicas),
                }),
                ..Default::default()
            }),
            endpoint_spec,
            ..Default::default()
        }
    }
}

pub fn traefik_labels(
    router: &str,
    hosts: &[String],
    port: i32,
    proxy_network: &str,
    tls: bool,
) -> HashMap<String, String> {
    let rule = hosts
        .iter()
        .map(|h| format!("Host(`{h}`)"))
        .collect::<Vec<_>>()
        .join(" || ");
    let mut labels = HashMap::from([
        ("traefik.enable".to_string(), "true".to_string()),
        (
            "traefik.swarm.network".to_string(),
            proxy_network.to_string(),
        ),
        (format!("traefik.http.routers.{router}.rule"), rule),
        (
            format!("traefik.http.routers.{router}.service"),
            router.to_string(),
        ),
        // Retries a request whose connection was refused on another container,
        // which is what makes the switch between deployments seamless.
        (
            format!("traefik.http.middlewares.{router}-retry.retry.attempts"),
            "3".to_string(),
        ),
        (
            format!("traefik.http.middlewares.{router}-retry.retry.initialinterval"),
            "100ms".to_string(),
        ),
        (
            format!("traefik.http.routers.{router}.middlewares"),
            format!("{router}-retry"),
        ),
        (
            format!("traefik.http.services.{router}.loadbalancer.server.port"),
            port.to_string(),
        ),
    ]);
    if tls {
        labels.insert(
            format!("traefik.http.routers.{router}.entrypoints"),
            "websecure".into(),
        );
        labels.insert(
            format!("traefik.http.routers.{router}.tls.certresolver"),
            "le".into(),
        );
    } else {
        labels.insert(
            format!("traefik.http.routers.{router}.entrypoints"),
            "web".into(),
        );
    }
    labels
}

/// Keeps a chatty app from filling the disk with logs.
pub fn bounded_logs() -> HostConfigLogConfig {
    HostConfigLogConfig {
        typ: Some("json-file".into()),
        config: Some(HashMap::from([
            ("max-size".to_string(), "10m".to_string()),
            ("max-file".to_string(), "3".to_string()),
        ])),
    }
}

/// A short-lived container from the builder image. The Docker socket is mounted
/// only when it has to build.
pub fn helper(
    image: &str,
    cmd: &[&str],
    env: Vec<String>,
    network: Option<&str>,
    docker_socket: bool,
) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(image.to_string()),
        cmd: Some(cmd.iter().map(|s| s.to_string()).collect()),
        env: Some(env),
        labels: Some(HashMap::from([(MANAGED.to_string(), "helper".to_string())])),
        host_config: Some(HostConfig {
            binds: docker_socket
                .then(|| vec!["/var/run/docker.sock:/var/run/docker.sock".to_string()]),
            network_mode: network.map(str::to_string),
            log_config: Some(bounded_logs()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A pull request's own copy of a database: the same image and credentials, its
/// own volumes, reachable on the project network as `alias`.
pub fn preview_database(
    db: &Service,
    image: &str,
    env: &[(String, String)],
    volumes: &[(String, String)],
    network: &str,
    alias: &str,
    pr: i32,
) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(image.to_string()),
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        labels: Some(HashMap::from([
            (MANAGED.to_string(), "true".to_string()),
            (LABEL_PROJECT.to_string(), db.project_id.to_string()),
            (LABEL_SERVICE.to_string(), db.id.to_string()),
            (LABEL_KIND.to_string(), PREVIEW_DATABASE.to_string()),
            (LABEL_PR.to_string(), pr.to_string()),
        ])),
        cmd: db.engine.as_deref().and_then(engines::command),
        host_config: Some(HostConfig {
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                maximum_retry_count: None,
            }),
            mounts: Some(
                volumes
                    .iter()
                    .map(|(name, path)| Mount {
                        target: Some(path.clone()),
                        source: Some(name.clone()),
                        typ: Some(MountType::VOLUME),
                        ..Default::default()
                    })
                    .collect(),
            ),
            log_config: Some(bounded_logs()),
            ..Default::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                network.to_string(),
                EndpointSettings {
                    aliases: Some(vec![alias.to_string()]),
                    ..Default::default()
                },
            )])),
        }),
        ..Default::default()
    }
}

/// `jd.kind` of a pull request's database copy.
pub const PREVIEW_DATABASE: &str = "preview-db";

/// A container from a service's own image that runs once and exits: a
/// pre-deploy command, a scheduled run or a one-off command. It sits on the
/// project network so it reaches the database, takes no traffic and no
/// volumes, and is labelled a helper so a crash never leaves it behind.
pub fn one_off(
    image: &str,
    command: Option<&str>,
    env: &[(String, String)],
    service: &Service,
    network: &str,
) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(image.to_string()),
        // `sh -c` replaces the entrypoint so the command means the same thing
        // whatever the image's entrypoint does. No command: the image's own.
        entrypoint: command.map(|_| vec!["sh".to_string(), "-c".to_string()]),
        cmd: command.map(|c| vec![c.to_string()]),
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        labels: Some(HashMap::from([(MANAGED.to_string(), "helper".to_string())])),
        host_config: Some(HostConfig {
            network_mode: Some(network.to_string()),
            nano_cpus: service.cpu_millis.map(|m| m as i64 * 1_000_000),
            memory: service.memory_mb.map(|m| m as i64 * 1024 * 1024),
            log_config: Some(bounded_logs()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn preview_databases_answer_on_their_own_alias_and_keep_their_data() {
        let mut db = service();
        db.kind = "database".into();
        db.engine = Some("postgres".into());
        let volumes = vec![(
            "jd-vol-x-pr4".to_string(),
            "/var/lib/postgresql/data".to_string(),
        )];
        let body = preview_database(&db, "postgres:17", &[], &volumes, "jd-proj-1", "db-pr-4", 4);
        let labels = body.labels.unwrap();
        assert_eq!(labels[LABEL_KIND], PREVIEW_DATABASE);
        assert_eq!(labels[LABEL_PR], "4");
        let endpoints = body.networking_config.unwrap().endpoints_config.unwrap();
        assert_eq!(
            endpoints["jd-proj-1"].aliases,
            Some(vec!["db-pr-4".to_string()])
        );
        let mounts = body.host_config.unwrap().mounts.unwrap();
        assert_eq!(mounts[0].source.as_deref(), Some("jd-vol-x-pr4"));
    }

    #[test]
    fn one_off_runs_its_command_through_a_shell_on_the_project_network() {
        let s = service();
        let body = one_off(
            "jd-x:1",
            Some("npm run migrate"),
            &[("A".into(), "1".into())],
            &s,
            "jd-proj-1",
        );
        assert_eq!(
            body.entrypoint,
            Some(vec!["sh".to_string(), "-c".to_string()])
        );
        assert_eq!(body.cmd, Some(vec!["npm run migrate".to_string()]));
        assert_eq!(body.labels.unwrap()[MANAGED], "helper");
        let host = body.host_config.unwrap();
        assert_eq!(host.network_mode.as_deref(), Some("jd-proj-1"));
        assert_eq!(host.memory, Some(256 * 1024 * 1024));

        let default = one_off("jd-x:1", None, &[], &s, "jd-proj-1");
        assert_eq!(
            (default.entrypoint, default.cmd),
            (None, None),
            "no command keeps the image's own"
        );
    }

    pub(crate) fn service() -> Service {
        Service {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            name: "api".into(),
            kind: "app".into(),
            repo_url: None,
            branch: None,
            root_dir: String::new(),
            git_token_enc: None,
            build_type: "auto".into(),
            dockerfile_path: "Dockerfile".into(),
            image: None,
            engine: None,
            port: Some(8080),
            public: true,
            health_path: None,
            cpu_millis: Some(500),
            memory_mb: Some(256),
            public_port: None,
            preview_enabled: false,
            webhook_secret: String::new(),
            desired_state: "running".into(),
            current_deployment_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            pre_deploy_command: None,
            cron_schedule: None,
            replicas: 2,
            autoscale_min: None,
            autoscale_max: None,
            autoscale_cpu: 70,
            node_id: None,
            node_label: None,
            on_status_page: false,
            pitr_destination_id: None,
            pitr_base_backup_at: None,
            restore_from: None,
            restore_target: None,
            replica_of: None,
        }
    }

    fn deployment(pr: Option<i32>) -> Deployment {
        Deployment {
            id: Uuid::nil(),
            service_id: Uuid::nil(),
            kind: if pr.is_some() {
                "preview"
            } else {
                "production"
            }
            .into(),
            pr_number: pr,
            repo_url: None,
            branch: None,
            commit_sha: None,
            commit_message: None,
            image_ref: None,
            status: "deploying".into(),
            trigger: "manual".into(),
            error: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
        }
    }

    fn release<'a>(s: &'a Service, d: &'a Deployment, hosts: &'a [String]) -> Release<'a> {
        Release {
            project_name: "shop",
            service: s,
            deployment: d,
            image: "jd-x:1",
            env: &[],
            hosts,
            volumes: &[],
            project_network: "jd-proj-1",
            proxy_network: "jd-mesh",
            tls: false,
            replicas: 2,
            constraints: vec!["node.id==n1".into()],
            args: None,
            otlp_endpoint: "http://jd:4318",
        }
    }

    fn container(spec: &ServiceSpec) -> TaskSpecContainerSpec {
        spec.task_template.clone().unwrap().container_spec.unwrap()
    }

    fn networks(spec: &ServiceSpec) -> Vec<NetworkAttachmentConfig> {
        spec.task_template.clone().unwrap().networks.unwrap()
    }

    #[test]
    fn public_service_gets_routed_with_its_port_limits_copies_and_placement() {
        let (s, d) = (service(), deployment(None));
        let hosts = vec!["api-shop.example.com".to_string()];
        let spec = release(&s, &d, &hosts).spec();
        let labels = spec.labels.clone().unwrap();
        let router = names::router(d.id);
        assert_eq!(
            labels[&format!("traefik.http.routers.{router}.rule")],
            "Host(`api-shop.example.com`)"
        );
        assert_eq!(
            labels[&format!("traefik.http.routers.{router}.priority")],
            d.created_at.timestamp().to_string(),
            "newer deployments must outrank older ones"
        );
        assert_eq!(labels["traefik.swarm.network"], "jd-mesh");
        assert!(container(&spec)
            .env
            .unwrap()
            .contains(&"PORT=8080".to_string()));
        assert_eq!(
            container(&spec).labels.unwrap()[LABEL_SERVICE],
            s.id.to_string()
        );
        let task = spec.task_template.clone().unwrap();
        let limits = task.resources.unwrap().limits.unwrap();
        assert_eq!(limits.nano_cpus, Some(500_000_000));
        assert_eq!(limits.memory_bytes, Some(256 * 1024 * 1024));
        assert_eq!(
            task.placement.unwrap().constraints,
            Some(vec!["node.id==n1".into()])
        );
        assert_eq!(
            spec.mode.clone().unwrap().replicated.unwrap().replicas,
            Some(2)
        );
        let nets = networks(&spec);
        assert_eq!(nets[0].aliases, Some(vec!["api".to_string()]));
        assert_eq!(nets[1].target.as_deref(), Some("jd-mesh"));
    }

    #[test]
    fn private_service_has_no_route_and_stays_off_the_proxy_network() {
        let (s, d) = (service(), deployment(None));
        let spec = release(&s, &d, &[]).spec();
        assert!(!spec.labels.unwrap().contains_key("traefik.enable"));
        assert_eq!(networks(&release(&s, &d, &[]).spec()).len(), 1);
    }

    #[test]
    fn routed_services_are_told_where_to_send_traces() {
        let (s, d) = (service(), deployment(None));
        let hosts = vec!["api-shop.example.com".to_string()];
        let env = container(&release(&s, &d, &hosts).spec()).env.unwrap();
        assert!(env.contains(&"OTEL_EXPORTER_OTLP_ENDPOINT=http://jd:4318".to_string()));
        assert!(env.contains(&format!("OTEL_SERVICE_NAME={}", s.name)));
        assert!(env.contains(&format!("OTEL_RESOURCE_ATTRIBUTES=jd.deployment={}", d.id)));

        // The app's own attributes and endpoint stay; the deployment is added.
        let own = [
            (
                "OTEL_RESOURCE_ATTRIBUTES".to_string(),
                "team=web".to_string(),
            ),
            (
                "OTEL_EXPORTER_OTLP_ENDPOINT".to_string(),
                "http://mine:4318".to_string(),
            ),
        ];
        let mut r = release(&s, &d, &hosts);
        r.env = &own;
        let env = container(&r.spec()).env.unwrap();
        assert!(env.contains(&format!(
            "OTEL_RESOURCE_ATTRIBUTES=team=web,jd.deployment={}",
            d.id
        )));
        assert!(env.contains(&"OTEL_EXPORTER_OTLP_ENDPOINT=http://mine:4318".to_string()));
        assert_eq!(
            env.iter()
                .filter(|e| e.starts_with("OTEL_EXPORTER_OTLP_ENDPOINT="))
                .count(),
            1
        );

        // Private services can't reach the receiver, so they aren't pointed at it.
        let env = container(&release(&s, &d, &[]).spec())
            .env
            .unwrap_or_default();
        assert!(!env.iter().any(|e| e.starts_with("OTEL_")));
    }

    #[test]
    fn previews_route_only_their_own_host_and_get_no_alias() {
        let (s, d) = (service(), deployment(Some(4)));
        let hosts = vec!["pr-4-api-shop.example.com".to_string()];
        let spec = release(&s, &d, &hosts).spec();
        let labels = spec.labels.clone().unwrap();
        assert_eq!(labels[LABEL_PR], "4");
        assert_eq!(
            labels[&format!("traefik.http.routers.{}.rule", names::router(d.id))],
            "Host(`pr-4-api-shop.example.com`)"
        );
        assert_eq!(networks(&spec)[0].aliases, None);
    }

    #[test]
    fn host_ports_publish_through_the_swarm() {
        let mut s = service();
        s.public_port = Some(15432);
        let d = deployment(None);
        let ports = release(&s, &d, &[])
            .spec()
            .endpoint_spec
            .unwrap()
            .ports
            .unwrap();
        assert_eq!(ports[0].published_port, Some(15432));
        assert_eq!(ports[0].target_port, Some(8080));
    }

    #[test]
    fn tls_uses_the_secure_entrypoint_and_resolver() {
        let labels = traefik_labels("r", &["a.example.com".into()], 80, "jd-proxy", true);
        assert_eq!(labels["traefik.http.routers.r.entrypoints"], "websecure");
        assert_eq!(labels["traefik.http.routers.r.tls.certresolver"], "le");
    }
}
