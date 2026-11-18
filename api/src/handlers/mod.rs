pub mod admin;
mod auth;
mod overview;
mod projects;
mod streams;
mod team;

use crate::state::SharedState;
use axum::{
    extract::State,
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::services::{ServeDir, ServeFile};

pub fn router(state: SharedState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        // auth
        .route("/auth/status", get(auth::status))
        .route("/auth/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/password", put(auth::change_password))
        .route(
            "/auth/two-factor",
            post(auth::start_two_factor).delete(auth::disable_two_factor),
        )
        .route("/auth/two-factor/confirm", post(auth::confirm_two_factor))
        .route(
            "/auth/two-factor/recovery-codes",
            post(auth::new_recovery_codes),
        )
        .route(
            "/auth/tokens",
            get(auth::list_tokens).post(auth::create_token),
        )
        .route("/auth/tokens/{id}", delete(auth::delete_token))
        .route(
            "/auth/invites/{token}",
            get(auth::invite_info).post(auth::accept_invite),
        )
        // people
        .route("/users", get(team::list_users))
        .route(
            "/users/{id}",
            patch(team::update_user).delete(team::delete_user),
        )
        .route(
            "/invites",
            get(team::list_invites).post(team::create_invite),
        )
        .route("/invites/{id}", delete(team::revoke_invite))
        .route("/activity", get(team::activity))
        // servers
        .route("/servers", get(admin::list_servers))
        .route("/servers/join", get(admin::join_server))
        .route(
            "/servers/{id}",
            patch(admin::update_server).delete(admin::remove_server),
        )
        .route("/cluster/resilience", get(admin::resilience))
        .route("/cluster/standby", put(admin::set_standby))
        // GitHub App
        .route("/github", get(admin::github_overview))
        .route("/github/manifest", post(admin::github_manifest))
        .route(
            "/github/app",
            post(admin::github_create).delete(admin::github_forget),
        )
        .route("/github/repos", get(admin::github_repos))
        .route("/overview", get(overview::overview))
        // projects and services
        .route("/projects", get(projects::list).post(projects::create))
        .route(
            "/projects/{id}",
            get(projects::get).delete(projects::delete),
        )
        .route("/projects/{id}/services", post(projects::create_service))
        .route(
            "/projects/{id}/env",
            get(projects::get_shared_env).put(projects::put_shared_env),
        )
        .route(
            "/projects/{id}/environments",
            post(projects::create_environment),
        )
        .route("/projects/{id}/sync", post(projects::sync_environment))
        .route("/projects/{id}/graph", get(projects::graph))
        .route("/projects/{id}/usage", get(projects::usage))
        .route("/projects/{id}/uptime", get(projects::uptime))
        .route(
            "/projects/{id}/status-page",
            put(projects::configure_status_page),
        )
        .route("/status/{name}", get(projects::public_status))
        .route("/services/{id}/logs/search", get(projects::search_logs))
        .route(
            "/services/{id}/pitr",
            get(projects::pitr_status).put(projects::configure_pitr),
        )
        .route("/services/{id}/pitr/restore", post(projects::pitr_restore))
        .route("/services/{id}/copy", post(projects::copy_database))
        .route(
            "/services/{id}/copies",
            get(projects::list_copies).post(projects::replace_data),
        )
        .route(
            "/services/{id}/replicas",
            get(projects::list_replicas).post(projects::create_replica),
        )
        .route(
            "/services/{id}",
            get(projects::get_service)
                .patch(projects::update_service)
                .delete(projects::delete_service),
        )
        .route("/services/{id}/actions", post(projects::service_action))
        .route("/services/{id}/deployments", get(projects::deployments))
        .route(
            "/services/{id}/env",
            get(projects::get_env).put(projects::put_env),
        )
        .route("/services/{id}/domains", post(projects::add_domain))
        .route(
            "/services/{id}/domains/{domain_id}",
            delete(projects::remove_domain),
        )
        .route(
            "/services/{id}/runs",
            get(projects::list_runs).post(projects::start_run),
        )
        .route("/services/{id}/logs", get(streams::runtime_logs))
        .route("/services/{id}/metrics", get(streams::metrics))
        .route("/services/{id}/terminal", get(streams::terminal))
        .route(
            "/services/{id}/backups",
            get(admin::list_backups).post(admin::backup_now),
        )
        .route(
            "/services/{id}/backup-schedule",
            get(admin::get_schedule).put(admin::put_schedule),
        )
        .route("/deployments/{id}", get(projects::deployment))
        .route("/deployments/{id}/logs", get(streams::deployment_logs))
        .route("/deployments/{id}/rollback", post(projects::rollback))
        // backups
        .route(
            "/backup-destinations",
            get(admin::list_destinations).post(admin::create_destination),
        )
        .route(
            "/backup-destinations/{id}",
            delete(admin::delete_destination),
        )
        .route("/backups/{id}/restore", post(admin::restore))
        // templates and settings
        .route("/templates", get(admin::list_templates))
        .route("/templates/{id}/deploy", post(admin::deploy_template))
        .route(
            "/settings",
            get(admin::get_settings).put(admin::put_settings),
        )
        .route("/dns-check", get(admin::dns_check))
        .route(
            "/settings/smtp",
            get(admin::get_smtp)
                .put(admin::put_smtp)
                .delete(admin::delete_smtp),
        )
        .route(
            "/notification-channels",
            get(admin::list_channels).post(admin::create_channel),
        )
        .route("/notification-channels/{id}", delete(admin::delete_channel))
        // webhooks authenticate by signature, not session
        .route("/webhooks/github/{id}", post(admin::github))
        .route("/webhooks/gitlab/{id}", post(admin::gitlab))
        .route("/webhooks/github-app", post(admin::github_app_webhook))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            team::record,
        ));

    let mut app = Router::new().nest("/api/v1", api);
    // In production the dashboard is served from here; unknown paths fall back to
    // index.html so client-side routes survive a reload.
    if let Some(dir) = &state.config.web_dir {
        let index = dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
    }
    app.with_state(state)
}

async fn health(State(state): State<SharedState>) -> Json<Value> {
    let database = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let docker = state.docker.ping().await.is_ok();
    Json(json!({ "ok": database && docker, "database": database, "docker": docker }))
}
