#![allow(dead_code)]

mod auth;
mod config;
mod db;
mod error;
mod routes;
mod services;

use axum::{middleware, Router};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
    pub oidc: auth::oidc::OidcProvider,
    pub archive_tx: tokio::sync::mpsc::UnboundedSender<String>,
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

    let oidc = auth::oidc::OidcProvider::new();

    let (archive_tx, archive_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let state = AppState {
        pool: pool.clone(),
        config: Arc::new(config.clone()),
        oidc,
        archive_tx,
    };

    tokio::spawn(run_session_cleanup(pool.clone()));

    tokio::spawn(services::archive_service::run_archive_worker(
        archive_rx, pool,
    ));

    let app = Router::new()
        .merge(routes::build_router())
        .nest_service("/static", ServeDir::new("static"))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::middleware::guest_session_layer,
        ))
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

        tokio::time::sleep(std::time::Duration::from_secs(60 * 60)).await;
    }
}
