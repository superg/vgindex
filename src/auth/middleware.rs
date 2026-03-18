use axum::{
    extract::FromRequestParts,
    http::request::Parts,
};

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
}

impl<S> FromRequestParts<S> for CurrentUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        let session_id = extract_session_cookie(&parts.headers);
        if let Some(sid) = session_id {
            if let Ok(Some(session)) =
                crate::auth::session::validate_session(&app_state.pool, &sid).await
            {
                if let Some(user_id) = session.user_id {
                    if let Ok(user) = sqlx::query_as::<_, UserRow>(
                        "SELECT id, username, role FROM users WHERE id = $1 AND is_active = true",
                    )
                    .bind(user_id)
                    .fetch_optional(&app_state.pool)
                    .await
                    {
                        if let Some(u) = user {
                            return Ok(CurrentUser(Some(AuthenticatedUser {
                                id: u.id,
                                username: u.username,
                                role: u.role,
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
    role: UserRole,
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
        let current = CurrentUser::from_request_parts(parts, state)
            .await
            .unwrap();
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

fn extract_session_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let cookie_header = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("session_id=") {
            return Some(value.to_string());
        }
    }
    None
}
