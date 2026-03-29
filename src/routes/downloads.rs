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
        .route("/downloads/{system}/{archive_type}", get(download_archive))
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
        "SELECT s.code, s.name, s.has_sbi,
                CASE WHEN 'cd' = ANY(s.media_types) OR 'gdrom' = ANY(s.media_types) THEN true ELSE false END AS has_cue
         FROM systems s
         ORDER BY LOWER(s.name), s.name"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let systems = rows
        .into_iter()
        .map(|r| SystemDownload {
            code: r.code,
            name: r.name,
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
    name: String,
    has_sbi: bool,
    has_cue: bool,
}

async fn download_archive(
    State(state): State<AppState>,
    Path((system, archive_type)): Path<(String, String)>,
) -> Response {
    match archive_service::get_or_generate_archive(&state.pool, &state.config, &system, &archive_type).await {
        Ok(data) => {
            let filename = format!("{system}-{archive_type}.zip");
            (
                [
                    (header::CONTENT_TYPE, "application/zip".to_string()),
                    (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
                ],
                data,
            ).into_response()
        }
        Err(e) => e.into_response(),
    }
}
