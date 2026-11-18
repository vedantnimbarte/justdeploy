//! People and what they may do: accounts and roles, sessions, API tokens,
//! invites and two-factor sign-in.

use crate::{
    crypto::random_string,
    error::{bad_request, Error, Result},
    state::SharedState,
};
use anyhow::anyhow;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts, HeaderMap},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use totp_rs::{Algorithm, Secret, TOTP};
use uuid::Uuid;

pub const COOKIE_NAME: &str = "jd_session";
const SESSION_DAYS: i64 = 30;
const INVITE_DAYS: i32 = 7;
const MIN_PASSWORD: usize = 10;
/// API tokens start with this, so they are easy to spot in a leaked file.
const TOKEN_PREFIX: &str = "jd_";

// ---------------------------------------------------------------- roles

/// Each role can do everything the ones before it can.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Reads everything except secrets.
    Viewer = 0,
    /// Deploys and changes services, runs commands, opens terminals.
    Member = 1,
    /// Deletes projects and services, restores backups, changes settings.
    Admin = 2,
    /// Manages people.
    Owner = 3,
}

impl Role {
    pub fn parse(value: &str) -> Result<Role> {
        match value {
            "viewer" => Ok(Role::Viewer),
            "member" => Ok(Role::Member),
            "admin" => Ok(Role::Admin),
            "owner" => Ok(Role::Owner),
            _ => Err(bad_request("role must be owner, admin, member or viewer")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Member => "member",
            Role::Admin => "admin",
            Role::Owner => "owner",
        }
    }

    fn from_level(level: u8) -> Role {
        match level {
            0 => Role::Viewer,
            1 => Role::Member,
            2 => Role::Admin,
            _ => Role::Owner,
        }
    }
}

#[derive(Clone, Debug)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
}

/// Extractor: a signed-in person (session cookie or API token) whose role is at
/// least `LEVEL`. Use the aliases below.
pub struct Can<const LEVEL: u8>(pub User);
pub type Viewer = Can<0>;
pub type Member = Can<1>;
pub type Admin = Can<2>;
pub type Owner = Can<3>;

impl<const LEVEL: u8> FromRequestParts<SharedState> for Can<LEVEL> {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &SharedState) -> Result<Self> {
        let user = current_user(state, &parts.headers)
            .await?
            .ok_or(Error::Unauthorized)?;
        let needed = Role::from_level(LEVEL);
        if user.role < needed {
            return Err(Error::Forbidden(format!(
                "this needs the {} role; you are a {}",
                needed.as_str(),
                user.role.as_str()
            )));
        }
        Ok(Can(user))
    }
}

/// Whoever the request is from: an API token in `Authorization: Bearer`, or the
/// session cookie.
pub async fn current_user(state: &SharedState, headers: &HeaderMap) -> Result<Option<User>> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let row = if let Some(token) = bearer {
        sqlx::query!(
            "UPDATE api_tokens t SET last_used_at = now() FROM users u
             WHERE t.token_hash = $1 AND u.id = t.user_id
             RETURNING u.id, u.email, u.role",
            hash_token(token)
        )
        .fetch_optional(&state.db)
        .await?
        .map(|r| (r.id, r.email, r.role))
    } else {
        let jar = CookieJar::from_headers(headers);
        let Some(cookie) = jar.get(COOKIE_NAME) else {
            return Ok(None);
        };
        sqlx::query!(
            "SELECT u.id, u.email, u.role FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now()",
            hash_token(cookie.value())
        )
        .fetch_optional(&state.db)
        .await?
        .map(|r| (r.id, r.email, r.role))
    };
    row.map(|(id, email, role)| {
        Ok(User {
            id,
            email,
            role: Role::parse(&role)?,
        })
    })
    .transpose()
}

// ---------------------------------------------------------------- passwords

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

fn normalize_email(email: &str) -> Result<String> {
    let email = email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(bad_request("enter a valid email address"));
    }
    Ok(email)
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Checks a person's current password, counting toward the login rate limit.
pub async fn check_password(state: &SharedState, user: &User, password: &str) -> Result<bool> {
    if !state.login_limiter.check(&user.email) {
        return Err(Error::RateLimited);
    }
    let hash = sqlx::query_scalar!("SELECT password_hash FROM users WHERE id = $1", user.id)
        .fetch_one(&state.db)
        .await?;
    verify_password(password, &hash)
}

