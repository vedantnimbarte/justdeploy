//! The GitHub App. Created in two clicks from a manifest, it replaces personal
//! tokens and per-service webhooks: it clones private repositories, delivers
//! pushes and pull requests for every repository it is installed on, and gets
//! a commit status for each deploy and preview.

use crate::{
    crypto::random_string,
    error::{bad_request, Error, Result},
    models::{Deployment, Service},
    names, services,
    state::SharedState,
    webhooks,
};
use anyhow::{anyhow, Context};
use axum::http::HeaderMap;
use reqwest::Method;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{sync::OnceLock, time::Duration};

const API: &str = "https://api.github.com";
const STATE_KEY: &str = "github_manifest_state";

struct App {
    app_id: i64,
    slug: String,
    html_url: String,
    private_key: String,
    webhook_secret: String,
}

async fn load(state: &SharedState) -> Result<Option<App>> {
    let row = sqlx::query!(
        "SELECT app_id, slug, html_url, private_key_enc, webhook_secret_enc FROM github_app"
    )
    .fetch_optional(&state.db)
    .await?;
    row.map(|r| {
        Ok(App {
            app_id: r.app_id,
            slug: r.slug,
            html_url: r.html_url,
            private_key: state.cipher.decrypt(&r.private_key_enc)?,
            webhook_secret: state.cipher.decrypt(&r.webhook_secret_enc)?,
        })
    })
    .transpose()
}

// ---------------------------------------------------------------- talking to GitHub

/// The app's own identity: a ten-minute RS256 token signed with its private key.
fn app_jwt(app: &App) -> anyhow::Result<String> {
    #[derive(Serialize)]
    struct Claims {
        iat: i64,
        exp: i64,
        iss: String,
    }
    let now = chrono::Utc::now().timestamp();
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(app.private_key.as_bytes())
        .context("the stored GitHub App key is not a valid RSA key")?;
    Ok(jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        // Backdated a minute for clock drift; GitHub refuses more than ten minutes.
        &Claims {
            iat: now - 60,
            exp: now + 9 * 60,
            iss: app.app_id.to_string(),
        },
        &key,
    )?)
}

async fn call<T: DeserializeOwned>(
    method: Method,
    path: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> anyhow::Result<T> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("justdeploy")
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_default()
    });
    let mut request = client
        .request(method, format!("{API}{path}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let detail: Value = response.json().await.unwrap_or_default();
        let message = detail["message"].as_str().unwrap_or("no details");
        return Err(anyhow!("GitHub answered {status}: {message}"));
    }
    Ok(response.json().await?)
}

async fn installation_token(app: &App, installation_id: i64) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    struct Token {
        token: String,
    }
    let token: Token = call(
        Method::POST,
        &format!("/app/installations/{installation_id}/access_tokens"),
        Some(&app_jwt(app)?),
        None,
    )
    .await?;
    Ok(token.token)
}

