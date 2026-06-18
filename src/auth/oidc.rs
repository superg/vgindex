use axum::{
    extract::{Query, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::auth::middleware::CurrentUser;
use crate::auth::session;
use crate::db::models::UserRole;
use crate::error::{AppError, AppResult};
use crate::AppState;

const LOGIN_STATE_TTL_MINUTES: i64 = 10;
const OIDC_SCOPE: &str = "openid profile email";

#[derive(Deserialize)]
struct LoginQuery {
    return_to: Option<String>,
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Serialize)]
struct TokenForm<'a> {
    grant_type: &'static str,
    code: &'a str,
    redirect_uri: &'a str,
    client_id: &'a str,
    client_secret: &'a str,
    code_verifier: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    alg: Option<String>,
    n: String,
    e: String,
}

#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    preferred_username: String,
    role: UserRole,
    picture: Option<String>,
    nonce: Option<String>,
}

#[derive(sqlx::FromRow)]
struct LoginStateRow {
    nonce: String,
    pkce_verifier: String,
    return_to: String,
}

#[derive(sqlx::FromRow)]
struct UserIdRow {
    id: i32,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/login", get(login))
        .route("/auth/oidc/callback", get(callback))
}

async fn login(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<LoginQuery>,
) -> AppResult<Response> {
    let return_to = sanitize_return_to(query.return_to.as_deref());
    if user.is_logged_in() {
        return Ok(Redirect::to(&return_to).into_response());
    }

    let discovery = fetch_discovery(&state).await?;
    let state_token = random_url_token(32);
    let nonce = random_url_token(32);
    let pkce_verifier = random_url_token(48);
    let code_challenge = pkce_s256(&pkce_verifier);

    store_login_state(
        &state.pool,
        &state_token,
        &nonce,
        &pkce_verifier,
        &return_to,
    )
    .await?;

    let redirect_uri = callback_url(&state);
    let auth_url = with_query(
        &discovery.authorization_endpoint,
        &[
            ("response_type", "code".to_string()),
            ("client_id", state.config.oidc_client_id.clone()),
            ("redirect_uri", redirect_uri),
            ("scope", OIDC_SCOPE.to_string()),
            ("state", state_token),
            ("nonce", nonce),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256".to_string()),
        ],
    );

    Ok(Redirect::to(&auth_url).into_response())
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> AppResult<Response> {
    if let Some(error) = query.error {
        let description = query.error_description.unwrap_or_default();
        let suffix = if description.is_empty() {
            String::new()
        } else {
            format!(": {description}")
        };
        return Err(AppError::BadRequest(format!(
            "OIDC authorization failed: {error}{suffix}"
        )));
    }

    let code = query
        .code
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Missing OIDC authorization code".into()))?;
    let state_token = query
        .state
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("Missing OIDC state".into()))?;

    let login_state = consume_login_state(&state.pool, state_token).await?;
    let discovery = fetch_discovery(&state).await?;
    let token = exchange_code(&state, &discovery, code, &login_state.pkce_verifier).await?;
    let claims = validate_id_token(&state, &discovery, &token.id_token, &login_state.nonce).await?;
    let avatar_url = sanitize_picture_url(claims.picture.as_deref());
    let user_id = upsert_user(
        &state.pool,
        &claims.preferred_username,
        avatar_url.as_deref(),
    )
    .await?;

    if let Some(existing_sid) = session::extract_session_cookie(&headers) {
        if let Err(e) = session::delete_session(&state.pool, &existing_sid).await {
            tracing::warn!("Failed to replace previous session during OIDC login: {e}");
        }
    }

    let ip = session::extract_client_ip(&headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let sid = session::create_session(
        &state.pool,
        user_id,
        claims.role,
        ip.as_deref(),
        ua.as_deref(),
    )
    .await?;

    let cookie = format!(
        "{}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        session::SESSION_COOKIE_NAME,
        14 * 86400
    );
    let mut response = Redirect::to(&login_state.return_to).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie.parse().unwrap());
    Ok(response)
}

async fn fetch_discovery(state: &AppState) -> AppResult<Discovery> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        state.config.oidc_provider_url.trim_end_matches('/')
    );
    state
        .http
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("OIDC discovery request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Internal(format!("OIDC discovery returned an error: {e}")))?
        .json::<Discovery>()
        .await
        .map_err(|e| AppError::Internal(format!("OIDC discovery parse failed: {e}")))
}

