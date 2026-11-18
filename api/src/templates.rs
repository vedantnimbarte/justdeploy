//! One-click templates: a project with its services wired together.
//!
//! Env values may use placeholders, resolved when the template is deployed:
//! `${random}` is a fresh 32-character secret, `${db.url}` is the connection URL
//! of the template's service named `db`, and `${db.KEY}` is one of its variables.

use crate::{
    crypto::random_string,
    error::{bad_request, Result},
    models::Project,
    services::{self, NewService},
    state::SharedState,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

const CATALOG: &str = include_str!("../../templates/catalog.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    /// frontend | backend | fullstack | selfhosted | database
    pub category: String,
    pub description: String,
    pub services: Vec<TemplateService>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateService {
    pub name: String,
    pub kind: String,
    /// Folder in the templates repository (code templates).
    #[serde(default)]
    pub root_dir: String,
    pub build_type: Option<String>,
    pub image: Option<String>,
    pub engine: Option<String>,
    pub port: Option<i32>,
    #[serde(default)]
    pub public: bool,
    pub health_path: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub volumes: Vec<String>,
}

pub fn catalog() -> Vec<Template> {
    serde_json::from_str(CATALOG).expect("templates/catalog.json is checked by a unit test")
}

/// Replaces `${...}` placeholders using services created earlier in the template.
fn resolve(
    value: &str,
    created: &HashMap<String, (Option<String>, HashMap<String, String>)>,
) -> Result<String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| bad_request(format!("unclosed placeholder in {value}")))?;
        let key = &rest[start + 2..start + end];
        let replacement = match key.split_once('.') {
            None if key == "random" => random_string(32),
            Some((service, "url")) => created
                .get(service)
                .and_then(|(url, _)| url.clone())
                .ok_or_else(|| bad_request(format!("${{{key}}}: no database named {service}")))?,
            Some((service, var)) => created
                .get(service)
                .and_then(|(_, env)| env.get(var).cloned())
                .ok_or_else(|| bad_request(format!("${{{key}}}: {service} has no {var}")))?,
            None => return Err(bad_request(format!("unknown placeholder ${{{key}}}"))),
        };
        out.push_str(&replacement);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[derive(Deserialize)]
pub struct DeployTemplate {
    /// A new project with this name...
    pub project_name: Option<String>,
    /// ...or an existing project.
    pub project_id: Option<Uuid>,
}

pub async fn deploy(state: &SharedState, id: &str, target: DeployTemplate) -> Result<Project> {
    let template = catalog()
        .into_iter()
        .find(|t| t.id == id)
        .ok_or(crate::error::Error::NotFound)?;
    let project = match (target.project_id, target.project_name) {
        (Some(project_id), _) => services::get_project(&state.db, project_id).await?,
        (None, Some(name)) if !name.trim().is_empty() => {
            services::create_project(&state.db, name.trim()).await?
        }
        _ => return Err(bad_request("choose a project or name a new one")),
    };

    let mut created = HashMap::new();
    for spec in &template.services {
        let mut env = BTreeMap::new();
        for (key, value) in &spec.env {
            env.insert(key.clone(), resolve(value, &created)?);
        }
        let code = spec.kind == "app";
        let service = services::create_service(
            state,
            project.id,
            NewService {
                name: spec.name.clone(),
                kind: spec.kind.clone(),
                repo_url: code.then(|| state.config.templates_repo.clone()),
                branch: code.then(|| state.config.templates_branch.clone()),
                root_dir: spec.root_dir.clone(),
                git_token: None,
                build_type: spec.build_type.clone(),
                dockerfile_path: None,
                image: spec.image.clone(),
                engine: spec.engine.clone(),
                port: spec.port,
                public: spec.public,
                health_path: spec.health_path.clone(),
                cpu_millis: None,
                memory_mb: None,
                env,
                volumes: spec.volumes.clone(),
                pre_deploy_command: None,
                cron_schedule: None,
            },
        )
        .await?;
        let vars: HashMap<String, String> = services::env(state, service.id)
            .await?
            .into_iter()
            .collect();
        let url = services::connection_url(state, &service).await?;
        created.insert(spec.name.clone(), (url, vars));
    }
    services::audit(
        &state.db,
        "template.deployed",
        &template.id,
        Some(&project.name),
    )
    .await;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{engines, names};

    #[test]
    fn catalog_parses_and_is_consistent() {
        let templates = catalog();
        assert!(templates.len() >= 15);
        let mut ids = std::collections::HashSet::new();
        for t in &templates {
            assert!(ids.insert(t.id.clone()), "duplicate template id {}", t.id);
            assert!(
                ["frontend", "backend", "fullstack", "selfhosted", "database"]
                    .contains(&t.category.as_str()),
                "{} has category {}",
                t.id,
                t.category
            );
            let mut seen = Vec::new();
            for s in &t.services {
                names::validate(&s.name, "service").unwrap();
                match s.kind.as_str() {
                    "app" => assert!(!s.root_dir.is_empty(), "{}: app without root_dir", t.id),
                    "image" => assert!(s.image.is_some(), "{}: image service without image", t.id),
                    "database" => {
                        assert!(engines::get(s.engine.as_deref().unwrap_or("")).is_some())
                    }
                    other => panic!("{}: unknown kind {other}", t.id),
                }
                if s.public {
                    assert!(s.port.is_some(), "{}: public without port", t.id);
                }
                // Placeholders may only point at services declared earlier.
                for value in s.env.values() {
                    for (service, _) in value.match_indices("${").filter_map(|(i, _)| {
                        value[i + 2..]
                            .split_once('}')
                            .and_then(|(k, _)| k.split_once('.'))
                    }) {
                        assert!(
                            seen.contains(&service.to_string()),
                            "{}: {value} refers forward",
                            t.id
                        );
                    }
                }
                seen.push(s.name.clone());
            }
        }
    }

    #[test]
    fn placeholders_resolve() {
        let mut created = HashMap::new();
        created.insert(
            "db".to_string(),
            (
                Some("postgresql://app:pw@db:5432/app".to_string()),
                HashMap::from([("POSTGRES_USER".to_string(), "app".to_string())]),
            ),
        );
        assert_eq!(
            resolve("${db.url}", &created).unwrap(),
            "postgresql://app:pw@db:5432/app"
        );
        assert_eq!(
            resolve("user=${db.POSTGRES_USER}!", &created).unwrap(),
            "user=app!"
        );
        assert_eq!(resolve("${random}", &created).unwrap().len(), 32);
        assert!(resolve("${cache.url}", &created).is_err());
        assert!(resolve("${oops", &created).is_err());
    }
}
