use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

async fn index() -> Json<Value> {
    Json(json!({ "service": "rust-axum" }))
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health));
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("bind");
    println!("listening on {port}");
    axum::serve(listener, app).await.expect("serve");
}
