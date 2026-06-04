use axum::{
    extract::State,
    response::Html,
    routing::{get, post},
    Json, Router,
};

use crate::auth::middleware::{CurrentUser, RequireAuth};
use crate::error::AppError;
use crate::transliteration::{Script, TransliterationError};
use crate::AppState;

// Online means sessions active in this window: registered users are deduped by
// user_id, while guests are deduped by anonymous session IP address.
const ACTIVE_WINDOW_MINUTES: i32 = 2;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/online", get(online_users))
        .route("/api/online-html", get(online_users_html))
        .route("/api/transliterate", post(transliterate))
}

/// Transliterate a non-Latin title into a Latin-script draft for the Main Title
/// field. Auth-gated: it's an editor helper.
async fn transliterate(
    State(state): State<AppState>,
    _user: RequireAuth,
    Json(req): Json<TransliterateRequest>,
) -> Result<Json<TransliterateResponse>, AppError> {
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

async fn online_users(State(state): State<AppState>) -> Json<OnlineInfo> {
    let (registered, guests) = fetch_online_counts(&state).await;

    let usernames: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT u.username FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.user_id IS NOT NULL
           AND s.expires_at > NOW()
           AND s.last_active_at > NOW() - ($1 * INTERVAL '1 minute')
         ORDER BY u.username",
    )
    .bind(ACTIVE_WINDOW_MINUTES)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    Json(OnlineInfo {
        registered,
        guests,
        usernames,
    })
}

async fn online_users_html(State(state): State<AppState>, _user: CurrentUser) -> Html<String> {
    let (registered, guests) = fetch_online_counts(&state).await;

    let html = format!("Users: {registered}, Guests: {guests}");

    Html(html)
}

async fn fetch_online_counts(state: &AppState) -> (i64, i64) {
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT
             COUNT(DISTINCT user_id) FILTER (WHERE user_id IS NOT NULL) AS registered,
             COUNT(DISTINCT ip_address) FILTER (
                 WHERE user_id IS NULL AND ip_address IS NOT NULL
             ) AS guests
         FROM sessions
         WHERE expires_at > NOW()
           AND last_active_at > NOW() - ($1 * INTERVAL '1 minute')",
    )
    .bind(ACTIVE_WINDOW_MINUTES)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0, 0))
}

#[derive(serde::Serialize)]
struct OnlineInfo {
    registered: i64,
    guests: i64,
    usernames: Vec<String>,
}
