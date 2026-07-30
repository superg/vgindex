use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};

use crate::auth::{csrf, middleware::RequireAuth};
use crate::error::AppError;
use crate::services::{disc_service, redumper_log};
use crate::transliteration::{Script, TransliterationError};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/transliterate", post(transliterate))
        .route("/api/parse-redumper-log", post(parse_redumper_log))
}

/// Transliterate a non-Latin title into a Latin-script draft for the Main Title
/// field. Auth-gated: it's an editor helper.
async fn transliterate(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(req): Json<TransliterateRequest>,
) -> Result<Json<TransliterateResponse>, AppError> {
    csrf::verify_headers(&user, &headers)?;

    let result = state
        .transliteration
        .transliterate(&req.text, req.script)
        .map_err(|e| match e {
            // Client problems: empty, too long, or nothing transliterable.
            TransliterationError::EmptyInput
            | TransliterationError::TooLong
            | TransliterationError::UnsupportedScript => AppError::BadRequest(e.to_string()),
            TransliterationError::Backend(msg) => AppError::Internal(msg),
        })?;

    Ok(Json(TransliterateResponse {
        text: result.text,
        script: result.script,
        notes: result.notes,
    }))
}

#[derive(serde::Deserialize)]
struct TransliterateRequest {
    text: String,
    /// Optional explicit script; auto-detected when omitted.
    #[serde(default)]
    script: Option<Script>,
}

#[derive(serde::Serialize)]
struct TransliterateResponse {
    text: String,
    script: Script,
    notes: Vec<String>,
}

/// Parse a redumper log into an Add Disc form draft. Auth and CSRF checks match
/// the other editor helpers; parsing does not persist anything.
async fn parse_redumper_log(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(req): Json<ParseRedumperLogRequest>,
) -> Result<Json<redumper_log::ParsedRedumperLog>, AppError> {
    csrf::verify_headers(&user, &headers)?;

    let systems = disc_service::get_all_systems(&state.pool).await?;
    let system_codes: Vec<String> = systems.into_iter().map(|system| system.code).collect();
    Ok(Json(redumper_log::parse(&req.log, &system_codes)))
}

#[derive(serde::Deserialize)]
struct ParseRedumperLogRequest {
    log: String,
}

#[cfg(test)]
mod tests {
    #[test]
    fn redumper_parser_route_is_authenticated_and_csrf_protected() {
        let source = include_str!("api.rs");

        assert!(source.contains(r#".route("/api/parse-redumper-log", post(parse_redumper_log))"#));
        let handler = source
            .split_once("async fn parse_redumper_log")
            .unwrap()
            .1
            .split_once("#[derive(serde::Deserialize)]")
            .unwrap()
            .0;
        assert!(handler.contains("RequireAuth(user): RequireAuth"));
        assert!(handler.contains("csrf::verify_headers(&user, &headers)?"));
        assert!(handler.contains("redumper_log::parse(&req.log, &system_codes)"));
    }
}
