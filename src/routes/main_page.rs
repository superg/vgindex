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
    region_flags: Vec<HomeRegionFlag>,
    created_at: String,
}

struct HomeRegionFlag {
    flag_code_lower: String,
    name: String,
}

async fn homepage(State(state): State<AppState>, user: CurrentUser) -> Html<String> {
    let rows: Vec<RecentDiscRow> = sqlx::query_as(
        "SELECT d.id, d.title, s.short_code AS system, d.status, d.created_at
         FROM discs d
         JOIN systems s ON s.id = d.system_id
         WHERE d.status != 'Bad'
         ORDER BY d.created_at DESC
         LIMIT 25"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut recent_discs = Vec::with_capacity(rows.len());
    for r in rows {
        let region_rows: Vec<HomeRegionRow> = sqlx::query_as(
            "SELECT r.flag_code, r.name FROM disc_regions dr
             JOIN regions r ON r.id = dr.region_id
             WHERE dr.disc_id = $1 ORDER BY r.display_order"
        )
        .bind(r.id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        recent_discs.push(RecentDisc {
            id: r.id,
            title: r.title,
            system: r.system,
            region_flags: region_rows.into_iter().map(|rr| HomeRegionFlag {
                flag_code_lower: rr.flag_code.to_lowercase(),
                name: rr.name,
            }).collect(),
            created_at: r.created_at.format("%Y-%m-%d %H:%M").to_string(),
        });
    }

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
    status: DiscStatus,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct HomeRegionRow {
    flag_code: String,
    name: String,
}
