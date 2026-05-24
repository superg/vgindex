use askama::Template;
use axum::{extract::State, response::Html, routing::get, Router};

use crate::auth::middleware::CurrentUser;
use crate::config::SiteConfig;
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
impl SiteConfig for MainTemplate {}

struct RecentDisc {
    id: i32,
    title: String,
    system: String,
    region_flags: Vec<HomeRegionFlag>,
    created_at: String,
}

struct HomeRegionFlag {
    code: String,
    name: String,
}

async fn homepage(State(state): State<AppState>, user: CurrentUser) -> Html<String> {
    let rows: Vec<RecentDiscRow> = sqlx::query_as(
        "SELECT d.id, d.title, s.code AS system_code, s.short_name AS system_short_name,
                (SELECT MIN(ds.created_at)
                 FROM disc_submissions ds
                 WHERE ds.target_disc_id = d.id) AS created_at
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         WHERE d.status != 'Disabled'
         ORDER BY d.id DESC
         LIMIT 40",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut recent_discs = Vec::with_capacity(rows.len());
    for r in rows {
        let region_rows: Vec<HomeRegionRow> = sqlx::query_as(
            "SELECT r.flag_code AS code, r.name FROM disc_regions dr
             JOIN regions r ON r.code = dr.region_code
             WHERE dr.disc_id = $1 ORDER BY r.sort_order",
        )
        .bind(r.id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        recent_discs.push(RecentDisc {
            id: r.id,
            title: r.title,
            system: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            region_flags: region_rows
                .into_iter()
                .map(|rr| HomeRegionFlag {
                    code: rr.code.to_lowercase(),
                    name: rr.name,
                })
                .collect(),
            created_at: r
                .created_at
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
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
    system_code: String,
    system_short_name: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct HomeRegionRow {
    code: String,
    name: String,
}
