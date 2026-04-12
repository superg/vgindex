use std::{collections::HashMap, sync::Arc};

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header as JwtHeader};
use rand::RngCore;
use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts, RsaPrivateKey, RsaPublicKey};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::middleware::CurrentUser;
use crate::auth::session::generate_session_id;
use crate::AppState;

// ---------------------------------------------------------------------------
// OidcProvider — holds RSA signing key generated at startup
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct OidcProvider {
    encoding_key: Arc<EncodingKey>,
    kid: String,
    jwks_json: Value,
}

impl OidcProvider {
    pub fn new() -> Self {
        tracing::info!("Generating OIDC RSA-2048 signing key (slow in debug builds)…");
        let mut rng = rand::thread_rng();
        let private_key =
            RsaPrivateKey::new(&mut rng, 2048).expect("failed to generate OIDC RSA key");
        let public_key = RsaPublicKey::from(&private_key);

        let pem = private_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .expect("failed to encode RSA private key to PEM");

        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes())
            .expect("failed to create JWT encoding key from RSA PEM");

        let n_b64 = URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
        let e_b64 = URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

        let mut kid_bytes = [0u8; 8];
        rng.fill_bytes(&mut kid_bytes);
        let kid = hex::encode(kid_bytes);

        let jwks_json = json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "alg": "RS256",
                "kid": &kid,
                "n": n_b64,
                "e": e_b64,
            }]
        });

        tracing::info!("OIDC signing key ready (kid={kid})");

        Self {
            encoding_key: Arc::new(encoding_key),
            kid,
            jwks_json,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal DB row helpers
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct AuthCodeRow {
    user_id: i32,
    redirect_uri: String,
    scope: String,
    nonce: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AccessTokenRow {
    user_id: i32,
}

#[derive(sqlx::FromRow)]
struct OidcUserRow {
    id: i32,
    username: String,
    email: String,
    role: crate::db::models::UserRole,
}

// ---------------------------------------------------------------------------
// Request / response structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AuthorizeParams {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    scope: Option<String>,
    state: Option<String>,
    nonce: Option<String>,
}

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: String,
    redirect_uri: String,
    client_id: Option<String>,
    client_secret: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn openid_config(State(state): State<AppState>) -> Json<Value> {
    let issuer = &state.config.oidc_issuer_url;
    let public = &state.config.base_url;
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{public}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "userinfo_endpoint": format!("{issuer}/oauth/userinfo"),
        "jwks_uri": format!("{issuer}/oauth/jwks"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "scopes_supported": ["openid", "profile", "email"],
        "claims_supported": ["sub", "preferred_username", "email", "role"],
        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
    }))
}

async fn jwks(State(state): State<AppState>) -> Json<Value> {
    Json(state.oidc.jwks_json.clone())
}

async fn authorize(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    if params.response_type != "code" {
        return (StatusCode::BAD_REQUEST, "unsupported response_type").into_response();
    }

    let client = match sqlx::query_as::<_, crate::db::models::OAuthClient>(
        "SELECT * FROM oauth_clients WHERE client_id = $1",
    )
    .bind(&params.client_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(c)) => c,
        _ => return (StatusCode::BAD_REQUEST, "invalid client_id").into_response(),
    };

    if !redirect_uri_matches(&client.redirect_uri, &params.redirect_uri) {
        return (StatusCode::BAD_REQUEST, "redirect_uri mismatch").into_response();
    }

    let authenticated = match user.user() {
        Some(u) => u,
        None => {
            let mut parts = vec![
                format!("client_id={}", urlencoding::encode(&params.client_id)),
                format!(
                    "redirect_uri={}",
                    urlencoding::encode(&params.redirect_uri)
                ),
                format!(
                    "response_type={}",
                    urlencoding::encode(&params.response_type)
                ),
            ];
            if let Some(ref s) = params.scope {
                parts.push(format!("scope={}", urlencoding::encode(s)));
            }
            if let Some(ref s) = params.state {
                parts.push(format!("state={}", urlencoding::encode(s)));
            }
            if let Some(ref n) = params.nonce {
                parts.push(format!("nonce={}", urlencoding::encode(n)));
            }
            let authorize_path = format!("/oauth/authorize?{}", parts.join("&"));
            let login_url =
                format!("/login?return_to={}", urlencoding::encode(&authorize_path));
            return Redirect::to(&login_url).into_response();
        }
    };

    let code = generate_session_id();
    let expires = Utc::now() + Duration::minutes(5);

    if let Err(e) = sqlx::query(
        "INSERT INTO oauth_authorization_codes \
             (code, client_id, user_id, redirect_uri, scope, nonce, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&code)
    .bind(&params.client_id)
    .bind(authenticated.id)
    .bind(&params.redirect_uri)
    .bind(params.scope.as_deref().unwrap_or("openid"))
    .bind(params.nonce.as_deref())
    .bind(expires)
    .execute(&state.pool)
    .await
    {
        tracing::error!("Failed to store auth code: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
    }

    let sep = if params.redirect_uri.contains('?') { "&" } else { "?" };
    let mut callback = format!(
        "{}{}code={}",
        params.redirect_uri,
        sep,
        urlencoding::encode(&code)
    );
    if let Some(ref st) = params.state {
        callback.push_str(&format!("&state={}", urlencoding::encode(st)));
    }

    Redirect::to(&callback).into_response()
}

async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(params): Form<TokenRequest>,
) -> Response {
    if params.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "unsupported_grant_type"})),
        )
            .into_response();
    }

    let (client_id, client_secret) =
        if let (Some(id), Some(secret)) = (params.client_id.as_ref(), params.client_secret.as_ref())
        {
            (id.clone(), secret.clone())
        } else if let Some(basic) = extract_basic_auth(&headers) {
            basic
        } else {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid_client"})),
            )
                .into_response();
        };

    let client_ok = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM oauth_clients WHERE client_id = $1 AND client_secret = $2)",
    )
    .bind(&client_id)
    .bind(&client_secret)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);

    if !client_ok {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_client"})),
        )
            .into_response();
    }

    let auth_code = match sqlx::query_as::<_, AuthCodeRow>(
        "DELETE FROM oauth_authorization_codes \
         WHERE code = $1 AND client_id = $2 AND expires_at > NOW() \
         RETURNING user_id, redirect_uri, scope, nonce",
    )
    .bind(&params.code)
    .bind(&client_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(row)) => row,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    };

    if auth_code.redirect_uri != params.redirect_uri {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }

    let user = match sqlx::query_as::<_, OidcUserRow>(
        "SELECT id, username, email, role FROM users WHERE id = $1 AND is_active = true",
    )
    .bind(auth_code.user_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(u)) => u,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    };

    let access_token = generate_session_id();
    let ttl_secs: i64 = 3600;
    let access_expires = Utc::now() + Duration::seconds(ttl_secs);

    let _ = sqlx::query(
        "INSERT INTO oauth_access_tokens (token, user_id, client_id, scope, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&access_token)
    .bind(user.id)
    .bind(&client_id)
    .bind(&auth_code.scope)
    .bind(access_expires)
    .execute(&state.pool)
    .await;

    let now = Utc::now();
    let mut claims = json!({
        "iss": &state.config.oidc_issuer_url,
        "sub": user.id.to_string(),
        "aud": &client_id,
        "exp": (now + Duration::seconds(ttl_secs)).timestamp(),
        "iat": now.timestamp(),
        "preferred_username": &user.username,
        "email": &user.email,
        "role": user.role.to_string(),
    });
    if let Some(nonce) = &auth_code.nonce {
        claims["nonce"] = json!(nonce);
    }

    let mut jwt_header = JwtHeader::new(Algorithm::RS256);
    jwt_header.kid = Some(state.oidc.kid.clone());

    let id_token = match encode(&jwt_header, &claims, &state.oidc.encoding_key) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to encode id_token: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "server_error"})),
            )
                .into_response();
        }
    };

    Json(json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": ttl_secs,
        "id_token": id_token,
        "scope": auth_code.scope,
    }))
    .into_response()
}

