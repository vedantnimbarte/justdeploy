#![cfg_attr(test, allow(clippy::unwrap_used))]

mod auth;
mod backups;
mod config;
mod crypto;
mod docker;
mod engines;
mod error;
mod handlers;
mod host;
mod metrics;
mod models;
mod names;
mod queue;
mod services;
mod state;
mod templates;
mod webhooks;

use anyhow::Context;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("justdeploy=info,tower_http=info")),
        )
        .init();

    let config = config::Config::from_env()?;
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .context("cannot connect to Postgres (DATABASE_URL)")?;
    sqlx::migrate!()
        .run(&db)
        .await
        .context("migrations failed")?;

    if std::env::args().nth(1).as_deref() == Some("reset-password") {
        return reset_password(&db).await;
    }

    let docker =
        bollard::Docker::connect_with_local_defaults().context("cannot reach the Docker daemon")?;
    docker
        .ping()
        .await
        .context("Docker is not answering; is /var/run/docker.sock mounted?")?;

    let state = Arc::new(state::AppState {
        db,
        docker,
        cipher: crypto::Cipher::new(&config.master_key),
        login_limiter: auth::RateLimiter::default(),
        config,
    });

    queue::maintenance::remove_stale_helpers(&state).await;
    queue::release_locks(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    queue::spawn(state.clone());

    let app = handlers::router(state.clone()).layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(state.config.bind).await?;
    tracing::info!("justdeploy listening on {}", state.config.bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}

/// `justdeploy reset-password`: prints a new random password and signs out every session.
async fn reset_password(db: &sqlx::PgPool) -> anyhow::Result<()> {
    let password = crypto::random_string(20);
    auth::reset_password(db, &password)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("New admin password: {password}");
    println!("Every session was signed out. Change it under Settings after signing in.");
    Ok(())
}
