#![allow(dead_code)]

mod auth;
mod config;
mod db;
mod error;
mod routes;
mod services;

use std::sync::Arc;
use axum::Router;
use sqlx::PgPool;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Arc<Config>,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "redump=debug,tower_http=debug".into()),
        )
        .init();

    let config = Config::from_env();
    let pool = db::create_pool(&config.database_url)
        .await
        .expect("Failed to connect to database");

    db::run_migrations(&pool)
        .await
        .expect("Failed to run migrations");

    std::fs::create_dir_all(&config.data_dir).ok();

    let state = AppState {
        pool,
        config: Arc::new(config.clone()),
    };

    let app = Router::new()
        .merge(routes::build_router())
        .nest_service("/static", ServeDir::new("static"))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    tracing::info!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
