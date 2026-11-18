//! Signing in and out, and what each person manages for themselves: their
//! password, two-factor sign-in and API tokens. Also accepting an invite.

use crate::{
    auth::{self, Can, Viewer, COOKIE_NAME},
    error::{bad_request, Result},
    services,
    state::SharedState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Serialize)]
pub struct Status {
    setup_required: bool,
    authenticated: bool,
    /// Set when this hostname is a project's status page: the project to show.
    status_page: Option<String>,
}

/// Lets the dashboard decide between the setup screen, the login screen and the app.
pub async fn status(State(state): State<SharedState>, headers: HeaderMap) -> Result<Json<Status>> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    Ok(Json(Status {
        setup_required: !auth::users_exist(&state.db).await?,
        authenticated: auth::current_user(&state, &headers).await?.is_some(),
        status_page: crate::uptime::page_for_host(&state, host).await?,
    }))
}

#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
    /// The one-time code, for accounts with two-factor sign-in.
    code: Option<String>,
}

pub async fn setup(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<Credentials>,
) -> Result<CookieJar> {
    auth::setup(&state.db, &body.email, &body.password).await?;
    let token = auth::login(&state, &body.email, &body.password, None).await?;
    Ok(jar.add(auth::session_cookie(token, state.config.cookie_secure)))
}

/// 401 with `TwoFactorRequired` means the password was right and a code is needed.
pub async fn login(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<Credentials>,
) -> Result<CookieJar> {
    let token = auth::login(&state, &body.email, &body.password, body.code.as_deref()).await?;
    services::audit(
        &state.db,
        Some(&body.email.trim().to_lowercase()),
        "signed in",
        "",
        None,
    )
    .await;
    Ok(jar.add(auth::session_cookie(token, state.config.cookie_secure)))
}

pub async fn logout(State(state): State<SharedState>, jar: CookieJar) -> Result<CookieJar> {
    if let Some(cookie) = jar.get(COOKIE_NAME) {
        auth::logout(&state.db, cookie.value()).await?;
    }
    Ok(jar.add(auth::removal_cookie()))
}

pub async fn me(Can(user): Viewer, State(state): State<SharedState>) -> Result<Json<auth::Person>> {
    Ok(Json(auth::get_user(&state.db, user.id).await?))
}

#[derive(Deserialize)]
pub struct PasswordChange {
    current: String,
    new: String,
}

pub async fn change_password(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Json(body): Json<PasswordChange>,
) -> Result<StatusCode> {
    if !auth::check_password(&state, &user, &body.current).await? {
        return Err(bad_request("the current password is wrong"));
    }
    auth::set_password(&state.db, user.id, &body.new).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- two-factor

pub async fn start_two_factor(
    Can(user): Viewer,
    State(state): State<SharedState>,
) -> Result<Json<auth::TwoFactorSetup>> {
    Ok(Json(auth::start_two_factor(&state, &user).await?))
}

#[derive(Deserialize)]
pub struct Code {
    code: String,
}

/// Returns the recovery codes, shown once.
pub async fn confirm_two_factor(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Json(body): Json<Code>,
) -> Result<Json<Vec<String>>> {
    Ok(Json(
        auth::confirm_two_factor(&state, &user, &body.code).await?,
    ))
}

/// New recovery codes, replacing the old ones. Asks for the password, so a
/// stolen session can't mint a way past the authenticator.
pub async fn new_recovery_codes(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Json(body): Json<Password>,
) -> Result<Json<Vec<String>>> {
    if !auth::check_password(&state, &user, &body.password).await? {
        return Err(bad_request("the password is wrong"));
    }
    if !auth::get_user(&state.db, user.id).await?.two_factor() {
        return Err(bad_request("turn on two-factor sign-in first"));
    }
    Ok(Json(auth::new_recovery_codes(&state.db, user.id).await?))
}

#[derive(Deserialize)]
pub struct Password {
    password: String,
}

/// Turning it off asks for the password, so a stolen session can't.
pub async fn disable_two_factor(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Json(body): Json<Password>,
) -> Result<StatusCode> {
    if !auth::check_password(&state, &user, &body.password).await? {
        return Err(bad_request("the password is wrong"));
    }
    auth::disable_two_factor(&state.db, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- API tokens

pub async fn list_tokens(
    Can(user): Viewer,
    State(state): State<SharedState>,
) -> Result<Json<Vec<auth::ApiToken>>> {
    Ok(Json(auth::list_tokens(&state.db, &user).await?))
}

#[derive(Deserialize)]
pub struct NewToken {
    name: String,
}

pub async fn create_token(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Json(body): Json<NewToken>,
) -> Result<(StatusCode, Json<Value>)> {
    let (token, value) = auth::create_token(&state.db, &user, &body.name).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "token": token, "value": value })),
    ))
}

pub async fn delete_token(
    Can(user): Viewer,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    auth::delete_token(&state.db, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------- invites (no session)

pub async fn invite_info(
    State(state): State<SharedState>,
    Path(token): Path<String>,
) -> Result<Json<auth::Invite>> {
    Ok(Json(auth::invite_info(&state.db, &token).await?))
}

pub async fn accept_invite(
    State(state): State<SharedState>,
    Path(token): Path<String>,
    jar: CookieJar,
    Json(body): Json<Password>,
) -> Result<CookieJar> {
    let (session, email) = auth::accept_invite(&state.db, &token, &body.password).await?;
    services::audit(&state.db, Some(&email), "joined", "", None).await;
    Ok(jar.add(auth::session_cookie(session, state.config.cookie_secure)))
}
