use askama::Template;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::auth::session;
use crate::config::SiteConfig;
use crate::error::AppError;
use crate::services::user_service;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page).post(login_submit))
        // Demo/public mode: disable open self-registration.
        // Re-enable by uncommenting this route.
        // .route("/register", get(register_page).post(register_submit))
        .route("/logout", post(logout))
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    current_user: Option<String>,
    error: Option<String>,
    return_to: Option<String>,
}
impl SiteConfig for LoginTemplate {}

#[derive(Deserialize)]
struct LoginQuery {
    return_to: Option<String>,
}

#[derive(Template)]
#[template(path = "register.html")]
struct RegisterTemplate {
    current_user: Option<String>,
    error: Option<String>,
    success: Option<String>,
}
impl SiteConfig for RegisterTemplate {}

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    pub return_to: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub email: String,
    pub password: String,
    pub password_confirm: String,
}

async fn login_page(user: CurrentUser, Query(query): Query<LoginQuery>) -> impl IntoResponse {
    if user.is_logged_in() {
        return Redirect::to("/").into_response();
    }
    Html(
        LoginTemplate {
            current_user: None,
            error: None,
            return_to: query.return_to,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

async fn login_submit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    match user_service::authenticate(&state.pool, &form.username, &form.password).await {
        Ok(user) => {
            let ip = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());
            let ua = headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let sid = session::create_session(&state.pool, user.id, ip.as_deref(), ua.as_deref())
                .await
                .unwrap();

            let redirect_target = form
                .return_to
                .as_deref()
                .filter(|u| u.starts_with('/'))
                .unwrap_or("/");

            let cookie = format!(
                "session_id={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
                14 * 86400
            );
            let mut response = Redirect::to(redirect_target).into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, cookie.parse().unwrap());
            response
        }
        Err(AppError::BadRequest(msg)) => Html(
            LoginTemplate {
                current_user: None,
                error: Some(msg),
                return_to: form.return_to,
            }
            .render()
            .unwrap(),
        )
        .into_response(),
        Err(e) => e.into_response(),
    }
}

async fn register_page(user: CurrentUser) -> impl IntoResponse {
    if user.is_logged_in() {
        return Redirect::to("/").into_response();
    }
    Html(
        RegisterTemplate {
            current_user: None,
            error: None,
            success: None,
        }
        .render()
        .unwrap(),
    )
    .into_response()
}

async fn register_submit(
    State(state): State<AppState>,
    Form(form): Form<RegisterForm>,
) -> impl IntoResponse {
    if form.password != form.password_confirm {
        return Html(
            RegisterTemplate {
                current_user: None,
                error: Some("Passwords do not match".into()),
                success: None,
            }
            .render()
            .unwrap(),
        );
    }

    match user_service::register(&state.pool, &form.username, &form.email, &form.password).await {
        Ok(_user) => Html(
            RegisterTemplate {
                current_user: None,
                error: None,
                success: Some(
                    "Registration successful! Check your email to verify your account.".into(),
                ),
            }
            .render()
            .unwrap(),
        ),
        Err(AppError::BadRequest(msg)) => Html(
            RegisterTemplate {
                current_user: None,
                error: Some(msg),
                success: None,
            }
            .render()
            .unwrap(),
        ),
        Err(_) => Html(
            RegisterTemplate {
                current_user: None,
                error: Some("An unexpected error occurred".into()),
                success: None,
            }
            .render()
            .unwrap(),
        ),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(cookie_header) = headers.get(header::COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            for part in cookie_str.split(';') {
                let part = part.trim();
                if let Some(sid) = part.strip_prefix("session_id=") {
                    session::delete_session(&state.pool, sid).await.ok();
                }
            }
        }
    }

    let cookie = "session_id=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0";
    let mut response = Redirect::to("/").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie.parse().unwrap());
    response
}