/// `owner/repo`, lowercased, for a GitHub clone URL.
pub fn repo_path(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .strip_prefix("https://github.com/")
        .or_else(|| url.trim().strip_prefix("http://github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = rest.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .then(|| format!("{owner}/{repo}").to_lowercase())
}

/// A short-lived token for a repository, when the app is installed on its
/// owner. Builds clone with it when the service has no token of its own.
pub async fn repo_token(state: &SharedState, repo_url: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Installation {
        id: i64,
    }
    let app = load(state).await.ok().flatten()?;
    let path = repo_path(repo_url)?;
    let result = async {
        let installation: Installation = call(
            Method::GET,
            &format!("/repos/{path}/installation"),
            Some(&app_jwt(&app)?),
            None,
        )
        .await?;
        installation_token(&app, installation.id).await
    }
    .await;
    match result {
        Ok(token) => Some(token),
        Err(error) => {
            tracing::debug!(%path, %error, "the GitHub App can't reach this repository");
            None
        }
    }
}

// ---------------------------------------------------------------- creating the app

#[derive(Serialize)]
pub struct ManifestForm {
    /// Where the browser posts `manifest` to.
    action_url: String,
    manifest: Value,
}

/// The form that creates the app on GitHub, for a personal account or an organization.
pub async fn manifest(state: &SharedState, org: Option<&str>) -> Result<ManifestForm> {
    let base = &state.config.public_url;
    if base.contains("://localhost") || base.contains("://127.") {
        return Err(bad_request(
            "GitHub has to reach this server: set JD_PUBLIC_URL to its public address first",
        ));
    }
    let nonce = random_string(24);
    sqlx::query!(
        "INSERT INTO settings (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        STATE_KEY,
        nonce
    )
    .execute(&state.db)
    .await?;
    let action_url = match org.map(str::trim).filter(|o| !o.is_empty()) {
        Some(org) => {
            format!("https://github.com/organizations/{org}/settings/apps/new?state={nonce}")
        }
        None => format!("https://github.com/settings/apps/new?state={nonce}"),
    };
    let manifest = json!({
        // App names are unique across GitHub.
        "name": format!("justdeploy-{}", random_string(6).to_lowercase()),
        "url": base,
        "hook_attributes": { "url": format!("{base}/api/v1/webhooks/github-app") },
        "redirect_url": format!("{base}/github/created"),
        "setup_url": format!("{base}/github/installed"),
        "setup_on_update": true,
        "public": false,
        "default_permissions": {
            "contents": "read",
            "metadata": "read",
            "pull_requests": "read",
            "statuses": "write"
        },
        "default_events": ["push", "pull_request"]
    });
    Ok(ManifestForm {
        action_url,
        manifest,
    })
}

/// GitHub sends the browser back with a code; trading it in gets the app's keys.
pub async fn finish_creation(state: &SharedState, code: &str, nonce: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct Created {
        id: i64,
        slug: String,
        html_url: String,
        pem: String,
        webhook_secret: String,
    }
    let expected = sqlx::query_scalar!("SELECT value FROM settings WHERE key = $1", STATE_KEY)
        .fetch_optional(&state.db)
        .await?;
    if expected.as_deref() != Some(nonce) {
        return Err(bad_request(
            "this link doesn't match the app being created; start again from Settings",
        ));
    }
    let created: Created = call(
        Method::POST,
        &format!("/app-manifests/{code}/conversions"),
        None,
        None,
    )
    .await
    .map_err(|e| bad_request(format!("{e:#}")))?;
    sqlx::query!(
        "INSERT INTO github_app (id, app_id, slug, html_url, private_key_enc, webhook_secret_enc)
         VALUES (1, $1, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET app_id = $1, slug = $2, html_url = $3,
             private_key_enc = $4, webhook_secret_enc = $5, created_at = now()",
        created.id,
        created.slug,
        created.html_url,
        state.cipher.encrypt(&created.pem),
        state.cipher.encrypt(&created.webhook_secret)
    )
    .execute(&state.db)
    .await?;
    sqlx::query!("DELETE FROM settings WHERE key = $1", STATE_KEY)
        .execute(&state.db)
        .await?;
    Ok(())
}

/// Forgets the app here. It still exists on GitHub until deleted there.
pub async fn forget(state: &SharedState) -> Result<()> {
    sqlx::query!("DELETE FROM github_app")
        .execute(&state.db)
        .await?;
    Ok(())
}

#[derive(Serialize)]
pub struct Overview {
    app: Option<AppInfo>,
    installations: Vec<InstallationInfo>,
    /// Why installations couldn't be listed, if they couldn't.
    error: Option<String>,
}

#[derive(Serialize)]
struct AppInfo {
    slug: String,
    html_url: String,
    install_url: String,
}

#[derive(Serialize, Deserialize)]
struct InstallationInfo {
    id: i64,
    account: String,
}

async fn installations(app: &App) -> anyhow::Result<Vec<InstallationInfo>> {
    #[derive(Deserialize)]
    struct Raw {
        id: i64,
        account: Option<Account>,
    }
    #[derive(Deserialize)]
    struct Account {
        login: String,
    }
    let raw: Vec<Raw> = call(
        Method::GET,
        "/app/installations?per_page=100",
        Some(&app_jwt(app)?),
        None,
    )
    .await?;
    Ok(raw
        .into_iter()
        .map(|r| InstallationInfo {
            id: r.id,
            account: r.account.map(|a| a.login).unwrap_or_default(),
        })
        .collect())
}

pub async fn overview(state: &SharedState) -> Result<Overview> {
    let Some(app) = load(state).await? else {
        return Ok(Overview {
            app: None,
            installations: vec![],
            error: None,
        });
    };
    let (installations, error) = match installations(&app).await {
        Ok(list) => (list, None),
        Err(e) => (vec![], Some(format!("{e:#}"))),
    };
    Ok(Overview {
        app: Some(AppInfo {
            install_url: format!("{}/installations/new", app.html_url),
            slug: app.slug,
            html_url: app.html_url,
        }),
        installations,
        error,
    })
}

#[derive(Serialize)]
pub struct Repo {
    full_name: String,
    clone_url: String,
    default_branch: String,
    private: bool,
}

/// Every repository the app can see, across its installations.
pub async fn repos(state: &SharedState) -> Result<Vec<Repo>> {
    #[derive(Deserialize)]
    struct Page {
        repositories: Vec<RawRepo>,
    }
    #[derive(Deserialize)]
    struct RawRepo {
        full_name: String,
        clone_url: String,
        default_branch: Option<String>,
        private: bool,
    }
    let app = load(state)
        .await?
        .ok_or_else(|| bad_request("connect a GitHub App in Settings first"))?;
    let mut all = Vec::new();
    for installation in installations(&app).await.map_err(Error::Internal)? {
        let token = installation_token(&app, installation.id)
            .await
            .map_err(Error::Internal)?;
        // ponytail: 1000 repositories per installation; add a search box if someone has more.
        for page in 1..=10 {
            let batch: Page = call(
                Method::GET,
                &format!("/installation/repositories?per_page=100&page={page}"),
                Some(&token),
                None,
            )
            .await
            .map_err(Error::Internal)?;
            let done = batch.repositories.len() < 100;
            all.extend(batch.repositories.into_iter().map(|r| Repo {
                full_name: r.full_name,
                clone_url: r.clone_url,
                default_branch: r.default_branch.unwrap_or_else(|| "main".into()),
                private: r.private,
            }));
            if done {
                break;
            }
        }
    }
    all.sort_by_key(|a| a.full_name.to_lowercase());
    Ok(all)
}

// ---------------------------------------------------------------- webhooks

/// One webhook for every repository the app is on: pushes deploy, pull requests
/// build previews, for each service built from that repository.
pub async fn webhook(state: &SharedState, headers: &HeaderMap, body: &[u8]) -> Result<Value> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let app = load(state).await?.ok_or(Error::NotFound)?;
    if !webhooks::verify_github(&app.webhook_secret, &header("x-hub-signature-256"), body) {
        return Err(Error::Forbidden(
            "signature does not match the GitHub App's webhook secret".into(),
        ));
    }
    let event = header("x-github-event");
    let payload: Value = serde_json::from_slice(body)
        .map_err(|e| bad_request(format!("unexpected payload: {e}")))?;
    let repo = payload["repository"]["full_name"]
        .as_str()
        .unwrap_or_default()
        .to_lowercase();
    if !matches!(event.as_str(), "push" | "pull_request") || repo.is_empty() {
        return Ok(json!({ "message": format!("ignored: {event} events are not used") }));
    }

    let services: Vec<Service> =
        sqlx::query_as!(Service, "SELECT * FROM services WHERE kind = 'app'")
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .filter(|s| s.repo_url.as_deref().and_then(repo_path).as_deref() == Some(repo.as_str()))
            .collect();
    let mut messages = Vec::new();
    for service in &services {
        let message = match event.as_str() {
            "push" => {
                let push = serde_json::from_value(payload.clone())
                    .map_err(|e| bad_request(format!("unexpected payload: {e}")))?;
                webhooks::on_push(state, service, push).await?
            }
            _ => {
                let pr = serde_json::from_value(payload.clone())
                    .map_err(|e| bad_request(format!("unexpected payload: {e}")))?;
                webhooks::on_pull_request(state, service, pr).await?
            }
        };
        messages.push(format!("{}: {message}", service.name));
    }
    if messages.is_empty() {
        messages.push(format!("ignored: no service is built from {repo}"));
    }
    Ok(json!({ "message": messages.join("; ") }))
}

// ---------------------------------------------------------------- commit statuses

/// Marks the deployment's commit on GitHub: pending while it deploys, then
/// success or failure, linking to the preview or the service. In the
/// background; a repository the app isn't on is skipped quietly.
pub fn report(
    state: &SharedState,
    service: &Service,
    deployment: &Deployment,
    outcome: &'static str,
    description: String,
) {
    let (Some(sha), Some(repo_url)) = (deployment.commit_sha.clone(), service.repo_url.clone())
    else {
        return;
    };
    let (state, service, pr) = (state.clone(), service.clone(), deployment.pr_number);
    tokio::spawn(async move {
        let Some(path) = repo_path(&repo_url) else {
            return;
        };
        let Some(token) = repo_token(&state, &repo_url).await else {
            return;
        };
        let target_url = match (pr, service.public && service.port.is_some()) {
            (Some(pr), true) => {
                async {
                    let project = services::get_project(&state.db, service.project_id)
                        .await
                        .ok()?;
                    let base = services::base_domain(&state).await.ok()?;
                    let scheme = if state.config.tls() { "https" } else { "http" };
                    Some(format!(
                        "{scheme}://{}",
                        names::preview_host(pr, &service.name, &project.name, &base)
                    ))
                }
                .await
            }
            _ => None,
        }
        .unwrap_or_else(|| format!("{}/services/{}", state.config.public_url, service.id));
        let context = match pr {
            Some(_) => format!("justdeploy/{} preview", service.name),
            None => format!("justdeploy/{}", service.name),
        };
        let body = json!({
            "state": outcome,
            "target_url": target_url,
            // GitHub cuts descriptions at 140 characters.
            "description": description.chars().take(140).collect::<String>(),
            "context": context,
        });
        if let Err(error) = call::<Value>(
            Method::POST,
            &format!("/repos/{path}/statuses/{sha}"),
            Some(&token),
            Some(body),
        )
        .await
        {
            tracing::warn!(%path, %error, "commit status not sent");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_paths_normalize() {
        assert_eq!(
            repo_path("https://github.com/Acme/Shop.git").as_deref(),
            Some("acme/shop")
        );
        assert_eq!(
            repo_path("https://github.com/acme/shop/").as_deref(),
            Some("acme/shop")
        );
        assert_eq!(repo_path("https://gitlab.com/acme/shop"), None);
        assert_eq!(repo_path("https://github.com/acme"), None);
        assert_eq!(repo_path("https://github.com/acme/shop/tree/main"), None);
    }
}
