mod admin;
mod auth;
mod overview;
mod projects;
mod streams;

use crate::state::SharedState;
use axum::{
    extract::State,
    routing::{delete, get, post, put},
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
        .route("/auth/password", put(admin::change_password))
        .route("/overview", get(overview::overview))
        // projects and services
        .route("/projects", get(projects::list).post(projects::create))
        .route(
            "/projects/{id}",
            get(projects::get).delete(projects::delete),
        )
        .route("/projects/{id}/services", post(projects::create_service))
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
            "/notification-channels",
            get(admin::list_channels).post(admin::create_channel),
        )
        .route(
            "/notification-channels/{id}",
            delete(admin::delete_channel),
        )
        // webhooks authenticate by signature, not session
        .route("/webhooks/github/{id}", post(admin::github))
        .route("/webhooks/gitlab/{id}", post(admin::gitlab));

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
