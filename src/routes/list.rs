use axum::{
    extract::{Path, State},
    http::header,
    response::IntoResponse,
    routing::get,
    Router,
};
use std::collections::HashMap;

use crate::auth::middleware::{AuthenticatedUser, RequireAuth};
use crate::db::models::sanitize_filename;
use crate::error::{AppError, AppResult};
use crate::services::{archive_service, disc_service};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/list/have/{username}", get(list_have))
        .route("/list/have/{username}/{syscode}", get(list_have))
        .route("/list/cues/{username}", get(list_cues))
        .route("/list/cues/{username}/{syscode}", get(list_cues))
        .route("/list/datfile/{username}", get(list_datfile))
        .route("/list/datfile/{username}/{syscode}", get(list_datfile))
}

fn ensure_self(user: &AuthenticatedUser, username: &str) -> AppResult<()> {
    if user.username == username {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn list_have(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(params): Path<HashMap<String, String>>,
) -> AppResult<impl IntoResponse> {
    ensure_self(&user, &params["username"])?;
    let syscode = params.get("syscode").map(|code| code.trim().to_ascii_uppercase());
    let discs = disc_service::list_dumped_discs(
        &state.pool,
        user.id,
        syscode.as_deref(),
        false,
    )
    .await?;
    let body: String = discs.iter().map(|(_, name, _)| format!("{name}\n")).collect();
    Ok((
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        body,
    ))
}

async fn list_cues(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(params): Path<HashMap<String, String>>,
) -> AppResult<impl IntoResponse> {
    ensure_self(&user, &params["username"])?;
    let syscode = params.get("syscode").map(|code| code.trim().to_ascii_uppercase());
    let files: Vec<(String, String)> = disc_service::list_dumped_discs(
        &state.pool,
        user.id,
        syscode.as_deref(),
        true,
    )
    .await?
    .into_iter()
    .filter_map(|(system, name, cue)| {
        let path = if syscode.is_some() { format!("{name}.cue") } else { format!("{system}/{name}.cue") };
        Some((path, cue?))
    })
    .collect();
    let system_name = match syscode.as_deref() {
        Some(code) => format!(" - {}", disc_service::get_system(&state.pool, code).await?.dat_system_name()),
        None => String::new(),
    };
    let disposition = format!(
        "attachment; filename=\"{}{system_name} - Cuesheets ({}) ({}).zip\"",
        sanitize_filename(&user.username),
        files.len(),
        archive_service::timestamp_now()
    );
    let zip = tokio::task::spawn_blocking(move || archive_service::zip_text_files(&files))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
        ],
        zip,
    ))
}

async fn list_datfile(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(params): Path<HashMap<String, String>>,
) -> AppResult<impl IntoResponse> {
    ensure_self(&user, &params["username"])?;
    let syscode = params.get("syscode").map(|code| code.trim().to_ascii_uppercase());
    let metadata = archive_service::ArchiveMetadata::from_site_url(&state.config.site_url);
    let archive = archive_service::generate_user_datfile(
        &state.pool,
        &metadata,
        user.id,
        &user.username,
        syscode.as_deref(),
    )
    .await?;
    let filename = archive.filename;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
        ],
        archive.data,
    ))
}