async fn exchange_code(
    state: &AppState,
    discovery: &Discovery,
    code: &str,
    pkce_verifier: &str,
) -> AppResult<TokenResponse> {
    let redirect_uri = callback_url(state);
    let form = TokenForm {
        grant_type: "authorization_code",
        code,
        redirect_uri: &redirect_uri,
        client_id: &state.config.oidc_client_id,
        client_secret: &state.config.oidc_client_secret,
        code_verifier: pkce_verifier,
    };

    state
        .http
        .post(&discovery.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("OIDC token request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::BadRequest(format!("OIDC token exchange failed: {e}")))?
        .json::<TokenResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("OIDC token response parse failed: {e}")))
}

async fn validate_id_token(
    state: &AppState,
    discovery: &Discovery,
    id_token: &str,
    expected_nonce: &str,
) -> AppResult<IdTokenClaims> {
    let header = decode_header(id_token)
        .map_err(|e| AppError::BadRequest(format!("Invalid OIDC ID token header: {e}")))?;
    if header.alg != Algorithm::RS256 {
        return Err(AppError::BadRequest("OIDC ID token must use RS256".into()));
    }
    let kid = header
        .kid
        .ok_or_else(|| AppError::BadRequest("OIDC ID token is missing kid".into()))?;

    let jwks = fetch_jwks(state, &discovery.jwks_uri).await?;
    let key = jwks
        .keys
        .iter()
        .find(|key| {
            key.kid.as_deref() == Some(kid.as_str())
                && key.kty == "RSA"
                && key.alg.as_deref().unwrap_or("RS256") == "RS256"
        })
        .ok_or_else(|| AppError::BadRequest("OIDC signing key not found".into()))?;
    let decoding_key = DecodingKey::from_rsa_components(&key.n, &key.e)
        .map_err(|e| AppError::BadRequest(format!("Invalid OIDC signing key: {e}")))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[state.config.oidc_client_id.as_str()]);
    validation.set_issuer(&[discovery.issuer.as_str()]);

    let token = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
        .map_err(|e| AppError::BadRequest(format!("Invalid OIDC ID token: {e}")))?;
    let claims = token.claims;
    if claims.sub.trim().is_empty() {
        return Err(AppError::BadRequest("OIDC ID token is missing sub".into()));
    }
    if claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(AppError::BadRequest("OIDC nonce mismatch".into()));
    }
    if claims.preferred_username.trim().is_empty() {
        return Err(AppError::BadRequest(
            "OIDC preferred_username is missing".into(),
        ));
    }
    Ok(claims)
}

