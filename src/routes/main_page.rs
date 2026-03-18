use askama::Template;
use axum::{extract::State, response::Html, routing::get, Router};

use crate::auth::middleware::CurrentUser;
use crate::db::models::DiscStatus;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(homepage))
}

#[derive(Template)]
#[template(path = "main.html")]
struct MainTemplate {
    current_user: Option<String>,
    recent_discs: Vec<RecentDisc>,
}

struct RecentDisc {
    id: i32,
    title: String,
    system: String,
    region_flag: Option<String>,
    region_flag_lower: String,
    region_name_display: String,
    created_at: String,
}

async fn homepage(State(state): State<AppState>, user: CurrentUser) -> Html<String> {
    let rows: Vec<RecentDiscRow> = sqlx::query_as(
        "SELECT d.id, d.title, s.short_code AS system, sr.flag_code AS region_flag,
                sr.name AS region_name, d.status, d.created_at
         FROM discs d
         JOIN systems s ON s.id = d.system_id
         LEFT JOIN system_regions sr ON sr.id = d.system_region_id
         WHERE d.status != 'Bad'
         ORDER BY d.created_at DESC
         LIMIT 25"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let recent_discs = rows
        .into_iter()
        .map(|r| RecentDisc {
            id: r.id,
            title: r.title,
            system: r.system,
            region_flag_lower: r.region_flag.as_deref().unwrap_or("").to_lowercase(),
            region_name_display: r.region_name.clone().unwrap_or_default(),
            region_flag: r.region_flag,
            created_at: r.created_at.format("%Y-%m-%d %H:%M").to_string(),
        })
        .collect();

    Html(
        MainTemplate {
            current_user: user.user().map(|u| u.username.clone()),
            recent_discs,
        }
        .render()
        .unwrap(),
    )
}

#[derive(sqlx::FromRow)]
struct RecentDiscRow {
    id: i32,
    title: String,
    system: String,
    region_flag: Option<String>,
    region_name: Option<String>,
    status: DiscStatus,
    created_at: chrono::DateTime<chrono::Utc>,
}
