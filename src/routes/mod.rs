pub mod about;
pub mod api;
pub mod auth_routes;
pub mod disc_edit;
pub mod disc_view;
pub mod discs;
pub mod downloads;
pub mod main_page;
pub mod maintenance;
pub mod queue;

use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::{
    extract::{rejection::PathRejection, Path, Request},
    http::{header, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
    Router,
};

pub fn build_router() -> Router<AppState> {
    Router::new()
        .merge(main_page::routes())
        .merge(auth_routes::routes())
        .merge(crate::auth::oidc::routes())
        .merge(discs::routes())
        .merge(disc_view::routes())
        .merge(disc_edit::routes())
        .merge(downloads::routes())
        .merge(queue::routes())
        .merge(api::routes())
        .merge(maintenance::routes())
        .merge(about::routes())
}

pub(crate) fn path_i32(path: Result<Path<i32>, PathRejection>) -> AppResult<i32> {
    path.map(|Path(id)| id).map_err(|_| AppError::NotFound)
}

pub async fn canonical_url_middleware(request: Request, next: Next) -> Response {
    if let Some(target) = canonical_url_target(request.uri()) {
        return (StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, target)]).into_response();
    }

    next.run(request).await
}

pub(crate) fn canonicalize_root_relative_url(url: &str) -> String {
    let (path, query) = split_path_query(url);
    canonical_target(path, query).unwrap_or_else(|| url.to_string())
}

fn canonical_url_target(uri: &Uri) -> Option<String> {
    canonical_target(uri.path(), uri.query())
}

fn split_path_query(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    }
}

fn canonical_target(path: &str, query: Option<&str>) -> Option<String> {
    canonical_path(path).map(|path| match query {
        Some(query) => format!("{path}?{query}"),
        None => path,
    })
}

fn canonical_path(path: &str) -> Option<String> {
    if path == "/" {
        return None;
    }

    let trimmed = path.trim_end_matches('/');
    let mut canonical = if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    };

    if let Some(normalized) = canonicalize_path_case(&canonical) {
        canonical = normalized;
    }

    (canonical != path).then_some(canonical)
}

fn canonicalize_path_case(path: &str) -> Option<String> {
    let mut changed = false;
    let mut out = Vec::new();

    for (index, segment) in path.split('/').enumerate() {
        if index == 0 {
            out.push(String::new());
            continue;
        }

        if index == 1 && segment.eq_ignore_ascii_case("static") {
            if segment != "static" {
                changed = true;
            }
            out.push("static".to_string());
            let rest = path
                .split('/')
                .skip(2)
                .map(str::to_string)
                .collect::<Vec<_>>();
            out.extend(rest);
            break;
        }

        if let Some(canonical) = canonical_route_segment(segment) {
            if segment != canonical {
                changed = true;
            }
            out.push(canonical.to_string());
        } else {
            out.push(segment.to_string());
        }
    }

    changed.then(|| out.join("/"))
}

fn canonical_route_segment(segment: &str) -> Option<&'static str> {
    match segment.to_ascii_lowercase().as_str() {
        "about" => Some("about"),
        "api" => Some("api"),
        "auth" => Some("auth"),
        "backups" => Some("backups"),
        "callback" => Some("callback"),
        "cue" => Some("cue"),
        "cues" => Some("cues"),
        "datfile" => Some("datfile"),
        "disc" => Some("disc"),
        "discs" => Some("discs"),
        "downloads" => Some("downloads"),
        "edit" => Some("edit"),
        "keys" => Some("keys"),
        "login" => Some("login"),
        "logout" => Some("logout"),
        "maintenance" => Some("maintenance"),
        "oidc" => Some("oidc"),
        "queue" => Some("queue"),
        "review" => Some("review"),
        "rebuild-cue" => Some("rebuild-cue"),
        "sbi" => Some("sbi"),
        "submit" => Some("submit"),
        "trigger-archive-generation" => Some("trigger-archive-generation"),
        "transliterate" => Some("transliterate"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, Uri};
    use axum::middleware;
    use axum::routing::get;
    use tower::ServiceExt;

    #[test]
    fn path_i32_returns_value_for_valid_extractor() {
        assert_eq!(path_i32(Ok(Path(123))).unwrap(), 123);
    }

    fn target(uri: &str) -> Option<String> {
        canonical_url_target(&uri.parse::<Uri>().unwrap())
    }

    #[test]
    fn canonical_url_removes_trailing_slash() {
        assert_eq!(target("/disc/1/").as_deref(), Some("/disc/1"));
        assert_eq!(target("/about/").as_deref(), Some("/about"));
        assert_eq!(
            target("/datfile/PS2/serial,version/").as_deref(),
            Some("/datfile/PS2/serial,version")
        );
    }

    #[test]
    fn canonical_url_lowercases_route_segments() {
        assert_eq!(target("/DISC/1/CUE/").as_deref(), Some("/disc/1/cue"));
        assert_eq!(
            target("/queue/42/REVIEW/").as_deref(),
            Some("/queue/42/review")
        );
    }

    #[test]
    fn canonical_url_preserves_query_string() {
        assert_eq!(
            target("/DISCS/?System=PS3&Q=Foo").as_deref(),
            Some("/discs?System=PS3&Q=Foo")
        );
    }

    #[test]
    fn canonical_url_preserves_root() {
        assert_eq!(target("/"), None);
    }

    #[test]
    fn canonical_url_does_not_lowercase_static_filename_segments() {
        assert_eq!(target("/static/Bios/File.DAT"), None);
        assert_eq!(
            target("/STATIC/Bios/File.DAT/").as_deref(),
            Some("/static/Bios/File.DAT")
        );
    }

    #[tokio::test]
    async fn canonical_url_middleware_redirects_permanently() {
        let app = Router::new()
            .route("/disc/{id}/cue", get(|| async { "ok" }))
            .layer(middleware::from_fn(canonical_url_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/DISC/1/CUE/?keep=Yes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/disc/1/cue?keep=Yes"
        );
    }

    #[test]
    fn templates_do_not_generate_trailing_slash_app_urls() {
        let templates = [
            include_str!("../../templates/base.html"),
            include_str!("../../templates/disc_edit.html"),
            include_str!("../../templates/disc_view.html"),
            include_str!("../../templates/discs.html"),
            include_str!("../../templates/maintenance.html"),
            include_str!("../../templates/main.html"),
            include_str!("../../templates/queue.html"),
            include_str!("../../templates/queue_detail.html"),
        ];

        for template in templates {
            for forbidden in [
                "href=\"/about/\"",
                "href=\"/disc/submit/\"",
                "href=\"/discs/\"",
                "href=\"/downloads/\"",
                "href=\"/maintenance/\"",
                "href=\"/queue/\"",
                "action=\"/discs/\"",
                "action=\"/maintenance/rebuild-cue/\"",
                "action=\"/maintenance/trigger-archive-generation/\"",
                "action=\"/queue/\"",
                "data-href=\"/discs/\"",
                "data-href=\"/queue/\"",
                "/queue/?",
                "/discs/?",
            ] {
                assert!(
                    !template.contains(forbidden),
                    "template contains trailing slash URL pattern {forbidden}"
                );
            }
        }
    }
}
