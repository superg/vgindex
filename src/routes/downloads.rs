use askama::Template;
use axum::{
    extract::{Path, State},
    http::header,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};

use crate::auth::middleware::CurrentUser;
use crate::config::SiteConfig;
use crate::services::archive_service;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/downloads", get(downloads_page))
        .route("/downloads/", get(downloads_page))
        .route("/datfile/{system}", get(download_dat))
        .route("/cues/{system}", get(download_cue))
        .route("/sbi/{system}", get(download_sbi))
}

#[derive(Template)]
#[template(path = "downloads.html")]
struct DownloadsTemplate {
    current_user: Option<String>,
    systems: Vec<SystemDownload>,
}
impl SiteConfig for DownloadsTemplate {}

struct SystemDownload {
    code: String,
    name: String,
    has_dat: bool,
    has_cue: bool,
    has_sbi: bool,
}

async fn downloads_page(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Html<String> {
    let rows: Vec<SystemDownloadRow> = sqlx::query_as(
        "SELECT s.code, s.manufacturer, s.name, s.has_sbi,
                EXISTS(SELECT 1 FROM media_types mt
                       WHERE mt.code = ANY(s.media_types) AND mt.rom_extension = 'bin') AS has_cue
         FROM systems s
         ORDER BY LOWER(s.manufacturer), s.manufacturer, LOWER(s.name), s.name",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let systems = rows
        .into_iter()
        .map(|r| SystemDownload {
            name: crate::db::models::build_system_name(&r.manufacturer, &r.name),
            code: r.code,
            has_dat: true,
            has_cue: r.has_cue,
            has_sbi: r.has_sbi,
        })
        .collect();

    Html(
        DownloadsTemplate {
            current_user: user.user().map(|u| u.username.clone()),
            systems,
        }
        .render()
        .unwrap(),
    )
}

#[derive(sqlx::FromRow)]
struct SystemDownloadRow {
    code: String,
    manufacturer: String,
    name: String,
    has_sbi: bool,
    has_cue: bool,
}

async fn serve_archive(pool: &sqlx::PgPool, system: &str, archive_type: &str) -> Response {
    match archive_service::get_or_generate_archive(pool, system, archive_type).await {
        Ok(result) => (
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", result.filename),
                ),
            ],
            result.data,
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

async fn download_dat(
    State(state): State<AppState>,
    Path(system): Path<String>,
) -> Response {
    serve_archive(&state.pool, &system, "dat").await
}

async fn download_cue(
    State(state): State<AppState>,
    Path(system): Path<String>,
) -> Response {
    serve_archive(&state.pool, &system, "cue").await
}

async fn download_sbi(
    State(state): State<AppState>,
    Path(system): Path<String>,
) -> Response {
    serve_archive(&state.pool, &system, "sbi").await
}
