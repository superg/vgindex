use axum::{
    extract::{FromRequestParts, Request, State},
    http::{header, request::Parts, HeaderMap, Method, Uri},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use chrono::{DateTime, Duration, Utc};

use crate::auth::session;
use crate::db::models::{Session, UserRole};
use crate::error::AppError;
use crate::AppState;

const OIDC_REVALIDATION_HOURS: i64 = 24;
const OIDC_REVALIDATION_RETRY_MINUTES: i64 = 15;

/// Current user info extracted from session cookie. None = anonymous.
#[derive(Debug, Clone)]
pub struct CurrentUser(pub Option<AuthenticatedUser>);

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub id: i32,
    pub username: String,
    pub role: UserRole,
    pub csrf_token: String,
    pub avatar_url: Option<String>,
}

impl AuthenticatedUser {
    pub fn template_only(username: impl Into<String>) -> Self {
        Self {
            id: 0,
            username: username.into(),
            role: UserRole::User,
            csrf_token: "test-csrf-token".to_string(),
            avatar_url: None,
        }
    }
}

impl CurrentUser {
    pub fn user(&self) -> Option<&AuthenticatedUser> {
        self.0.as_ref()
    }

    pub fn role(&self) -> UserRole {
        self.0.as_ref().map(|u| u.role).unwrap_or(UserRole::User)
    }

    pub fn is_logged_in(&self) -> bool {
        self.0.is_some()
    }

    pub fn can_view_disabled_discs(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|u| u.role.can_view_disabled_discs())
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i32,
    username: String,
    avatar_url: Option<String>,
}

async fn load_authenticated_session(
    state: &AppState,
    sid: &str,
) -> Result<Option<(AuthenticatedUser, Session)>, sqlx::Error> {
    let Some(session) = session::validate_session(&state.pool, sid).await? else {
        return Ok(None);
    };
    let (Some(user_id), Some(role)) = (session.user_id, session.role) else {
        return Ok(None);
    };
    let Some(user) =
        sqlx::query_as::<_, UserRow>("SELECT id, username, avatar_url FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?
    else {
        return Ok(None);
    };

    Ok(Some((
        AuthenticatedUser {
            id: user.id,
            username: user.username,
            role,
            csrf_token: session.csrf_token.clone().unwrap_or_default(),
            avatar_url: user.avatar_url,
        },
        session,
    )))
}

fn is_top_level_navigation(method: &Method, headers: &HeaderMap) -> bool {
    if method != Method::GET && method != Method::HEAD {
        return false;
    }
    if headers.contains_key("hx-request") {
        return false;
    }
    match headers
        .get("sec-fetch-mode")
        .and_then(|value| value.to_str().ok())
    {
        Some(mode) => mode.eq_ignore_ascii_case("navigate"),
        None => headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html")),
    }
}

fn is_api_request(uri: &Uri, headers: &HeaderMap) -> bool {
    uri.path().starts_with("/api/")
        || headers
            .get(header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|accept| accept.contains("application/json"))
}

fn automatic_sso_exempt_path(path: &str) -> bool {
    matches!(
        path,
        "/login" | "/auth/oidc/revalidate" | "/auth/oidc/callback"
    )
}

fn root_relative_uri(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/")
        .to_string()
}

fn oidc_redirect(path: &str, return_to: &str) -> Response {
    let location = format!("{path}?return_to={}", urlencoding::encode(return_to));
    Redirect::to(&location).into_response()
}

fn revalidation_is_due(session: &Session, now: DateTime<Utc>) -> bool {
    session.oidc_validated_at <= now - Duration::hours(OIDC_REVALIDATION_HOURS)
}

fn revalidation_retry_is_due(session: &Session, now: DateTime<Utc>) -> bool {
    session
        .oidc_revalidation_attempted_at
        .is_none_or(|attempted| {
            attempted <= now - Duration::minutes(OIDC_REVALIDATION_RETRY_MINUTES)
        })
}

fn response_sets_session_cookie(response: &Response) -> bool {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with(&format!("{}=", session::SESSION_COOKIE_NAME)))
}

fn append_expired_session_cookie(response: &mut Response, state: &AppState) {
    if response_sets_session_cookie(response) {
        return;
    }
    response.headers_mut().append(
        header::SET_COOKIE,
        session::expired_session_cookie(&state.config)
            .parse()
            .expect("session cookie must be a valid header"),
    );
}

