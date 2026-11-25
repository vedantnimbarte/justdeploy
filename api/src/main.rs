#![cfg_attr(test, allow(clippy::unwrap_used))]

mod auth;
mod backups;
mod cluster;
mod config;
mod crypto;
mod data;
mod docker;
mod engines;
mod error;
mod github;
mod ha;
mod handlers;
mod host;
mod logs;
mod metrics;
mod models;
mod names;
mod notify;
mod pitr;
mod queue;
mod runs;
mod services;
mod state;
mod templates;
mod traces;
mod uptime;
mod usage;
mod webhooks;

use anyhow::Context;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    // A promoted control plane gets its settings as a file (a Swarm secret);
    // what its own environment says wins.
    if let Ok(path) = std::env::var("JD_ENV_FILE") {
        dotenvy::from_path(path).context("reading JD_ENV_FILE")?;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("justdeploy=info,tower_http=info")),
        )
        .init();

    if std::env::args().nth(1).as_deref() == Some("promote") {
        return ha::promote().await;
    }

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
        return reset_password(&db, std::env::args().nth(2).as_deref()).await;
    }

    let docker =
        bollard::Docker::connect_with_defaults().context("cannot reach the Docker daemon")?;
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
    cluster::ensure(&state).await?;
    runs::fail_interrupted(&state)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    queue::release_locks(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    queue::spawn(state.clone());

    let otlp = tokio::net::TcpListener::bind(("0.0.0.0", traces::PORT)).await?;
    tracing::info!("receiving traces on port {}", traces::PORT);
    let receiver = traces::router(state.clone());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(otlp, receiver).await {
            tracing::error!(%error, "trace receiver stopped");
        }
    });

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

/// `justdeploy reset-password [email]`: a new random password for that account
/// (the first owner by default), with two-factor sign-in turned off and its
/// sessions signed out.
async fn reset_password(db: &sqlx::PgPool, email: Option<&str>) -> anyhow::Result<()> {
    let password = crypto::random_string(20);
    let email = auth::reset_password(db, email, &password)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("New password for {email}: {password}");
    println!("Two-factor sign-in is off and every session of theirs was signed out.");
    println!("Change it under Account after signing in.");
    Ok(())
}
