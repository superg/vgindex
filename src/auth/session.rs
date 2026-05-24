use axum::http::{header, HeaderMap};
use chrono::{Duration, Utc};
use rand::RngCore;
use sqlx::PgPool;

use crate::db::models::Session;

pub const SESSION_COOKIE_NAME: &str = "session_id";
const SESSION_DURATION_DAYS: i64 = 14;

pub fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part
            .strip_prefix(SESSION_COOKIE_NAME)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(value.to_string());
        }
    }
    None
}

pub fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .map(ToOwned::to_owned)
}

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
         VALUES ($1, $2, $3, $4, $5)",
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
         VALUES ($1, NULL, $2, $3, $4)",
    )
    .bind(&id)
    .bind(ip)
    .bind(ua)
    .bind(expires)
    .execute(pool)
    .await?;

    Ok(id)
}

pub async fn validate_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<Option<Session>, sqlx::Error> {
    let session: Option<Session> =
        sqlx::query_as("SELECT * FROM sessions WHERE id = $1 AND expires_at > NOW()")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_cookie(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        headers
    }

    #[test]
    fn extract_session_cookie_returns_none_without_cookie_header() {
        assert_eq!(extract_session_cookie(&HeaderMap::new()), None);
    }

    #[test]
    fn extract_session_cookie_finds_session_among_multiple_cookies() {
        let headers = headers_with_cookie("theme=dark; session_id=abc123; locale=en");
        assert_eq!(extract_session_cookie(&headers), Some("abc123".to_string()));
    }

    #[test]
    fn extract_session_cookie_handles_whitespace_around_parts() {
        let headers = headers_with_cookie(" theme=dark ;   session_id=guest-session  ; locale=en ");
        assert_eq!(
            extract_session_cookie(&headers),
            Some("guest-session".to_string())
        );
    }

    #[test]
    fn extract_client_ip_uses_first_forwarded_for_value() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.1, 10.0.0.1".parse().unwrap());

        assert_eq!(extract_client_ip(&headers), Some("203.0.113.1".to_string()));
    }

    #[test]
    fn extract_client_ip_falls_back_to_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "203.0.113.2".parse().unwrap());

        assert_eq!(extract_client_ip(&headers), Some("203.0.113.2".to_string()));
    }

    #[test]
    fn extract_client_ip_returns_none_without_ip_headers() {
        assert_eq!(extract_client_ip(&HeaderMap::new()), None);
    }
}