pub async fn set_password(db: &PgPool, user_id: Uuid, password: &str) -> Result<()> {
    validate_password(password)?;
    sqlx::query!(
        "UPDATE users SET password_hash = $2 WHERE id = $1",
        user_id,
        hash_password(password)?
    )
    .execute(db)
    .await?;
    Ok(())
}

/// `justdeploy reset-password [email]`: a new password for that person (the
/// first owner by default), two-factor sign-in turned off, and their sessions
/// ended. Returns whose password changed.
pub async fn reset_password(db: &PgPool, email: Option<&str>, password: &str) -> Result<String> {
    let user = sqlx::query!(
        "SELECT id, email FROM users
         WHERE ($1::text IS NULL AND role = 'owner') OR email = $1
         ORDER BY created_at LIMIT 1",
        email.map(str::to_lowercase)
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| bad_request("no such account; open the dashboard to create the first one"))?;
    sqlx::query!(
        "UPDATE users SET password_hash = $2, totp_secret_enc = NULL, totp_pending_enc = NULL
         WHERE id = $1",
        user.id,
        hash_password(password)?
    )
    .execute(db)
    .await?;
    sqlx::query!("DELETE FROM sessions WHERE user_id = $1", user.id)
        .execute(db)
        .await?;
    Ok(user.email)
}

// ---------------------------------------------------------------- sign-in

pub async fn users_exist(db: &PgPool) -> Result<bool> {
    Ok(
        sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "exists!""#)
            .fetch_one(db)
            .await?,
    )
}

/// Creates the first owner. Only works while nobody has an account.
pub async fn setup(db: &PgPool, email: &str, password: &str) -> Result<()> {
    let email = normalize_email(email)?;
    validate_password(password)?;
    let inserted = sqlx::query!(
        "INSERT INTO users (id, email, password_hash, role)
         SELECT $1, $2, $3, 'owner' WHERE NOT EXISTS (SELECT 1 FROM users)",
        Uuid::new_v4(),
        email,
        hash_password(password)?
    )
    .execute(db)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Err(Error::Conflict("setup is already complete".into()));
    }
    Ok(())
}