async fn userinfo(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token_str = match headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
    {
        Some(t) => t.to_string(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid_token"})),
            )
                .into_response();
        }
    };

    let row = match sqlx::query_as::<_, AccessTokenRow>(
        "SELECT user_id FROM oauth_access_tokens WHERE token = $1 AND expires_at > NOW()",
    )
    .bind(&token_str)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(r)) => r,
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid_token"})),
            )
                .into_response();
        }
    };

    let user = match sqlx::query_as::<_, OidcUserRow>(
        "SELECT id, username, email, role FROM users WHERE id = $1",
    )
    .bind(row.user_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(Some(u)) => u,
        _ => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "server_error"})),
            )
                .into_response();
        }
    };

    Json(json!({
        "sub": user.id.to_string(),
        "preferred_username": &user.username,
        "email": &user.email,
        "role": user.role.to_string(),
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_basic_auth(headers: &HeaderMap) -> Option<(String, String)> {
    let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = auth.strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    Some((id.to_string(), secret.to_string()))
}

fn redirect_uri_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }

    let (registered_base, registered_query) = match registered.split_once('?') {
        Some(parts) => parts,
        None => return false,
    };
    let (requested_base, requested_query) = match requested.split_once('?') {
        Some(parts) => parts,
        None => return false,
    };

    if registered_base != requested_base {
        return false;
    }

    let registered_params = parse_query_params(registered_query);
    let requested_params = parse_query_params(requested_query);

    registered_params.iter().all(|(key, registered_values)| {
        requested_params
            .get(key)
            .map(|requested_values| {
                registered_values
                    .iter()
                    .all(|value| requested_values.contains(value))
            })
            .unwrap_or(false)
    })
}

fn parse_query_params(query: &str) -> HashMap<String, Vec<String>> {
    let mut params = HashMap::new();

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params
            .entry(key.to_string())
            .or_insert_with(Vec::new)
            .push(value.to_string());
    }

    params
}

#[cfg(test)]
mod tests {
    use super::redirect_uri_matches;

    #[test]
    fn redirect_uri_exact_match_still_passes() {
        assert!(redirect_uri_matches(
            "https://forum.localhost:8443/ucp.php?mode=login",
            "https://forum.localhost:8443/ucp.php?mode=login"
        ));
    }

    #[test]
    fn redirect_uri_with_extra_query_params_passes() {
        assert!(redirect_uri_matches(
            "https://forum.localhost:8443/ucp.php?mode=login",
            "https://forum.localhost:8443/ucp.php?mode=login&login=external&oauth_service=vgindex"
        ));
    }

    #[test]
    fn redirect_uri_with_different_base_fails() {
        assert!(!redirect_uri_matches(
            "https://forum.localhost:8443/ucp.php?mode=login",
            "https://evil.localhost:8443/ucp.php?mode=login&login=external"
        ));
    }

    #[test]
    fn redirect_uri_missing_registered_params_fails() {
        assert!(!redirect_uri_matches(
            "https://forum.localhost:8443/ucp.php?mode=login&oauth_service=vgindex",
            "https://forum.localhost:8443/ucp.php?mode=login"
        ));
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/.well-known/openid-configuration", get(openid_config))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/token", post(token))
        .route("/oauth/userinfo", get(userinfo))
        .route("/oauth/jwks", get(jwks))
}
