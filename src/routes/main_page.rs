use askama::Template;
use axum::{extract::State, response::Html, routing::get, Router};

use crate::auth::middleware::{AuthenticatedUser, CurrentUser};
use crate::config::SiteConfig;
use crate::services::disc_service::RECENT_CHANGE_PREDICATE;
use crate::services::news_service::NewsItem;
use crate::AppState;

const HOME_RECENT_LIMIT: i64 = 30;
const HOME_NEWS_LIMIT: usize = 3;
// Shares `RECENT_CHANGE_PREDICATE` with the disc list "Modification date" sort so
// both agree on what counts as a genuine change.
fn home_recent_changes_sql() -> String {
    format!(
        "SELECT d.id, d.title, s.code AS system_code, s.short_name AS system_short_name,
                MAX(ds.created_at) AS modified_at
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         JOIN disc_submissions ds ON ds.target_disc_id = d.id
         WHERE d.status != 'Disabled'
           AND {RECENT_CHANGE_PREDICATE}
         GROUP BY d.id, d.title, s.code, s.short_name
         ORDER BY MAX(ds.created_at) DESC, d.id DESC
         LIMIT $1"
    )
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(homepage))
}

#[derive(Template)]
#[template(path = "main.html")]
struct MainTemplate {
    current_user: Option<AuthenticatedUser>,
    news_items: Vec<NewsItem>,
    recent_discs: Vec<RecentDisc>,
    recent_changes: Vec<RecentChange>,
}
impl SiteConfig for MainTemplate {}

struct RecentDisc {
    id: i32,
    title: String,
    system: String,
    region_flags: Vec<HomeRegionFlag>,
    created_at: String,
}

struct RecentChange {
    id: i32,
    title: String,
    system: String,
    region_flags: Vec<HomeRegionFlag>,
    modified_at: String,
}

struct HomeRegionFlag {
    code: String,
    name: String,
}

async fn homepage(State(state): State<AppState>, user: CurrentUser) -> Html<String> {
    let mut news_items = state
        .news_cache
        .get(&state.http, &state.config.news_feed_url)
        .await
        .unwrap_or_else(|err| {
            tracing::warn!("Failed to load homepage news: {err}");
            Vec::new()
        });
    news_items.truncate(HOME_NEWS_LIMIT);

    let rows: Vec<RecentDiscRow> = sqlx::query_as(
        "SELECT d.id, d.title, s.code AS system_code, s.short_name AS system_short_name,
                (SELECT MIN(ds.created_at)
                 FROM disc_submissions ds
                 WHERE ds.target_disc_id = d.id) AS created_at
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         WHERE d.status != 'Disabled'
         ORDER BY d.id DESC
         LIMIT $1",
    )
    .bind(HOME_RECENT_LIMIT)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut recent_discs = Vec::with_capacity(rows.len());
    for r in rows {
        recent_discs.push(RecentDisc {
            id: r.id,
            title: r.title,
            system: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            region_flags: load_home_region_flags(&state.pool, r.id).await,
            created_at: r
                .created_at
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
        });
    }

    let change_rows: Vec<RecentChangeRow> = sqlx::query_as(&home_recent_changes_sql())
        .bind(HOME_RECENT_LIMIT)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let mut recent_changes = Vec::with_capacity(change_rows.len());
    for r in change_rows {
        recent_changes.push(RecentChange {
            id: r.id,
            title: r.title,
            system: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            region_flags: load_home_region_flags(&state.pool, r.id).await,
            modified_at: r
                .modified_at
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
        });
    }

    Html(
        MainTemplate {
            current_user: user.user().cloned(),
            news_items,
            recent_discs,
            recent_changes,
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
struct RecentChangeRow {
    id: i32,
    title: String,
    system_code: String,
    system_short_name: String,
    modified_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct HomeRegionRow {
    code: String,
    name: String,
}

async fn load_home_region_flags(pool: &sqlx::PgPool, disc_id: i32) -> Vec<HomeRegionFlag> {
    let region_rows: Vec<HomeRegionRow> = sqlx::query_as(
        "SELECT r.flag_code AS code, r.name FROM disc_regions dr
         JOIN regions r ON r.code = dr.region_code
         WHERE dr.disc_id = $1 ORDER BY r.sort_order",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    region_rows
        .into_iter()
        .map(|rr| HomeRegionFlag {
            code: rr.code.to_lowercase(),
            name: rr.name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_changes_query_only_uses_public_history_rows() {
        assert!(home_recent_changes_sql().contains("ds.status IN ('Approved', 'Legacy')"));
    }

    #[test]
    fn recent_changes_query_excludes_new_disc_creation_rows() {
        let sql = home_recent_changes_sql();
        assert!(sql.contains("ds.changes = '{}'::jsonb"));
        assert!(sql.contains(
            "COALESCE(ds.review_comment, '') IN ('added-backfill', 'no-added-sentinel')"
        ));
        assert!(sql.contains("ds.submission_type = 'Disc'"));
        assert!(sql.contains("ds.id <> ("));
        assert!(sql.contains("SELECT MIN(ds_first.id)"));
    }

    #[test]
    fn recent_changes_query_keeps_edit_rows() {
        assert!(home_recent_changes_sql().contains("ds.submission_type = 'Edit'"));
    }
}
