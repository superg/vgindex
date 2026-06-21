#![allow(dead_code)]

mod auth;
mod config;
mod db;
mod error;
mod routes;
mod services;
mod transliteration;

use axum::{middleware, Router};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub http: reqwest::Client,
    pub edition_suggestions: services::disc_service::EditionSuggestionsCache,
    pub news_cache: services::news_service::NewsCache,
    pub transliteration: Arc<transliteration::TransliterationRegistry>,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vgindex=debug,tower_http=debug".into()),
        )
        .init();

    let config = Config::from_env();
    config::init_site_config(&config);
    let pool = db::create_pool(&config.database_url)
        .await
        .expect("Failed to connect to database");

    db::run_migrations(&pool)
        .await
        .expect("Failed to run migrations");

    std::fs::create_dir_all(config::DATA_DIR).ok();

    let transliteration = Arc::new(
        transliteration::TransliterationRegistry::new()
            .expect("Failed to initialize transliteration registry"),
    );

    let archive_metadata =
        services::archive_service::ArchiveMetadata::from_site_url(&config.site_url);

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config.clone()),
        http: reqwest::Client::new(),
        edition_suggestions: services::disc_service::EditionSuggestionsCache::new(
            Duration::from_secs(60 * 60 * 24),
        ),
        news_cache: services::news_service::NewsCache::new(Duration::from_secs(
            services::news_service::NEWS_FEED_TTL_SECONDS,
        )),
        transliteration,
    };

    tokio::spawn(run_session_cleanup(pool.clone()));

    tokio::spawn(services::archive_service::run_archive_worker(
        pool,
        archive_metadata,
    ));

    let app = Router::new()
        .merge(routes::build_router())
        .nest_service("/static", ServeDir::new("static"))
        .layer(middleware::from_fn(routes::canonical_url_middleware))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn run_session_cleanup(pool: PgPool) {
    loop {
        match auth::session::cleanup_expired(&pool).await {
            Ok(deleted) if deleted > 0 => {
                tracing::debug!("Cleaned up {deleted} expired sessions");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Failed to clean up expired sessions: {e}");
            }
        }
        match auth::oidc::cleanup_expired_login_states(&pool).await {
            Ok(deleted) if deleted > 0 => {
                tracing::debug!("Cleaned up {deleted} expired OIDC login states");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Failed to clean up expired OIDC login states: {e}");
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(60 * 60)).await;
    }
}
