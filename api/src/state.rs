use crate::{auth::RateLimiter, config::Config, crypto::Cipher};
use bollard::Docker;
use sqlx::PgPool;
use std::sync::Arc;

pub struct AppState {
    pub db: PgPool,
    pub docker: Docker,
    pub config: Config,
    pub cipher: Cipher,
    pub login_limiter: RateLimiter,
}

pub type SharedState = Arc<AppState>;