pub async fn session_auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let sid = session::extract_session_cookie(request.headers());
    let loaded = if let Some(sid) = sid.as_deref() {
        match load_authenticated_session(&state, sid).await {
            Ok(loaded) => loaded,
            Err(error) => {
                tracing::warn!("Failed to load app session: {error}");
                None
            }
        }
    } else {
        None
    };
    let invalid_session_cookie = sid.is_some() && loaded.is_none();

    request
        .extensions_mut()
        .insert(CurrentUser(loaded.as_ref().map(|(user, _)| user.clone())));

    let navigation = is_top_level_navigation(request.method(), request.headers());
    let return_to = root_relative_uri(request.uri());
    let exempt = automatic_sso_exempt_path(request.uri().path());

    if let Some((_, app_session)) = loaded.as_ref() {
        let now = Utc::now();
        if revalidation_is_due(app_session, now) {
            if is_api_request(request.uri(), request.headers()) {
                return AppError::Unauthorized.into_response();
            }
            if navigation && !exempt && revalidation_retry_is_due(app_session, now) {
                if let Some(sid) = sid.as_deref() {
                    if let Err(error) = session::mark_revalidation_attempt(&state.pool, sid).await {
                        tracing::warn!("Failed to record OIDC revalidation attempt: {error}");
                    }
                }
                return oidc_redirect("/auth/oidc/revalidate", &return_to);
            }
        }
    } else if navigation && !exempt && !session::has_sso_check_cookie(request.headers()) {
        let mut response = oidc_redirect("/auth/oidc/revalidate", &return_to);
        if invalid_session_cookie {
            append_expired_session_cookie(&mut response, &state);
        }
        return response;
    }

    let mut response = next.run(request).await;
    if invalid_session_cookie {
        append_expired_session_cookie(&mut response, &state);
    }
    response
}

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(current) = parts.extensions.get::<CurrentUser>() {
            return Ok(current.clone());
        }

        let app_state = AppState::from_ref(state);
        let current = if let Some(sid) = session::extract_session_cookie(&parts.headers) {
            match load_authenticated_session(&app_state, &sid).await {
                Ok(Some((user, _))) => Some(user),
                Ok(None) => None,
                Err(error) => {
                    tracing::warn!("Failed to load app session: {error}");
                    None
                }
            }
        } else {
            None
        };

        Ok(CurrentUser(current))
    }
}

fn authentication_required(parts: &Parts) -> Response {
    if is_top_level_navigation(&parts.method, &parts.headers) {
        return oidc_redirect("/login", &root_relative_uri(&parts.uri));
    }
    AppError::Unauthorized.into_response()
}

/// Extractor that requires authentication.
pub struct RequireAuth(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireAuth
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let current = CurrentUser::from_request_parts(parts, state).await.unwrap();
        current
            .0
            .map(RequireAuth)
            .ok_or_else(|| authentication_required(parts))
    }
}

/// Extractor that requires moderator or above.
pub struct RequireModerator(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireModerator
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let RequireAuth(user) = RequireAuth::from_request_parts(parts, state).await?;
        if user.role.can_moderate() {
            Ok(RequireModerator(user))
        } else {
            Err(AppError::Forbidden.into_response())
        }
    }
}

/// Extractor that requires an administrator.
pub struct RequireAdmin(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let RequireAuth(user) = RequireAuth::from_request_parts(parts, state).await?;
        if user.role.can_admin() {
            Ok(RequireAdmin(user))
        } else {
            Err(AppError::Forbidden.into_response())
        }
    }
}

// We need FromRef to extract AppState from any state type.
pub trait FromRef<T> {
    fn from_ref(input: &T) -> Self;
}

impl FromRef<AppState> for AppState {
    fn from_ref(input: &AppState) -> Self {
        input.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(values: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in values {
            headers.insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        headers
    }

    #[test]
    fn browser_navigation_requires_navigation_fetch_or_html_accept() {
        assert!(is_top_level_navigation(
            &Method::GET,
            &headers(&[("sec-fetch-mode", "navigate")])
        ));
        assert!(is_top_level_navigation(
            &Method::GET,
            &headers(&[("accept", "text/html,application/xhtml+xml")])
        ));
        assert!(!is_top_level_navigation(
            &Method::POST,
            &headers(&[("sec-fetch-mode", "navigate")])
        ));
        assert!(!is_top_level_navigation(
            &Method::GET,
            &headers(&[("hx-request", "true"), ("accept", "text/html")])
        ));
    }

    #[test]
    fn revalidation_obeys_success_and_retry_windows() {
        let now = Utc::now();
        let session = Session {
            id: "sid".into(),
            user_id: Some(1),
            role: Some(UserRole::User),
            csrf_token: Some("csrf".into()),
            ip_address: None,
            user_agent: None,
            created_at: now - Duration::days(2),
            last_active_at: now,
            oidc_validated_at: now - Duration::hours(25),
            oidc_revalidation_attempted_at: None,
        };

        assert!(revalidation_is_due(&session, now));
        assert!(revalidation_retry_is_due(&session, now));

        let mut attempted = session;
        attempted.oidc_revalidation_attempted_at = Some(now - Duration::minutes(5));
        assert!(!revalidation_retry_is_due(&attempted, now));
    }
}
