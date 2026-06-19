use askama::Template;
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use serde::Deserialize;

use crate::auth::{
    csrf::{self, CsrfForm},
    middleware::{AuthenticatedUser, RequireModerator},
};
use crate::config::SiteConfig;
use crate::services::{archive_service, disc_service};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/maintenance", get(maintenance_page))
        .route("/maintenance/rebuild-cue", post(rebuild_cue))
        .route(
            "/maintenance/clear-archives-cache",
            post(clear_archives_cache),
        )
}

#[derive(Deserialize, Default)]
struct MaintenanceQuery {
    status: Option<String>,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "maintenance.html")]
struct MaintenanceTemplate {
    current_user: Option<AuthenticatedUser>,
    status_message: String,
    error_message: String,
}
impl SiteConfig for MaintenanceTemplate {}

async fn maintenance_page(
    RequireModerator(user): RequireModerator,
    Query(query): Query<MaintenanceQuery>,
) -> Html<String> {
    Html(
        MaintenanceTemplate {
            current_user: Some(user),
            status_message: query.status.unwrap_or_default(),
            error_message: query.error.unwrap_or_default(),
        }
        .render()
        .unwrap(),
    )
}

async fn rebuild_cue(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<CsrfForm>,
) -> crate::error::AppResult<Response> {
    csrf::verify_form(&user, &form)?;

    Ok(match disc_service::regenerate_all_cue_entries(&state.pool).await {
        Ok(summary) => redirect_with_message(
            "status",
            &format!(
                "Rebuilt CUE for {} disc(s): {} active, {} cue text update(s), {} file metadata upsert(s), {} file metadata delete(s), {} unchanged.",
                summary.total,
                summary.active,
                summary.updated_cues,
                summary.upserted_file_entries,
                summary.deleted_file_entries,
                summary.skipped,
            ),
        ),
        Err(err) => {
            tracing::error!("Failed to rebuild database cue: {err}");
            redirect_with_message("error", "Failed to rebuild database cue.")
        }
    })
}

async fn clear_archives_cache(
    RequireModerator(user): RequireModerator,
    Form(form): Form<CsrfForm>,
) -> crate::error::AppResult<Response> {
    csrf::verify_form(&user, &form)?;

    Ok(match archive_service::clear_archives_cache() {
        Ok(true) => redirect_with_message("status", "Cleared archives cache."),
        Ok(false) => redirect_with_message("status", "Archives cache was already empty."),
        Err(err) => {
            tracing::error!("Failed to clear archives cache: {err}");
            redirect_with_message("error", "Failed to clear archives cache.")
        }
    })
}

fn redirect_with_message(param: &str, message: &str) -> Response {
    let location = format!("/maintenance?{param}={}", urlencoding::encode(message));
    Redirect::to(&location).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::middleware::AuthenticatedUser;
    use crate::db::models::UserRole;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    #[derive(Template)]
    #[template(
        source = "{% extends \"base.html\" %}{% block title %}Test{% endblock %}{% block content %}{% endblock %}",
        ext = "html"
    )]
    struct BaseMenuTemplate {
        current_user: Option<AuthenticatedUser>,
    }
    impl SiteConfig for BaseMenuTemplate {}

    fn auth_user(role: UserRole) -> AuthenticatedUser {
        AuthenticatedUser {
            id: 1,
            username: "tester".to_string(),
            role,
            csrf_token: "test-csrf-token".to_string(),
            avatar_url: None,
        }
    }

    fn render_menu(role: UserRole) -> String {
        BaseMenuTemplate {
            current_user: Some(auth_user(role)),
        }
        .render()
        .unwrap()
    }

    fn test_state() -> AppState {
        let (archive_tx, _archive_rx) = tokio::sync::mpsc::unbounded_channel();
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
            archive_tx,
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
    fn user_menu_removes_settings_for_all_roles() {
        for role in [
            UserRole::User,
            UserRole::UserPlus,
            UserRole::Moderator,
            UserRole::Admin,
        ] {
            let html = render_menu(role);
            assert!(!html.contains("Settings"));
        }
    }

    #[test]
    fn user_menu_shows_maintenance_only_to_moderators_and_admins() {
        for role in [UserRole::User, UserRole::UserPlus] {
            let html = render_menu(role);
            assert!(!html.contains(r#"<a href="/maintenance">Maintenance</a>"#));
        }

        for role in [UserRole::Moderator, UserRole::Admin] {
            let html = render_menu(role);
            assert!(html.contains(r#"<a href="/maintenance">Maintenance</a>"#));
        }
    }

    #[test]
    fn user_menu_places_maintenance_above_logout() {
        let html = render_menu(UserRole::Moderator);
        let maintenance_pos = html
            .find(r#"<a href="/maintenance">Maintenance</a>"#)
            .unwrap();
        let logout_pos = html.find(r#"action="/logout""#).unwrap();

        assert!(maintenance_pos < logout_pos);
    }

    #[test]
    fn logged_in_base_template_emits_csrf_meta_and_logout_field() {
        let html = render_menu(UserRole::User);

        assert!(html.contains(r#"<meta name="csrf-token" content="test-csrf-token">"#));
        assert!(html.contains(r#"<input type="hidden" name="_csrf" value="test-csrf-token">"#));
    }

    #[test]
    fn maintenance_forms_include_csrf_fields() {
        let html = MaintenanceTemplate {
            current_user: Some(auth_user(UserRole::Moderator)),
            status_message: String::new(),
            error_message: String::new(),
        }
        .render()
        .unwrap();

        assert_eq!(
            html.matches(r#"name="_csrf" value="test-csrf-token""#)
                .count(),
            3
        );
        assert!(html.contains(r#"action="/maintenance/rebuild-cue""#));
        assert!(html.contains(r#"action="/maintenance/clear-archives-cache""#));
    }

    #[tokio::test]
    async fn maintenance_page_rejects_guests() {
        let response = routes()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/maintenance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn maintenance_post_routes_reject_guests() {
        for uri in [
            "/maintenance/rebuild-cue",
            "/maintenance/clear-archives-cache",
        ] {
            let response = routes()
                .with_state(test_state())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}
