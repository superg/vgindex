use axum::http::{header, HeaderMap};
use chrono::{Duration, Utc};
use rand::RngCore;
use sqlx::PgPool;

use crate::config::Config;
use crate::db::models::{Session, UserRole};

pub const SESSION_COOKIE_NAME: &str = "session_id";
const SESSION_DURATION_DAYS: i64 = 14;
const GUEST_SESSION_DURATION_SECS: i64 = 86400;
const AUTH_SESSION_DURATION_SECS: i64 = SESSION_DURATION_DAYS * 86400;

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

pub fn generate_csrf_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn cookies_should_be_secure(config: &Config) -> bool {
    config.base_url.starts_with("https://") || config.site_url.starts_with("https://")
}

pub fn session_cookie(sid: &str, max_age_secs: i64, secure: bool) -> String {
    let mut cookie = format!(
        "{}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}",
        SESSION_COOKIE_NAME
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub fn login_session_cookie(sid: &str, config: &Config) -> String {
    session_cookie(
        sid,
        AUTH_SESSION_DURATION_SECS,
        cookies_should_be_secure(config),
    )
}

pub fn guest_session_cookie(sid: &str, config: &Config) -> String {
    session_cookie(
        sid,
        GUEST_SESSION_DURATION_SECS,
        cookies_should_be_secure(config),
    )
}

pub fn expired_session_cookie(config: &Config) -> String {
    session_cookie("", 0, cookies_should_be_secure(config))
}

pub async fn create_session(
    pool: &PgPool,
    user_id: i32,
    role: UserRole,
    ip: Option<&str>,
    ua: Option<&str>,
) -> Result<String, sqlx::Error> {
    let id = generate_session_id();
    let csrf_token = generate_csrf_token();
    let expires = Utc::now() + Duration::days(SESSION_DURATION_DAYS);

    sqlx::query(
        "INSERT INTO sessions (id, user_id, role, csrf_token, ip_address, user_agent, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(role)
    .bind(csrf_token)
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
    let csrf_token = generate_csrf_token();
    let expires = Utc::now() + Duration::days(1);

    sqlx::query(
        "INSERT INTO sessions (id, user_id, role, csrf_token, ip_address, user_agent, expires_at)
         VALUES ($1, NULL, NULL, $2, $3, $4, $5)",
    )
    .bind(&id)
    .bind(csrf_token)
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
    let mut session: Option<Session> =
        sqlx::query_as("SELECT * FROM sessions WHERE id = $1 AND expires_at > NOW()")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;

    if let Some(ref mut s) = session {
        let fallback_token = generate_csrf_token();
        let csrf_token: Option<String> = sqlx::query_scalar(
            "UPDATE sessions
             SET last_active_at = NOW(),
                 csrf_token = COALESCE(csrf_token, $2)
             WHERE id = $1
             RETURNING csrf_token",
        )
        .bind(&s.id)
        .bind(fallback_token)
        .fetch_optional(pool)
        .await?;
        s.csrf_token = csrf_token;
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
    use crate::config::Config;

    fn headers_with_cookie(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        headers
    }

    fn config(base_url: &str, site_url: &str) -> Config {
        Config {
            site_name: "localhost".to_string(),
            database_url: "postgres://postgres:postgres@localhost/postgres".to_string(),
            site_url: site_url.to_string(),
            base_url: base_url.to_string(),
            wiki_url: "#".to_string(),
            forum_url: "#".to_string(),
            news_feed_url: "#".to_string(),
            port: 0,
            oidc_provider_url: "#".to_string(),
            oidc_client_id: "test".to_string(),
            oidc_client_secret: "test".to_string(),
        }
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

    #[test]
    fn login_session_cookie_includes_secure_for_https_public_url() {
        let cookie =
            login_session_cookie("sid", &config("https://vgindex.test", "http://root.test"));

        assert!(cookie.contains("session_id=sid"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Secure"));
    }

    #[test]
    fn login_session_cookie_omits_secure_for_http_public_url() {
        let cookie =
            login_session_cookie("sid", &config("http://vgindex.test", "http://root.test"));

        assert!(!cookie.contains("Secure"));
    }

    #[test]
    fn expired_session_cookie_preserves_security_attributes() {
        let cookie = expired_session_cookie(&config("http://vgindex.test", "https://root.test"));

        assert!(cookie.contains("session_id="));
        assert!(cookie.contains("Max-Age=0"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Secure"));
    }
}
