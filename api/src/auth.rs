//! The single admin: setup, login, sessions.

use crate::{
    error::{bad_request, Error, Result},
    state::SharedState,
};
use anyhow::anyhow;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{extract::FromRequestParts, http::request::Parts};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const COOKIE_NAME: &str = "jd_session";
const SESSION_DAYS: i64 = 30;
const MIN_PASSWORD: usize = 10;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| Error::Internal(anyhow!("could not hash password: {e}")))
}

fn verify_password(password: &str, stored_hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(stored_hash)
        .map_err(|e| Error::Internal(anyhow!("stored password hash is malformed: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn validate_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD {
        return Err(bad_request(format!(
            "password must be at least {MIN_PASSWORD} characters"
        )));
    }
    Ok(())
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub async fn admin_exists(db: &PgPool) -> Result<bool> {
    Ok(sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM admin) AS "exists!""#)
        .fetch_one(db)
        .await?)
}

/// Creates the admin. Only ever succeeds once: the table holds a single row.
pub async fn setup(db: &PgPool, email: &str, password: &str) -> Result<()> {
    let email = email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(bad_request("enter a valid email address"));
    }
    validate_password(password)?;
    let hash = hash_password(password)?;
    let inserted = sqlx::query!(
        "INSERT INTO admin (email, password_hash) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        email,
        hash
    )
    .execute(db)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(Error::Conflict("setup is already complete".into()));
    }
    Ok(())
}

/// Checks credentials and opens a session, returning its token.
pub async fn login(state: &SharedState, email: &str, password: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    if !state.login_limiter.check(&email) {
        return Err(Error::RateLimited);
    }
    let admin = sqlx::query!("SELECT email, password_hash FROM admin")
        .fetch_optional(&state.db)
        .await?;
    let ok = match &admin {
        Some(admin) => admin.email == email && verify_password(password, &admin.password_hash)?,
        None => false,
    };
    if !ok {
        return Err(bad_request("wrong email or password"));
    }
    state.login_limiter.reset(&email);

    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    sqlx::query!(
        "INSERT INTO sessions (token_hash, expires_at) VALUES ($1, now() + make_interval(days => $2))",
        hash_token(&token),
        SESSION_DAYS as i32
    )
    .execute(&state.db)
    .await?;
    Ok(token)
}

pub async fn logout(db: &PgPool, token: &str) -> Result<()> {
    sqlx::query!("DELETE FROM sessions WHERE token_hash = $1", hash_token(token))
        .execute(db)
        .await?;
    Ok(())
}

pub async fn admin_email(db: &PgPool) -> Result<String> {
    Ok(sqlx::query_scalar!("SELECT email FROM admin")
        .fetch_one(db)
        .await?)
}

/// Checks the admin's current password, counting toward the login rate limit.
pub async fn check_password(state: &SharedState, password: &str) -> Result<bool> {
    if !state.login_limiter.check("password-change") {
        return Err(Error::RateLimited);
    }
    let hash = sqlx::query_scalar!("SELECT password_hash FROM admin")
        .fetch_one(&state.db)
        .await?;
    verify_password(password, &hash)
}

pub async fn set_password(db: &PgPool, password: &str) -> Result<()> {
    let hash = hash_password(password)?;
    let updated = sqlx::query!("UPDATE admin SET password_hash = $1", hash)
        .execute(db)
        .await?
        .rows_affected();
    if updated == 0 {
        return Err(bad_request("no admin exists yet; open the dashboard to create one"));
    }
    Ok(())
}

/// Sets a new password and signs every session out. Used by `justdeploy reset-password`.
pub async fn reset_password(db: &PgPool, password: &str) -> Result<()> {
    set_password(db, password).await?;
    sqlx::query!("DELETE FROM sessions").execute(db).await?;
    Ok(())
}

pub fn session_cookie(token: String, secure: bool) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, token))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(secure)
        .max_age(time::Duration::days(SESSION_DAYS))
        .build()
}

pub fn removal_cookie() -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, ""))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time::Duration::seconds(0))
        .build()
}

pub async fn session_valid(db: &PgPool, token: &str) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM sessions WHERE token_hash = $1 AND expires_at > now()) AS "valid!""#,
        hash_token(token)
    )
    .fetch_one(db)
    .await?)
}

/// Extractor: the request carries a valid admin session.
pub struct Admin;

impl FromRequestParts<SharedState> for Admin {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar.get(COOKIE_NAME).ok_or(Error::Unauthorized)?;
        if session_valid(&state.db, token.value()).await? {
            Ok(Admin)
        } else {
            Err(Error::Unauthorized)
        }
    }
}

/// Login attempts per email in a sliding window. In memory: it resets on restart,
/// which is fine for one process and one admin.
pub struct RateLimiter {
    entries: Mutex<HashMap<String, (u32, Instant)>>,
    max: u32,
    window: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            max: 10,
            window: Duration::from_secs(15 * 60),
        }
    }
}

impl RateLimiter {
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let Ok(mut entries) = self.entries.lock() else {
            return true;
        };
        if entries.len() > 1_000 {
            entries.retain(|_, (_, first)| now.duration_since(*first) < self.window);
        }
        let entry = entries.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= self.window {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 <= self.max
    }

    pub fn reset(&self, key: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_refuses_after_the_limit_and_resets_on_success() {
        let limiter = RateLimiter::default();
        for _ in 0..10 {
            assert!(limiter.check("a@b.c"));
        }
        assert!(!limiter.check("a@b.c"));
        limiter.reset("a@b.c");
        assert!(limiter.check("a@b.c"));
    }

    #[test]
    fn passwords_hash_and_verify() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(verify_password("correct horse battery", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn short_passwords_are_refused() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("long enough pw").is_ok());
    }
}