/// Checks credentials, and the one-time code when the account has two-factor
/// sign-in, then opens a session and returns its token.
pub async fn login(
    state: &SharedState,
    email: &str,
    password: &str,
    code: Option<&str>,
) -> Result<String> {
    let email = email.trim().to_lowercase();
    if !state.login_limiter.check(&email) {
        return Err(Error::RateLimited);
    }
    let user = sqlx::query!(
        "SELECT id, password_hash, totp_secret_enc FROM users WHERE email = $1",
        email
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(user) = user.filter(|u| verify_password(password, &u.password_hash).unwrap_or(false))
    else {
        return Err(bad_request("wrong email or password"));
    };
    if let Some(secret) = &user.totp_secret_enc {
        let code = code.map(str::trim).filter(|c| !c.is_empty());
        let Some(code) = code else {
            return Err(Error::TwoFactorRequired);
        };
        let current = totp(&state.cipher.decrypt(secret)?, &email)?
            .check_current(code)
            .unwrap_or(false);
        if !current && !use_recovery_code(&state.db, user.id, code).await? {
            return Err(bad_request("that code is wrong or has expired"));
        }
    }
    state.login_limiter.reset(&email);
    create_session(&state.db, user.id).await
}

async fn create_session(db: &PgPool, user_id: Uuid) -> Result<String> {
    let token = random_string(64);
    sqlx::query!(
        "INSERT INTO sessions (token_hash, user_id, expires_at)
         VALUES ($1, $2, now() + make_interval(days => $3))",
        hash_token(&token),
        user_id,
        SESSION_DAYS as i32
    )
    .execute(db)
    .await?;
    Ok(token)
}

pub async fn logout(db: &PgPool, token: &str) -> Result<()> {
    sqlx::query!(
        "DELETE FROM sessions WHERE token_hash = $1",
        hash_token(token)
    )
    .execute(db)
    .await?;
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

// ---------------------------------------------------------------- two-factor sign-in

fn totp(encoded_secret: &str, email: &str) -> Result<TOTP> {
    let bytes = Secret::Encoded(encoded_secret.to_string())
        .to_bytes()
        .map_err(|e| Error::Internal(anyhow!("stored two-factor secret is malformed: {e:?}")))?;
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        bytes,
        Some("justdeploy".to_string()),
        email.to_string(),
    )
    .map_err(|e| Error::Internal(anyhow!("two-factor setup failed: {e}")))
}

#[derive(Serialize)]
pub struct TwoFactorSetup {
    /// For typing into an authenticator by hand.
    secret: String,
    otpauth_url: String,
    qr_svg: String,
}

/// Starts setup with a fresh secret. Nothing changes until a code from it is confirmed.
pub async fn start_two_factor(state: &SharedState, user: &User) -> Result<TwoFactorSetup> {
    let secret = Secret::generate_secret().to_encoded().to_string();
    let otpauth_url = totp(&secret, &user.email)?.get_url();
    let qr_svg = qrcode::QrCode::new(otpauth_url.as_bytes())
        .map_err(|e| Error::Internal(anyhow!("could not draw the QR code: {e}")))?
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build();
    sqlx::query!(
        "UPDATE users SET totp_pending_enc = $2 WHERE id = $1",
        user.id,
        state.cipher.encrypt(&secret)
    )
    .execute(&state.db)
    .await?;
    Ok(TwoFactorSetup {
        secret,
        otpauth_url,
        qr_svg,
    })
}

/// Ten codes that each sign in once in place of the authenticator, replacing
/// any from before. Only their hashes are kept; these are shown once.
pub async fn new_recovery_codes(db: &PgPool, user_id: Uuid) -> Result<Vec<String>> {
    let codes: Vec<String> = (0..10).map(|_| recovery_code()).collect();
    let hashes: Vec<String> = codes
        .iter()
        .map(|c| hash_token(&normalize_code(c)))
        .collect();
    let mut tx = db.begin().await?;
    sqlx::query!("DELETE FROM recovery_codes WHERE user_id = $1", user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "INSERT INTO recovery_codes (user_id, code_hash) SELECT $1, * FROM UNNEST($2::text[])",
        user_id,
        &hashes
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(codes)
}

/// `abcde-12345`: ten lowercase letters and digits, easy to read back.
fn recovery_code() -> String {
    let raw = random_string(10).to_lowercase();
    format!("{}-{}", &raw[..5], &raw[5..])
}

/// Spaces, dashes and case don't matter when typing one.
fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Spends an unused recovery code. Codes are ten characters, so a six-digit
/// authenticator code is never mistaken for one.
async fn use_recovery_code(db: &PgPool, user_id: Uuid, code: &str) -> Result<bool> {
    let code = normalize_code(code);
    if code.len() != 10 {
        return Ok(false);
    }
    let used = sqlx::query!(
        "UPDATE recovery_codes SET used_at = now()
         WHERE user_id = $1 AND code_hash = $2 AND used_at IS NULL",
        user_id,
        hash_token(&code)
    )
    .execute(db)
    .await?
    .rows_affected();
    Ok(used == 1)
}

/// Turns two-factor sign-in on and returns its recovery codes.
pub async fn confirm_two_factor(
    state: &SharedState,
    user: &User,
    code: &str,
) -> Result<Vec<String>> {
    let pending = sqlx::query_scalar!("SELECT totp_pending_enc FROM users WHERE id = $1", user.id)
        .fetch_one(&state.db)
        .await?
        .ok_or_else(|| bad_request("start two-factor setup first"))?;
    let secret = state.cipher.decrypt(&pending)?;
    if !totp(&secret, &user.email)?
        .check_current(code.trim())
        .unwrap_or(false)
    {
        return Err(bad_request(
            "that code is wrong or has expired; try the current one",
        ));
    }
    sqlx::query!(
        "UPDATE users SET totp_secret_enc = totp_pending_enc, totp_pending_enc = NULL WHERE id = $1",
        user.id
    )
    .execute(&state.db)
    .await?;
    new_recovery_codes(&state.db, user.id).await
}

pub async fn disable_two_factor(db: &PgPool, user_id: Uuid) -> Result<()> {
    sqlx::query!(
        "UPDATE users SET totp_secret_enc = NULL, totp_pending_enc = NULL WHERE id = $1",
        user_id
    )
    .execute(db)
    .await?;
    sqlx::query!("DELETE FROM recovery_codes WHERE user_id = $1", user_id)
        .execute(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- people

#[derive(Serialize)]
pub struct Person {
    id: Uuid,
    email: String,
    role: String,
    two_factor: bool,
    /// Unused recovery codes, when two-factor sign-in is on.
    recovery_codes_left: i64,
    created_at: DateTime<Utc>,
}

impl Person {
    pub fn two_factor(&self) -> bool {
        self.two_factor
    }
}

pub async fn list_users(db: &PgPool) -> Result<Vec<Person>> {
    Ok(sqlx::query_as!(
        Person,
        r#"SELECT id, email, role, totp_secret_enc IS NOT NULL AS "two_factor!", created_at,
                  (SELECT count(*) FROM recovery_codes r WHERE r.user_id = users.id AND r.used_at IS NULL)
                    AS "recovery_codes_left!"
           FROM users ORDER BY created_at"#
    )
    .fetch_all(db)
    .await?)
}

pub async fn get_user(db: &PgPool, id: Uuid) -> Result<Person> {
    Ok(sqlx::query_as!(
        Person,
        r#"SELECT id, email, role, totp_secret_enc IS NOT NULL AS "two_factor!", created_at,
                  (SELECT count(*) FROM recovery_codes r WHERE r.user_id = users.id AND r.used_at IS NULL)
                    AS "recovery_codes_left!"
           FROM users WHERE id = $1"#,
        id
    )
    .fetch_one(db)
    .await?)
}

/// Refuses a change that would leave nobody able to manage people.
async fn keep_an_owner(db: &PgPool, losing: Uuid) -> Result<()> {
    let others = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM users WHERE role = 'owner' AND id <> $1"#,
        losing
    )
    .fetch_one(db)
    .await?;
    let is_owner = sqlx::query_scalar!("SELECT role FROM users WHERE id = $1", losing)
        .fetch_one(db)
        .await?
        == "owner";
    if is_owner && others == 0 {
        return Err(bad_request(
            "make someone else an owner first; there must always be one",
        ));
    }
    Ok(())
}

pub async fn set_role(db: &PgPool, id: Uuid, role: Role) -> Result<()> {
    if role != Role::Owner {
        keep_an_owner(db, id).await?;
    }
    sqlx::query!(
        "UPDATE users SET role = $2 WHERE id = $1",
        id,
        role.as_str()
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn delete_user(db: &PgPool, id: Uuid) -> Result<()> {
    keep_an_owner(db, id).await?;
    sqlx::query!("DELETE FROM users WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- invites

#[derive(Serialize)]
pub struct Invite {
    id: Uuid,
    email: String,
    role: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

/// Returns the invite and the one-time token for its link.
pub async fn create_invite(
    db: &PgPool,
    by: &User,
    email: &str,
    role: Role,
) -> Result<(Invite, String)> {
    let email = normalize_email(email)?;
    if role == Role::Owner {
        return Err(bad_request("invite as an admin, then make them an owner"));
    }
    let taken = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM users WHERE email = $1) AS "taken!""#,
        email
    )
    .fetch_one(db)
    .await?;
    if taken {
        return Err(Error::Conflict(format!("{email} already has an account")));
    }
    let token = random_string(48);
    let invite = sqlx::query_as!(
        Invite,
        "INSERT INTO invites (id, token_hash, email, role, invited_by, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(days => $6))
         RETURNING id, email, role, expires_at, created_at",
        Uuid::new_v4(),
        hash_token(&token),
        email,
        role.as_str(),
        by.id,
        INVITE_DAYS
    )
    .fetch_one(db)
    .await?;
    Ok((invite, token))
}

pub async fn list_invites(db: &PgPool) -> Result<Vec<Invite>> {
    Ok(sqlx::query_as!(
        Invite,
        "SELECT id, email, role, expires_at, created_at FROM invites
         WHERE expires_at > now() ORDER BY created_at DESC"
    )
    .fetch_all(db)
    .await?)
}

pub async fn revoke_invite(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query!("DELETE FROM invites WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(())
}

/// What an invite link is for, while it is still valid.
pub async fn invite_info(db: &PgPool, token: &str) -> Result<Invite> {
    sqlx::query_as!(
        Invite,
        "SELECT id, email, role, expires_at, created_at FROM invites
         WHERE token_hash = $1 AND expires_at > now()",
        hash_token(token)
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| {
        bad_request("this invite link has expired or was already used; ask for a new one")
    })
}

/// Creates the account, spends the invite and signs the new person in.
pub async fn accept_invite(db: &PgPool, token: &str, password: &str) -> Result<(String, String)> {
    validate_password(password)?;
    let invite = invite_info(db, token).await?;
    let id = Uuid::new_v4();
    let mut tx = db.begin().await?;
    sqlx::query!(
        "INSERT INTO users (id, email, password_hash, role) VALUES ($1, $2, $3, $4)",
        id,
        invite.email,
        hash_password(password)?,
        invite.role
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| match Error::from(e) {
        Error::Conflict(_) => Error::Conflict(format!("{} already has an account", invite.email)),
        other => other,
    })?;
    sqlx::query!("DELETE FROM invites WHERE id = $1", invite.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((create_session(db, id).await?, invite.email))
}

// ---------------------------------------------------------------- API tokens

#[derive(Serialize)]
pub struct ApiToken {
    id: Uuid,
    name: String,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

pub async fn list_tokens(db: &PgPool, user: &User) -> Result<Vec<ApiToken>> {
    Ok(sqlx::query_as!(
        ApiToken,
        "SELECT id, name, last_used_at, created_at FROM api_tokens WHERE user_id = $1 ORDER BY created_at",
        user.id
    )
    .fetch_all(db)
    .await?)
}

/// Returns the token's record and its value, which is shown once and never stored.
pub async fn create_token(db: &PgPool, user: &User, name: &str) -> Result<(ApiToken, String)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad_request("name the token after where it will be used"));
    }
    let value = format!("{TOKEN_PREFIX}{}", random_string(40));
    let token = sqlx::query_as!(
        ApiToken,
        "INSERT INTO api_tokens (id, user_id, name, token_hash) VALUES ($1, $2, $3, $4)
         RETURNING id, name, last_used_at, created_at",
        Uuid::new_v4(),
        user.id,
        name,
        hash_token(&value)
    )
    .fetch_one(db)
    .await?;
    Ok((token, value))
}

pub async fn delete_token(db: &PgPool, user: &User, id: Uuid) -> Result<()> {
    let removed = sqlx::query!(
        "DELETE FROM api_tokens WHERE id = $1 AND user_id = $2",
        id,
        user.id
    )
    .execute(db)
    .await?
    .rows_affected();
    if removed == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

// ---------------------------------------------------------------- rate limit

/// Login attempts per email in a sliding window. In memory: it resets on
/// restart, which is fine for one process.
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
    fn recovery_codes_read_back_loosely() {
        let code = recovery_code();
        assert_eq!(code.len(), 11);
        assert_eq!(
            normalize_code(&code.to_uppercase().replace('-', " ")).len(),
            10
        );
        assert_eq!(normalize_code("ABcde-12 345"), "abcde12345");
    }

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

    #[test]
    fn roles_rank_and_round_trip() {
        assert!(
            Role::Viewer < Role::Member && Role::Member < Role::Admin && Role::Admin < Role::Owner
        );
        for role in [Role::Viewer, Role::Member, Role::Admin, Role::Owner] {
            assert_eq!(Role::parse(role.as_str()).unwrap(), role);
            assert_eq!(Role::from_level(role as u8), role);
        }
        assert!(Role::parse("root").is_err());
    }

    #[test]
    fn a_fresh_secret_accepts_its_own_current_code() {
        let secret = Secret::generate_secret().to_encoded().to_string();
        let totp = totp(&secret, "a@example.com").unwrap();
        let code = totp.generate_current().unwrap();
        assert!(totp.check_current(&code).unwrap());
        assert!(totp
            .get_url()
            .starts_with("otpauth://totp/justdeploy:a%40example.com?"));
    }
}
