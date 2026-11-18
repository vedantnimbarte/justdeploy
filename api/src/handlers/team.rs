//! People, invites and the activity log.

use crate::{
    auth::{self, Admin, Can, Owner, Role, Viewer},
    error::{bad_request, Result},
    services,
    state::SharedState,
};
use axum::{
    body::Body,
    extract::{MatchedPath, Path, Query, Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

/// Everyone signed in can see who else is on the team.
pub async fn list_users(
    _: Viewer,
    State(state): State<SharedState>,
) -> Result<Json<Vec<auth::Person>>> {
    Ok(Json(auth::list_users(&state.db).await?))
}

#[derive(Deserialize)]
pub struct UserUpdate {
    role: Option<String>,
    /// Turns their two-factor sign-in off, for a lost phone.
    #[serde(default)]
    reset_two_factor: bool,
}

pub async fn update_user(
    _: Owner,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UserUpdate>,
) -> Result<Json<auth::Person>> {
    auth::get_user(&state.db, id).await?;
    if let Some(role) = &body.role {
        auth::set_role(&state.db, id, Role::parse(role)?).await?;
    }
    if body.reset_two_factor {
        auth::disable_two_factor(&state.db, id).await?;
    }
    Ok(Json(auth::get_user(&state.db, id).await?))
}

pub async fn delete_user(
    Can(me): Owner,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    if me.id == id {
        return Err(bad_request("you can't remove yourself; ask another owner"));
    }
    auth::delete_user(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_invites(
    _: Owner,
    State(state): State<SharedState>,
) -> Result<Json<Vec<auth::Invite>>> {
    Ok(Json(auth::list_invites(&state.db).await?))
}

#[derive(Deserialize)]
pub struct NewInvite {
    email: String,
    role: String,
}

/// Returns the link to share. It is shown once; only its hash is kept.
pub async fn create_invite(
    Can(me): Owner,
    State(state): State<SharedState>,
    Json(body): Json<NewInvite>,
) -> Result<(StatusCode, Json<Value>)> {
    let (invite, token) =
        auth::create_invite(&state.db, &me, &body.email, Role::parse(&body.role)?).await?;
    let link = format!("{}/invite/{token}", state.config.public_url);
    Ok((
        StatusCode::CREATED,
        Json(json!({ "invite": invite, "link": link })),
    ))
}

pub async fn revoke_invite(
    _: Owner,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    auth::revoke_invite(&state.db, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- activity

#[derive(Serialize)]
pub struct Entry {
    id: i64,
    actor: Option<String>,
    action: String,
    target: String,
    detail: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct Page {
    /// Entries older than this id, for the next page.
    before: Option<i64>,
}

pub async fn activity(
    _: Admin,
    State(state): State<SharedState>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<Entry>>> {
    Ok(Json(
        sqlx::query_as!(
            Entry,
            "SELECT id, actor, action, target, detail, created_at FROM audit_log
             WHERE $1::bigint IS NULL OR id < $1
             ORDER BY id DESC LIMIT 100",
            page.before
        )
        .fetch_all(&state.db)
        .await?,
    ))
}

// ---------------------------------------------------------------- recording

/// Middleware: every successful change made through the API goes in the
/// activity log, with who made it and what it was made to.
pub async fn record(
    State(state): State<SharedState>,
    matched: Option<MatchedPath>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let route = matched
        .as_ref()
        .map(|m| m.as_str().trim_start_matches("/api/v1").to_string())
        .unwrap_or_default();
    let Some(phrase) = describe(method.as_str(), &route) else {
        return next.run(request).await;
    };
    let actor = auth::current_user(&state, request.headers())
        .await
        .ok()
        .flatten()
        .map(|u| u.email);
    // Looked up before the change, while what it names still exists.
    let target = target_name(&state, request.uri().path()).await;

    // Service actions say what they did in the body; nothing else is read from it.
    let (request, action) = if route == "/services/{id}/actions" {
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, 64 * 1024)
            .await
            .unwrap_or_default();
        let action = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v["action"].as_str().map(str::to_string));
        (Request::from_parts(parts, Body::from(bytes)), action)
    } else {
        (request, None)
    };
    let phrase = match action.as_deref() {
        Some("deploy") => "deployed",
        Some("restart") => "restarted",
        Some("stop") => "stopped",
        Some("start") => "started",
        _ => phrase,
    };

    let response = next.run(request).await;
    if response.status().is_success() {
        services::audit(&state.db, actor.as_deref(), phrase, &target, None).await;
    }
    response
}

/// The log's wording for each change. Routes not listed aren't recorded.
fn describe(method: &str, route: &str) -> Option<&'static str> {
    Some(match (method, route) {
        ("PUT", "/auth/password") => "changed their password",
        ("POST", "/auth/two-factor/confirm") => "turned on two-factor sign-in",
        ("DELETE", "/auth/two-factor") => "turned off two-factor sign-in",
        ("POST", "/auth/tokens") => "created an API token",
        ("DELETE", "/auth/tokens/{id}") => "deleted an API token",
        ("POST", "/projects") => "created a project",
        ("DELETE", "/projects/{id}") => "deleted",
        ("PUT", "/projects/{id}/env") => "changed shared variables in",
        ("POST", "/projects/{id}/services") => "added a service to",
        ("POST", "/projects/{id}/environments") => "added an environment to",
        ("POST", "/projects/{id}/sync") => "copied settings into",
        ("PUT", "/projects/{id}/status-page") => "changed the status page of",
        ("PUT", "/services/{id}/pitr") => "changed point-in-time recovery of",
        ("POST", "/services/{id}/pitr/restore") => "restored to a point in time:",
        ("PATCH", "/services/{id}") => "changed settings of",
        ("DELETE", "/services/{id}") => "deleted",
        ("POST", "/services/{id}/actions") => "acted on",
        ("PUT", "/services/{id}/env") => "changed variables of",
        ("POST", "/services/{id}/domains") => "added a domain to",
        ("DELETE", "/services/{id}/domains/{domain_id}") => "removed a domain from",
        ("POST", "/services/{id}/runs") => "ran a command on",
        ("POST", "/services/{id}/backups") => "backed up",
        ("PUT", "/services/{id}/backup-schedule") => "changed the backup schedule of",
        ("POST", "/deployments/{id}/rollback") => "rolled back",
        ("POST", "/backups/{id}/restore") => "restored a backup of",
        ("POST", "/backup-destinations") => "added a backup destination",
        ("DELETE", "/backup-destinations/{id}") => "removed backup destination",
        ("POST", "/templates/{id}/deploy") => "deployed the template",
        ("PUT", "/settings") => "changed the base domain",
        ("POST", "/notification-channels") => "added a notification channel",
        ("DELETE", "/notification-channels/{id}") => "removed notification channel",
        ("PATCH", "/users/{id}") => "changed the access of",
        ("DELETE", "/users/{id}") => "removed",
        ("POST", "/invites") => "sent an invite",
        ("DELETE", "/invites/{id}") => "revoked the invite for",
        ("POST", "/github/app") => "connected a GitHub App",
        ("PATCH", "/servers/{id}") => "changed server",
        ("DELETE", "/servers/{id}") => "removed server",
        ("DELETE", "/github/app") => "disconnected the GitHub App",
        _ => return None,
    })
}

/// What a path names, in words: `shop/web` for a service, an email for a person.
async fn target_name(state: &SharedState, path: &str) -> String {
    // Inside the API router the path has no /api/v1 prefix; outside it does.
    let mut parts = path
        .trim_start_matches("/api/v1")
        .trim_start_matches('/')
        .split('/');
    let kind = parts.next().unwrap_or_default();
    let id = parts.next().unwrap_or_default();
    let Ok(uuid) = id.parse::<Uuid>() else {
        return if matches!(kind, "templates" | "servers") {
            id.to_string()
        } else {
            String::new()
        };
    };
    let db = &state.db;
    let found = match kind {
        "projects" => {
            sqlx::query_scalar!("SELECT name FROM projects WHERE id = $1", uuid)
                .fetch_optional(db)
                .await
        }
        "services" => {
            sqlx::query_scalar!(
                r#"SELECT p.name || '/' || s.name AS "n!"
                   FROM services s JOIN projects p ON p.id = s.project_id WHERE s.id = $1"#,
                uuid
            )
            .fetch_optional(db)
            .await
        }
        "deployments" | "backups" => {
            sqlx::query_scalar!(
                r#"SELECT p.name || '/' || s.name AS "n!"
                   FROM services s JOIN projects p ON p.id = s.project_id
                   WHERE s.id IN (SELECT service_id FROM deployments WHERE id = $1
                                  UNION SELECT service_id FROM backups WHERE id = $1)"#,
                uuid
            )
            .fetch_optional(db)
            .await
        }
        "users" => {
            sqlx::query_scalar!("SELECT email FROM users WHERE id = $1", uuid)
                .fetch_optional(db)
                .await
        }
        "invites" => {
            sqlx::query_scalar!("SELECT email FROM invites WHERE id = $1", uuid)
                .fetch_optional(db)
                .await
        }
        "backup-destinations" => {
            sqlx::query_scalar!("SELECT name FROM backup_destinations WHERE id = $1", uuid)
                .fetch_optional(db)
                .await
        }
        "notification-channels" => {
            sqlx::query_scalar!("SELECT name FROM notification_channels WHERE id = $1", uuid)
                .fetch_optional(db)
                .await
        }
        _ => Ok(None),
    };
    found.ok().flatten().unwrap_or_default()
}
