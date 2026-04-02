use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
    routing::get,
    Router,
};
use serde::Deserialize;

use crate::auth::middleware::RequireAuth;
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::AppResult;
use crate::services::queue_service;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/queue", get(queue_list))
        .route("/queue/", get(queue_list))
}

#[derive(Deserialize, Default)]
pub struct QueueQuery {
    pub status: Option<String>,
    pub sub_type: Option<String>,
    pub system: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page: Option<i64>,
}

struct SystemOption {
    code: String,
    name: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "queue.html")]
struct QueueTemplate {
    current_user: Option<String>,
    is_moderator: bool,
    entries: Vec<SubmissionListRow>,
    systems: Vec<SystemOption>,
    filter_status: String,
    filter_type: String,
    filter_system: String,
    total_count: i64,
    page: i64,
    total_pages: i64,
    prev_page: i64,
    next_page: i64,
    sort_column: String,
    sort_order: String,
    next_date_order: String,
    next_title_order: String,
    next_system_order: String,
    next_submitter_order: String,
    next_reviewer_order: String,
    next_type_order: String,
    next_status_order: String,
}
impl SiteConfig for QueueTemplate {}

#[derive(sqlx::FromRow)]
struct SysRow {
    code: String,
    name: String,
}

const PAGE_SIZE: i64 = 50;

async fn queue_list(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<QueueQuery>,
) -> AppResult<Html<String>> {
    let page = query.page.unwrap_or(1).max(1);
    let is_mod = user.role.can_moderate();

    let filter_status = query.status.clone().unwrap_or_else(|| "Pending".to_string());
    let filter_type = query.sub_type.clone().unwrap_or_default();
    let filter_system = query.system.clone().unwrap_or_default();
    let sort_column = query.sort.clone().unwrap_or_else(|| "date".to_string());
    let sort_order = query.order.clone().unwrap_or_else(|| "desc".to_string());

    let status_for_query = if filter_status == "All Statuses" {
        None
    } else if filter_status.is_empty() {
        Some("Pending")
    } else {
        Some(filter_status.as_str())
    };

    let type_for_query = if filter_type.is_empty() { None } else { Some(filter_type.as_str()) };
    let system_for_query = if filter_system.is_empty() { None } else { Some(filter_system.as_str()) };

    let entries = queue_service::list_submissions(
        &state.pool,
        if is_mod { None } else { Some(user.id) },
        status_for_query,
        type_for_query,
        system_for_query,
        &sort_column,
        &sort_order,
        page,
        PAGE_SIZE,
    ).await?;

    let total_count = queue_service::count_submissions(
        &state.pool,
        if is_mod { None } else { Some(user.id) },
        status_for_query,
        type_for_query,
        system_for_query,
    ).await?;

    let total_pages = (total_count + PAGE_SIZE - 1) / PAGE_SIZE;

    let sys_rows: Vec<SysRow> =
        sqlx::query_as("SELECT code, name FROM systems ORDER BY LOWER(name)")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();

    let systems: Vec<SystemOption> = sys_rows.into_iter().map(|s| SystemOption {
        selected: s.code == filter_system,
        code: s.code,
        name: s.name,
    }).collect();

    let is_asc = sort_order != "desc";
    let next_order = |col: &str| -> String {
        if sort_column == col && is_asc { "desc" } else { "asc" }.to_string()
    };

    Ok(Html(
        QueueTemplate {
            current_user: Some(user.username),
            is_moderator: is_mod,
            entries,
            systems,
            filter_status: if filter_status.is_empty() { "Pending".to_string() } else { filter_status },
            filter_type,
            filter_system,
            total_count,
            page,
            total_pages,
            prev_page: page - 1,
            next_page: page + 1,
            sort_column: sort_column.clone(),
            sort_order,
            next_date_order: next_order("date"),
            next_title_order: next_order("title"),
            next_system_order: next_order("system"),
            next_submitter_order: next_order("submitter"),
            next_reviewer_order: next_order("reviewer"),
            next_type_order: next_order("type"),
            next_status_order: next_order("status"),
        }
        .render()
        .unwrap(),
    ))
}