async fn fetch_jwks(state: &AppState, jwks_uri: &str) -> AppResult<Jwks> {
    state
        .http
        .get(jwks_uri)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("OIDC JWKS request failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::Internal(format!("OIDC JWKS returned an error: {e}")))?
        .json::<Jwks>()
        .await
        .map_err(|e| AppError::Internal(format!("OIDC JWKS parse failed: {e}")))
}

async fn upsert_user(pool: &PgPool, username: &str, avatar_url: Option<&str>) -> AppResult<i32> {
    let username = username.trim();
    if username.is_empty() || username.chars().count() > 64 {
        return Err(AppError::BadRequest(
            "phpBB username is not valid for the app".into(),
        ));
    }

    let row: UserIdRow = sqlx::query_as(
        "INSERT INTO users (username, avatar_url)
         VALUES ($1, $2)
         ON CONFLICT (username) DO UPDATE SET
             username = EXCLUDED.username,
             avatar_url = EXCLUDED.avatar_url
         RETURNING id",
    )
    .bind(username)
    .bind(avatar_url)
    .fetch_one(pool)
    .await?;
    Ok(row.id)
}

fn sanitize_picture_url(picture: Option<&str>) -> Option<String> {
    let picture = picture?.trim();
    if picture.is_empty()
        || picture.len() > 2048
        || picture.chars().any(char::is_control)
        || !(picture.starts_with("http://") || picture.starts_with("https://"))
    {
        return None;
    }
    Some(picture.to_string())
}

async fn store_login_state(
    pool: &PgPool,
    state_token: &str,
    nonce: &str,
    pkce_verifier: &str,
    return_to: &str,
) -> Result<(), sqlx::Error> {
    let expires = Utc::now() + Duration::minutes(LOGIN_STATE_TTL_MINUTES);
    sqlx::query(
        "INSERT INTO oidc_login_states
             (state_hash, nonce, pkce_verifier, return_to, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(hash_token(state_token))
    .bind(nonce)
    .bind(pkce_verifier)
    .bind(return_to)
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(())
}

async fn consume_login_state(pool: &PgPool, state_token: &str) -> AppResult<LoginStateRow> {
    sqlx::query_as(
        "DELETE FROM oidc_login_states
         WHERE state_hash = $1 AND expires_at > NOW()
         RETURNING nonce, pkce_verifier, return_to",
    )
    .bind(hash_token(state_token))
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::BadRequest("OIDC login state expired or was already used".into()))
}

pub async fn cleanup_expired_login_states(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM oidc_login_states WHERE expires_at < NOW()")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

fn callback_url(state: &AppState) -> String {
    format!(
        "{}/auth/oidc/callback",
        state.config.base_url.trim_end_matches('/')
    )
}

fn random_url_token(byte_count: usize) -> String {
    let mut bytes = vec![0u8; byte_count];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn pkce_s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn sanitize_return_to(return_to: Option<&str>) -> String {
    let Some(return_to) = return_to else {
        return "/".to_string();
    };
    if return_to.starts_with('/')
        && !return_to.starts_with("//")
        && !return_to.contains('\r')
        && !return_to.contains('\n')
    {
        return crate::routes::canonicalize_root_relative_url(return_to);
    }
    "/".to_string()
}

fn with_query(base: &str, params: &[(&str, String)]) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };
    let query = params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}{separator}{query}")
}

#[cfg(test)]
mod tests {
    use super::{hash_token, pkce_s256, sanitize_picture_url, sanitize_return_to, with_query};

    #[test]
    fn sanitize_return_to_accepts_root_relative_paths() {
        assert_eq!(
            sanitize_return_to(Some("/queue?status=Pending")),
            "/queue?status=Pending"
        );
        assert_eq!(
            sanitize_return_to(Some("/DISCS/?System=PS3")),
            "/discs?System=PS3"
        );
    }

    #[test]
    fn sanitize_return_to_rejects_external_or_header_like_values() {
        assert_eq!(sanitize_return_to(Some("https://example.com")), "/");
        assert_eq!(sanitize_return_to(Some("//example.com/path")), "/");
        assert_eq!(sanitize_return_to(Some("/ok\r\nSet-Cookie: nope")), "/");
        assert_eq!(sanitize_return_to(None), "/");
    }

    #[test]
    fn sanitize_picture_url_accepts_only_absolute_http_urls() {
        assert_eq!(
            sanitize_picture_url(Some(" https://forum.example/avatar.png ")).as_deref(),
            Some("https://forum.example/avatar.png")
        );
        assert_eq!(sanitize_picture_url(Some("/avatar.png")), None);
        assert_eq!(sanitize_picture_url(Some("javascript:alert(1)")), None);
        assert_eq!(
            sanitize_picture_url(Some("https://example.test/a\nb")),
            None
        );
        assert_eq!(sanitize_picture_url(None), None);
    }

    #[test]
    fn pkce_s256_matches_rfc7636_vector() {
        assert_eq!(
            pkce_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn hash_token_is_stable_sha256_hex() {
        assert_eq!(
            hash_token("state"),
            "4ba69735ca53765ed6a709edb56c6ea236b7193a3b29a6b390c346f0f4340e4e"
        );
    }

    #[test]
    fn with_query_appends_and_encodes_params() {
        assert_eq!(
            with_query(
                "https://forum.example/authorize",
                &[("scope", "openid profile".into())]
            ),
            "https://forum.example/authorize?scope=openid%20profile"
        );
        assert_eq!(
            with_query(
                "https://forum.example/authorize?x=1",
                &[("state", "a+b".into())]
            ),
            "https://forum.example/authorize?x=1&state=a%2Bb"
        );
    }
}
