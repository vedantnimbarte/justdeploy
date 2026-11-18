//! Pure builders for container definitions, kept free of I/O so they can be tested.

use super::{LABEL_DEPLOYMENT, LABEL_KIND, LABEL_PR, LABEL_PROJECT, LABEL_SERVICE, MANAGED};
use crate::{
    engines,
    models::{Deployment, Service, Volume},
    names,
};
use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, HostConfigLogConfig, Mount, MountType,
    NetworkingConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum,
};
use std::collections::HashMap;

pub struct Release<'a> {
    pub project_name: &'a str,
    pub service: &'a Service,
    pub deployment: &'a Deployment,
    pub image: &'a str,
    pub env: &'a [(String, String)],
    /// Hostnames routed to this container; empty for private services.
    pub hosts: &'a [String],
    pub volumes: &'a [Volume],
    pub project_network: &'a str,
    pub proxy_network: &'a str,
    pub tls: bool,
}

impl Release<'_> {
    pub fn container_name(&self) -> String {
        names::container(self.project_name, &self.service.name, self.deployment.id)
    }

    pub fn routed(&self) -> bool {
        !self.hosts.is_empty() && self.service.port.is_some()
    }

    pub fn body(&self) -> ContainerCreateBody {
        let service = self.service;
        let pr = self.deployment.pr_number;

        let mut labels = HashMap::from([
            (MANAGED.to_string(), "true".to_string()),
            (LABEL_PROJECT.to_string(), service.project_id.to_string()),
            (LABEL_SERVICE.to_string(), service.id.to_string()),
            (LABEL_DEPLOYMENT.to_string(), self.deployment.id.to_string()),
            (LABEL_KIND.to_string(), self.deployment.kind.clone()),
        ]);
        if let Some(pr) = pr {
            labels.insert(LABEL_PR.to_string(), pr.to_string());
        }
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
            // moves over before the previous container is stopped.
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

        // The service name is an alias on the project network, so siblings reach
        // it as `api:8080` no matter which deployment is live. Previews get none,
        // so they never answer for production.
        let mut endpoints = HashMap::from([(
            self.project_network.to_string(),
            EndpointSettings {
                aliases: pr.is_none().then(|| vec![service.name.clone()]),
                ..Default::default()
            },
        )]);
        if self.routed() {
            endpoints.insert(self.proxy_network.to_string(), EndpointSettings::default());
        }

        let mut exposed = None;
        let mut port_bindings = None;
        if let (Some(host_port), Some(port), None) = (service.public_port, service.port, pr) {
            let key = format!("{port}/tcp");
            exposed = Some(vec![key.clone()]);
            port_bindings = Some(HashMap::from([(
                key,
                Some(vec![PortBinding {
                    host_ip: Some("0.0.0.0".into()),
                    host_port: Some(host_port.to_string()),
                }]),
            )]));
        }

        ContainerCreateBody {
            image: Some(self.image.to_string()),
            env: Some(env),
            labels: Some(labels),
            cmd: service.engine.as_deref().and_then(engines::command),
            exposed_ports: exposed,
            host_config: Some(HostConfig {
                restart_policy: Some(RestartPolicy {
                    name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                    maximum_retry_count: None,
                }),
                nano_cpus: service.cpu_millis.map(|m| m as i64 * 1_000_000),
                memory: service.memory_mb.map(|m| m as i64 * 1024 * 1024),
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
                port_bindings,
                log_config: Some(bounded_logs()),
                ..Default::default()
            }),
            networking_config: Some(NetworkingConfig {
                endpoints_config: Some(endpoints),
            }),
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
            "traefik.docker.network".to_string(),
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
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn one_off_runs_its_command_through_a_shell_on_the_project_network() {
        let s = service();
        let body = one_off("jd-x:1", Some("npm run migrate"), &[("A".into(), "1".into())], &s, "jd-proj-1");
        assert_eq!(body.entrypoint, Some(vec!["sh".to_string(), "-c".to_string()]));
        assert_eq!(body.cmd, Some(vec!["npm run migrate".to_string()]));
        assert_eq!(body.labels.unwrap()[MANAGED], "helper");
        let host = body.host_config.unwrap();
        assert_eq!(host.network_mode.as_deref(), Some("jd-proj-1"));
        assert_eq!(host.memory, Some(256 * 1024 * 1024));

        let default = one_off("jd-x:1", None, &[], &s, "jd-proj-1");
        assert_eq!((default.entrypoint, default.cmd), (None, None), "no command keeps the image's own");
    }

    fn service() -> Service {
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
            proxy_network: "jd-proxy",
            tls: false,
        }
    }

    #[test]
    fn public_service_gets_routed_with_its_port_and_limits() {
        let (s, d) = (service(), deployment(None));
        let hosts = vec!["api-shop.example.com".to_string()];
        let body = release(&s, &d, &hosts).body();
        let labels = body.labels.unwrap();
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
        assert_eq!(
            labels[&format!("traefik.http.services.{router}.loadbalancer.server.port")],
            "8080"
        );
        assert!(body.env.unwrap().contains(&"PORT=8080".to_string()));
        let host = body.host_config.unwrap();
        assert_eq!(host.nano_cpus, Some(500_000_000));
        assert_eq!(host.memory, Some(256 * 1024 * 1024));
        let endpoints = body.networking_config.unwrap().endpoints_config.unwrap();
        assert!(endpoints.contains_key("jd-proxy"));
        assert_eq!(
            endpoints["jd-proj-1"].aliases,
            Some(vec!["api".to_string()])
        );
    }

    #[test]
    fn private_service_has_no_route_and_stays_off_the_proxy_network() {
        let (s, d) = (service(), deployment(None));
        let body = release(&s, &d, &[]).body();
        assert!(!body.labels.unwrap().contains_key("traefik.enable"));
        let endpoints = body.networking_config.unwrap().endpoints_config.unwrap();
        assert!(!endpoints.contains_key("jd-proxy"));
    }

    #[test]
    fn previews_route_only_their_own_host_and_get_no_alias() {
        let (s, d) = (service(), deployment(Some(4)));
        let hosts = vec!["pr-4-api-shop.example.com".to_string()];
        let body = release(&s, &d, &hosts).body();
        let labels = body.labels.unwrap();
        assert_eq!(labels[LABEL_PR], "4");
        assert_eq!(
            labels[&format!("traefik.http.routers.{}.rule", names::router(d.id))],
            "Host(`pr-4-api-shop.example.com`)"
        );
        let endpoints = body.networking_config.unwrap().endpoints_config.unwrap();
        assert_eq!(endpoints["jd-proj-1"].aliases, None);
    }

    #[test]
    fn tls_uses_the_secure_entrypoint_and_resolver() {
        let labels = traefik_labels("r", &["a.example.com".into()], 80, "jd-proxy", true);
        assert_eq!(labels["traefik.http.routers.r.entrypoints"], "websecure");
        assert_eq!(labels["traefik.http.routers.r.tls.certresolver"], "le");
    }
}
