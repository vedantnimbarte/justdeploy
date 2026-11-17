mod auth;

use crate::state::SharedState;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

pub fn router(state: SharedState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/auth/status", get(auth::status))
        .route("/auth/setup", post(auth::setup))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me));

    Router::new().nest("/api/v1", api).with_state(state)
}

async fn health(State(state): State<SharedState>) -> Json<Value> {
    let database = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();
    let docker = state.docker.ping().await.is_ok();
    Json(json!({ "ok": database && docker, "database": database, "docker": docker }))
}
