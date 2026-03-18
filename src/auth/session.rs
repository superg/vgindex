use chrono::{Duration, Utc};
use rand::RngCore;
use sqlx::PgPool;

use crate::db::models::Session;

const SESSION_DURATION_DAYS: i64 = 14;

pub fn generate_session_id() -> String {
    let mut bytes = [0u8; 48];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub async fn create_session(
    pool: &PgPool,
    user_id: i32,
    ip: Option<&str>,
    ua: Option<&str>,
) -> Result<String, sqlx::Error> {
    let id = generate_session_id();
    let expires = Utc::now() + Duration::days(SESSION_DURATION_DAYS);

    sqlx::query(
        "INSERT INTO sessions (id, user_id, ip_address, user_agent, expires_at)
         VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&id)
    .bind(user_id)
    .bind(ip)
    .bind(ua)
    .bind(expires)
    .execute(pool)
    .await?;

    Ok(id)
}

pub async fn create_guest_session(
    pool: &PgPool,
    ip: Option<&str>,
    ua: Option<&str>,
) -> Result<String, sqlx::Error> {
    let id = generate_session_id();
    let expires = Utc::now() + Duration::days(1);

    sqlx::query(
        "INSERT INTO sessions (id, user_id, ip_address, user_agent, expires_at)
         VALUES ($1, NULL, $2, $3, $4)"
    )
    .bind(&id)
    .bind(ip)
    .bind(ua)
    .bind(expires)
    .execute(pool)
    .await?;

    Ok(id)
}

pub async fn validate_session(pool: &PgPool, session_id: &str) -> Result<Option<Session>, sqlx::Error> {
    let session: Option<Session> = sqlx::query_as(
        "SELECT * FROM sessions WHERE id = $1 AND expires_at > NOW()"
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;

    if let Some(ref s) = session {
        sqlx::query("UPDATE sessions SET last_active_at = NOW() WHERE id = $1")
            .bind(&s.id)
            .execute(pool)
            .await?;
    }

    Ok(session)
}

pub async fn delete_session(pool: &PgPool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn cleanup_expired(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM sessions WHERE expires_at < NOW()")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}
