use askama::Template;
use axum::{
    extract::{Path, State},
    http::header,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use std::collections::{HashMap, HashSet};

use crate::auth::middleware::{AuthenticatedUser, CurrentUser, RequireAuth};
use crate::config::SiteConfig;
use crate::services::archive_service;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/downloads", get(downloads_page))
        .route("/datfile/{system}", get(download_dat))
        .route("/cues/{system}", get(download_cue))
        .route("/keys/{system}", get(download_key))
        .route("/sbi/{system}", get(download_sbi))
}

#[derive(Template)]
#[template(path = "downloads.html")]
struct DownloadsTemplate {
    current_user: Option<AuthenticatedUser>,
    can_download_keys: bool,
    systems: Vec<SystemDownload>,
    bios_systems: Vec<BiosDownload>,
}
impl SiteConfig for DownloadsTemplate {}

struct SystemDownload {
    code: String,
    name: String,
    has_dat: bool,
    has_cue: bool,
    has_key: bool,
    has_sbi: bool,
}

struct BiosDownload {
    name: String,
    href: &'static str,
}

struct BiosDownloadSpec {
    code: &'static str,
    fallback_name: &'static str,
    href: &'static str,
}

const BIOS_DOWNLOADS: &[BiosDownloadSpec] = &[
    BiosDownloadSpec {
        code: "XBOX",
        fallback_name: "Microsoft Xbox",
        href: "/static/bios/Microsoft%20-%20Xbox%20-%20BIOS%20Images%20%289%29%20%282026-06-16%29.dat",
    },
    BiosDownloadSpec {
        code: "GC",
        fallback_name: "Nintendo GameCube",
        href: "/static/bios/Nintendo%20-%20GameCube%20-%20BIOS%20Images%20%2817%29%20%282026-06-16%29.dat",
    },
    BiosDownloadSpec {
        code: "PSX",
        fallback_name: "Sony PlayStation",
        href: "/static/bios/Sony%20-%20PlayStation%20-%20BIOS%20Images%20%2824%29%20%282026-06-16%29.dat",
    },
    BiosDownloadSpec {
        code: "PS2",
        fallback_name: "Sony PlayStation 2",
        href: "/static/bios/Sony%20-%20PlayStation%202%20-%20BIOS%20Datfile%20%28140%29%20%282026-06-16%29.dat",
    },
];

fn bios_downloads(system_names: &HashMap<String, String>) -> Vec<BiosDownload> {
    BIOS_DOWNLOADS
        .iter()
        .map(|spec| BiosDownload {
            name: system_names
                .get(spec.code)
                .cloned()
                .unwrap_or_else(|| spec.fallback_name.to_string()),
            href: spec.href,
        })
        .collect()
}

