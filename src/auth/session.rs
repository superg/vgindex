use axum::http::{header, HeaderMap};
use rand::RngCore;
use sqlx::PgPool;

use crate::config::Config;
use crate::db::models::{Session, UserRole};

pub const SESSION_COOKIE_NAME: &str = "session_id";
pub const SSO_CHECK_COOKIE_NAME: &str = "forum_sso_checked";
pub const SSO_CHECK_TTL_SECS: i64 = 15 * 60;
const ABANDONED_SESSION_DAYS: i64 = 30;

fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(value) = part
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(value.to_string());
        }
    }
    None
}

pub fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, SESSION_COOKIE_NAME)
}

pub fn has_sso_check_cookie(headers: &HeaderMap) -> bool {
    extract_cookie(headers, SSO_CHECK_COOKIE_NAME).is_some()
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

fn cookie(name: &str, value: &str, max_age_secs: Option<i64>, secure: bool) -> String {
    let mut cookie = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax");
    if let Some(max_age_secs) = max_age_secs {
        cookie.push_str(&format!("; Max-Age={max_age_secs}"));
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub fn login_session_cookie(sid: &str, config: &Config) -> String {
    cookie(
        SESSION_COOKIE_NAME,
        sid,
        None,
        cookies_should_be_secure(config),
    )
}

pub fn expired_session_cookie(config: &Config) -> String {
    cookie(
        SESSION_COOKIE_NAME,
        "",
        Some(0),
        cookies_should_be_secure(config),
    )
}

pub fn checked_sso_cookie(config: &Config) -> String {
    cookie(
        SSO_CHECK_COOKIE_NAME,
        "1",
        Some(SSO_CHECK_TTL_SECS),
        cookies_should_be_secure(config),
    )
}

pub fn suppress_automatic_sso_cookie(config: &Config) -> String {
    cookie(
        SSO_CHECK_COOKIE_NAME,
        "1",
        None,
        cookies_should_be_secure(config),
    )
}

pub fn expired_sso_check_cookie(config: &Config) -> String {
    cookie(
        SSO_CHECK_COOKIE_NAME,
        "",
        Some(0),
        cookies_should_be_secure(config),
    )
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

    sqlx::query(
        "INSERT INTO sessions (id, user_id, role, csrf_token, ip_address, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&id)
    .bind(user_id)
    .bind(role)
    .bind(csrf_token)
    .bind(ip)
    .bind(ua)
    .execute(pool)
    .await?;

    Ok(id)
}

pub async fn validate_session(
    pool: &PgPool,
    session_id: &str,
) -> Result<Option<Session>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE sessions
         SET last_active_at = NOW(),
             csrf_token = COALESCE(csrf_token, $2)
         WHERE id = $1
         RETURNING *",
    )
    .bind(session_id)
    .bind(generate_csrf_token())
    .fetch_optional(pool)
    .await
}

pub async fn mark_revalidation_attempt(pool: &PgPool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE sessions
         SET oidc_revalidation_attempted_at = NOW()
         WHERE id = $1",
    )
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_session(pool: &PgPool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn cleanup_abandoned(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM sessions
         WHERE last_active_at < NOW() - make_interval(days => $1::int)",
    )
    .bind(ABANDONED_SESSION_DAYS)
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
        assert!(!cookie.contains("Max-Age"));
    }

    #[test]
    fn login_session_cookie_omits_secure_for_http_public_url() {
        let cookie =
            login_session_cookie("sid", &config("http://vgindex.test", "http://root.test"));

        assert!(!cookie.contains("Secure"));
        assert!(!cookie.contains("Max-Age"));
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

    #[test]
    fn checked_sso_cookie_is_short_lived_and_non_identifying() {
        let cookie = checked_sso_cookie(&config("https://vgindex.test", "#"));

        assert!(cookie.contains("forum_sso_checked=1"));
        assert!(cookie.contains("Max-Age=900"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
    }

    #[test]
    fn sso_suppression_cookie_lasts_for_the_browser_session() {
        let cookie = suppress_automatic_sso_cookie(&config("https://vgindex.test", "#"));

        assert!(cookie.contains("forum_sso_checked=1"));
        assert!(!cookie.contains("Max-Age"));
    }
}
