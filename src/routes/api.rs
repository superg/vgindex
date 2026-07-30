use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};

use crate::auth::{csrf, middleware::RequireAuth};
use crate::db::models::UserRole;
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
) -> Result<Response, AppError> {
    csrf::verify_headers(&user, &headers)?;

    if !can_use_redumper_autofill(user.role, &req.log) {
        return Ok(unsupported_redumper_build_response());
    }

    let systems = disc_service::get_all_systems(&state.pool).await?;
    let system_codes: Vec<String> = systems.into_iter().map(|system| system.code).collect();
    Ok(Json(redumper_log::parse(&req.log, &system_codes)).into_response())
}

fn can_use_redumper_autofill(role: UserRole, log: &str) -> bool {
    role.can_moderate() || redumper_log::has_supported_autofill_builds(log)
}

const UNSUPPORTED_REDUMPER_BUILD_MESSAGE: &str =
    "Autofill is not supported for this version of redumper. Please update to the latest redumper build.";

fn unsupported_redumper_build_response() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(ParseRedumperLogError {
            error: "unsupported_redumper_build",
            message: UNSUPPORTED_REDUMPER_BUILD_MESSAGE,
        }),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct ParseRedumperLogRequest {
    log: String,
}

#[derive(serde::Serialize)]
struct ParseRedumperLogError {
    error: &'static str,
    message: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            handler.find("can_use_redumper_autofill").unwrap()
                < handler.find("get_all_systems").unwrap()
        );
    }

    #[test]
    fn moderators_bypass_the_redumper_build_requirement() {
        let unsupported = "redumper (build: LOCAL)";
        assert!(!can_use_redumper_autofill(UserRole::User, unsupported));
        assert!(!can_use_redumper_autofill(UserRole::UserPlus, unsupported));
        assert!(can_use_redumper_autofill(UserRole::Moderator, unsupported));
        assert!(can_use_redumper_autofill(UserRole::Admin, ""));
    }

    #[tokio::test]
    async fn unsupported_redumper_build_response_is_structured_422() {
        let response = unsupported_redumper_build_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "unsupported_redumper_build");
        assert_eq!(body["message"], UNSUPPORTED_REDUMPER_BUILD_MESSAGE);
    }
}
