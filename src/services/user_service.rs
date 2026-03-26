use sqlx::PgPool;

use crate::auth::password;
use crate::db::models::User;
use crate::error::{AppError, AppResult};

pub async fn register(
    pool: &PgPool,
    username: &str,
    email: &str,
    raw_password: &str,
) -> AppResult<User> {
    if username.len() < 3 || username.len() > 64 {
        return Err(AppError::BadRequest(
            "Username must be 3-64 characters".into(),
        ));
    }
    if raw_password.len() < 8 {
        return Err(AppError::BadRequest(
            "Password must be at least 8 characters".into(),
        ));
    }

    let existing: Option<(i32,)> = sqlx::query_as(
        "SELECT id FROM users WHERE lower(username) = lower($1) OR lower(email) = lower($2)",
    )
    .bind(username)
    .bind(email)
    .fetch_optional(pool)
    .await?;

    if existing.is_some() {
        return Err(AppError::BadRequest(
            "Username or email already exists".into(),
        ));
    }

    let hash = password::hash_password(raw_password)
        .map_err(|e| AppError::Internal(format!("Password hashing failed: {e}")))?;

    let verify_token = crate::auth::session::generate_session_id();

    let user: User = sqlx::query_as(
        "INSERT INTO users (username, email, password_hash, email_verify_token, email_verify_expires_at)
         VALUES ($1, $2, $3, $4, NOW() + INTERVAL '24 hours')
         RETURNING *",
    )
    .bind(username)
    .bind(email)
    .bind(&hash)
    .bind(&verify_token)
    .fetch_one(pool)
    .await?;

    Ok(user)
}

pub async fn authenticate(
    pool: &PgPool,
    username_or_email: &str,
    raw_password: &str,
) -> AppResult<User> {
    let user: Option<User> = sqlx::query_as(
        "SELECT * FROM users WHERE (lower(username) = lower($1) OR lower(email) = lower($1)) AND is_active = true",
    )
    .bind(username_or_email)
    .fetch_optional(pool)
    .await?;

    let user = user.ok_or(AppError::BadRequest("Invalid credentials".into()))?;

    if let Some(locked_until) = user.locked_until {
        if locked_until > chrono::Utc::now() {
            return Err(AppError::BadRequest(
                "Account is temporarily locked. Try again later.".into(),
            ));
        }
    }

    let valid = password::verify_password(raw_password, &user.password_hash)
        .map_err(|e| AppError::Internal(format!("Password verification failed: {e}")))?;

    if !valid {
        sqlx::query(
            "UPDATE users SET failed_login_attempts = failed_login_attempts + 1,
             locked_until = CASE WHEN failed_login_attempts >= 9 THEN NOW() + INTERVAL '15 minutes' ELSE locked_until END
             WHERE id = $1",
        )
        .bind(user.id)
        .execute(pool)
        .await?;
        return Err(AppError::BadRequest("Invalid credentials".into()));
    }

    sqlx::query(
        "UPDATE users SET failed_login_attempts = 0, locked_until = NULL, last_login_at = NOW() WHERE id = $1",
    )
    .bind(user.id)
    .execute(pool)
    .await?;

    Ok(user)
}

pub async fn get_by_id(pool: &PgPool, id: i32) -> AppResult<User> {
    sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}
