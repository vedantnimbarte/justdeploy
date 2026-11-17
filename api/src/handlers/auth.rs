use crate::{
    auth::{self, Admin, COOKIE_NAME},
    error::Result,
    state::SharedState,
};
use axum::{extract::State, Json};
use axum_extra::extract::CookieJar;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct Status {
    setup_required: bool,
    authenticated: bool,
}

/// Lets the dashboard decide between the setup screen, the login screen and the app.
pub async fn status(State(state): State<SharedState>, jar: CookieJar) -> Result<Json<Status>> {
    let setup_required = !auth::admin_exists(&state.db).await?;
    let authenticated = match jar.get(COOKIE_NAME) {
        Some(cookie) => auth::session_valid(&state.db, cookie.value()).await?,
        None => false,
    };
    Ok(Json(Status {
        setup_required,
        authenticated,
    }))
}

#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
}

pub async fn setup(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<Credentials>,
) -> Result<CookieJar> {
    auth::setup(&state.db, &body.email, &body.password).await?;
    let token = auth::login(&state, &body.email, &body.password).await?;
    Ok(jar.add(auth::session_cookie(token, state.config.cookie_secure)))
}

pub async fn login(
    State(state): State<SharedState>,
    jar: CookieJar,
    Json(body): Json<Credentials>,
) -> Result<CookieJar> {
    let token = auth::login(&state, &body.email, &body.password).await?;
    Ok(jar.add(auth::session_cookie(token, state.config.cookie_secure)))
}

pub async fn logout(State(state): State<SharedState>, jar: CookieJar) -> Result<CookieJar> {
    if let Some(cookie) = jar.get(COOKIE_NAME) {
        auth::logout(&state.db, cookie.value()).await?;
    }
    Ok(jar.add(auth::removal_cookie()))
}

#[derive(Serialize)]
pub struct Me {
    email: String,
}

pub async fn me(_: Admin, State(state): State<SharedState>) -> Result<Json<Me>> {
    Ok(Json(Me {
        email: auth::admin_email(&state.db).await?,
    }))
}