async fn downloads_page(State(state): State<AppState>, user: CurrentUser) -> Html<String> {
    let media_types: Vec<MediaTypeCdRow> =
        sqlx::query_as("SELECT code, rom_extension FROM media_types")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();
    let cd_media_codes: HashSet<String> = media_types
        .into_iter()
        .filter(|mt| crate::db::models::is_cd_rom_extension(&mt.rom_extension))
        .map(|mt| mt.code)
        .collect();

    let rows: Vec<SystemDownloadRow> = sqlx::query_as(
        "SELECT s.code, s.manufacturer, s.name, s.has_key, s.has_sbi, s.media_types
         FROM systems s
         ORDER BY LOWER(CONCAT_WS(' ', NULLIF(s.manufacturer, ''), s.name))",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let system_names: HashMap<String, String> = rows
        .iter()
        .map(|r| {
            (
                r.code.clone(),
                crate::db::models::build_system_name(&r.manufacturer, &r.name),
            )
        })
        .collect();

    let systems =
        rows.iter()
            .map(|r| SystemDownload {
                name: system_names.get(&r.code).cloned().unwrap_or_else(|| {
                    crate::db::models::build_system_name(&r.manufacturer, &r.name)
                }),
                code: r.code.clone(),
                has_dat: true,
                has_cue: r
                    .media_types
                    .iter()
                    .any(|code| cd_media_codes.contains(code)),
                has_key: r.has_key,
                has_sbi: r.has_sbi,
            })
            .collect();

    Html(
        DownloadsTemplate {
            current_user: user.user().cloned(),
            can_download_keys: user.is_logged_in(),
            systems,
            bios_systems: bios_downloads(&system_names),
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
    has_key: bool,
    has_sbi: bool,
    media_types: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct MediaTypeCdRow {
    code: String,
    rom_extension: String,
}

fn normalize_archive_system_code(system: &str) -> String {
    system.trim().to_ascii_uppercase()
}

async fn serve_archive(state: &AppState, system: &str, archive_type: &str) -> Response {
    let system = normalize_archive_system_code(system);
    match archive_service::get_cached_archive(&state.pool, &system, archive_type).await {
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

async fn download_dat(State(state): State<AppState>, Path(system): Path<String>) -> Response {
    serve_archive(&state, &system, "dat").await
}

async fn download_cue(State(state): State<AppState>, Path(system): Path<String>) -> Response {
    serve_archive(&state, &system, "cue").await
}

async fn download_key(
    State(state): State<AppState>,
    _user: RequireAuth,
    Path(system): Path<String>,
) -> Response {
    serve_archive(&state, &system, "key").await
}

async fn download_sbi(State(state): State<AppState>, Path(system): Path<String>) -> Response {
    serve_archive(&state, &system, "sbi").await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::UserRole;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    fn test_system_names() -> HashMap<String, String> {
        [
            ("XBOX", "Microsoft Xbox"),
            ("GC", "Nintendo GameCube"),
            ("PSX", "Sony PlayStation"),
            ("PS2", "Sony PlayStation 2"),
        ]
        .into_iter()
        .map(|(code, name)| (code.to_string(), name.to_string()))
        .collect()
    }

    fn template(can_download_keys: bool) -> DownloadsTemplate {
        let system_names = test_system_names();

        DownloadsTemplate {
            current_user: can_download_keys.then(|| AuthenticatedUser {
                id: 1,
                username: "tester".to_string(),
                role: UserRole::User,
                csrf_token: "test-csrf-token".to_string(),
                avatar_url: None,
            }),
            can_download_keys,
            systems: vec![
                SystemDownload {
                    code: "PS3".to_string(),
                    name: "Sony - PlayStation 3".to_string(),
                    has_dat: true,
                    has_cue: false,
                    has_key: true,
                    has_sbi: false,
                },
                SystemDownload {
                    code: "PC".to_string(),
                    name: "IBM - PC compatible".to_string(),
                    has_dat: true,
                    has_cue: true,
                    has_key: false,
                    has_sbi: false,
                },
            ],
            bios_systems: bios_downloads(&system_names),
        }
    }

    fn test_state() -> AppState {
        let database_url = "postgres://postgres:postgres@localhost/postgres".to_string();

        AppState {
            pool: sqlx::postgres::PgPoolOptions::new()
                .connect_lazy(&database_url)
                .unwrap(),
            config: Arc::new(crate::config::Config {
                site_name: "localhost".to_string(),
                database_url,
                site_url: "http://localhost".to_string(),
                base_url: "http://localhost".to_string(),
                wiki_url: "#".to_string(),
                forum_url: "#".to_string(),
                news_feed_url: "#".to_string(),
                port: 0,
                oidc_provider_url: "#".to_string(),
                oidc_client_id: "test".to_string(),
                oidc_client_secret: "test".to_string(),
            }),
            http: reqwest::Client::new(),
            edition_suggestions: crate::services::disc_service::EditionSuggestionsCache::new(
                Duration::from_secs(60),
            ),
            news_cache: crate::services::news_service::NewsCache::new(Duration::from_secs(
                crate::services::news_service::NEWS_FEED_TTL_SECONDS,
            )),
            transliteration: Arc::new(
                crate::transliteration::TransliterationRegistry::new().unwrap(),
            ),
        }
    }

    #[test]
    fn downloads_page_hides_key_links_from_guests() {
        let html = template(false).render().unwrap();

        assert!(!html.contains(r#"/keys/PS3"#));
        assert!(html.contains(r#"/datfile/PS3"#));
        assert!(html.contains(r#"/cues/PC"#));
    }

    #[test]
    fn downloads_page_shows_key_links_to_authenticated_users() {
        let html = template(true).render().unwrap();

        assert!(html.contains(r#"/keys/PS3"#));
        assert!(!html.contains(r#"/keys/PC"#));
    }

    #[test]
    fn downloads_page_shows_bios_dat_links() {
        let html = template(false).render().unwrap();

        assert!(html.contains(">BIOS<"));
        assert!(html.contains("Microsoft Xbox"));
        assert!(html.contains(
            r#"/static/bios/Microsoft%20-%20Xbox%20-%20BIOS%20Images%20%289%29%20%282026-06-16%29.dat"#
        ));
        assert!(html.contains("Nintendo GameCube"));
        assert!(html.contains(
            r#"/static/bios/Nintendo%20-%20GameCube%20-%20BIOS%20Images%20%2817%29%20%282026-06-16%29.dat"#
        ));
        assert!(html.contains("Sony PlayStation"));
        assert!(html.contains(
            r#"/static/bios/Sony%20-%20PlayStation%20-%20BIOS%20Images%20%2824%29%20%282026-06-16%29.dat"#
        ));
        assert!(html.contains("Sony PlayStation 2"));
        assert!(html.contains(
            r#"/static/bios/Sony%20-%20PlayStation%202%20-%20BIOS%20Datfile%20%28140%29%20%282026-06-16%29.dat"#
        ));
    }

    #[test]
    fn archive_download_system_path_is_case_insensitive() {
        assert_eq!(normalize_archive_system_code("ps3"), "PS3");
        assert_eq!(normalize_archive_system_code("Pc-98"), "PC-98");
    }

    #[tokio::test]
    async fn key_download_route_rejects_guest_direct_links() {
        let app = routes().with_state(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/keys/PS3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
