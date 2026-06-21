use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};

use crate::auth::{csrf, middleware::RequireAuth};
use crate::error::AppError;
use crate::transliteration::{Script, TransliterationError};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/transliterate", post(transliterate))
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
