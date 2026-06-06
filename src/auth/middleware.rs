use axum::{
    extract::{FromRequestParts, Request, State},
    http::{header, request::Parts},
    middleware::Next,
    response::Response,
};

use crate::auth::session;
use crate::db::models::UserRole;
use crate::error::AppError;
use crate::AppState;

/// Current user info extracted from session cookie. None = anonymous.
#[derive(Debug, Clone)]
pub struct CurrentUser(pub Option<AuthenticatedUser>);

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub id: i32,
    pub username: String,
    pub role: UserRole,
    pub avatar_url: Option<String>,
}

impl AuthenticatedUser {
    pub fn template_only(username: impl Into<String>) -> Self {
        Self {
            id: 0,
            username: username.into(),
            role: UserRole::User,
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

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        let session_id = session::extract_session_cookie(&parts.headers);
        if let Some(sid) = session_id {
            if let Ok(Some(session)) = session::validate_session(&app_state.pool, &sid).await {
                if let (Some(user_id), Some(role)) = (session.user_id, session.role) {
                    if let Ok(user) = sqlx::query_as::<_, UserRow>(
                        "SELECT id, username, avatar_url FROM users WHERE id = $1",
                    )
                    .bind(user_id)
                    .fetch_optional(&app_state.pool)
                    .await
                    {
                        if let Some(u) = user {
                            return Ok(CurrentUser(Some(AuthenticatedUser {
                                id: u.id,
                                username: u.username,
                                role,
                                avatar_url: u.avatar_url,
                            })));
                        }
                    }
                }
            }
        }

        Ok(CurrentUser(None))
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i32,
    username: String,
    avatar_url: Option<String>,
}

/// Extractor that requires authentication -- rejects with 401 if not logged in.
pub struct RequireAuth(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireAuth
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let current = CurrentUser::from_request_parts(parts, state).await.unwrap();
        match current.0 {
            Some(user) => Ok(RequireAuth(user)),
            None => Err(AppError::Unauthorized),
        }
    }
}

/// Extractor that requires moderator or above.
pub struct RequireModerator(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireModerator
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let RequireAuth(user) = RequireAuth::from_request_parts(parts, state).await?;
        if user.role.can_moderate() {
            Ok(RequireModerator(user))
        } else {
            Err(AppError::Forbidden)
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

pub async fn guest_session_layer(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let session_id = session::extract_session_cookie(request.headers());
    let has_valid_session = if let Some(ref sid) = session_id {
        session::validate_session(&state.pool, sid)
            .await
            .ok()
            .flatten()
            .is_some()
    } else {
        false
    };
    let is_static = request.uri().path().starts_with("/static/");

    let (ip, ua) = if !has_valid_session && !is_static {
        let ip = session::extract_client_ip(request.headers());
        let ua = request
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        (ip, ua)
    } else {
        (None, None)
    };

    // Create a guest session before handling the request so its IP can
    // participate in online stats.
    let created_guest_sid = if !has_valid_session && !is_static {
        session::create_guest_session(&state.pool, ip.as_deref(), ua.as_deref())
            .await
            .ok()
    } else {
        None
    };

    let mut response = next.run(request).await;

    if let Some(sid) = created_guest_sid {
        let session_cookie_prefix = format!("{}=", session::SESSION_COOKIE_NAME);
        let already_set = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|v| {
                v.to_str()
                    .map(|s| s.starts_with(&session_cookie_prefix))
                    .unwrap_or(false)
            });

        if already_set {
            let _ = session::delete_session(&state.pool, &sid).await;
        } else {
            let cookie = format!(
                "{}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400",
                session::SESSION_COOKIE_NAME
            );
            if let Ok(val) = cookie.parse() {
                response.headers_mut().append(header::SET_COOKIE, val);
            }
        }
    }

    response
}
