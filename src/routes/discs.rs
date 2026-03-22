use askama::Template;
use axum::{extract::{Query, State}, response::Html, routing::get, Router};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::db::models::DiscStatus;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/discs/", get(discs_page))
}

#[derive(Deserialize, Default)]
pub struct DiscsQuery {
    pub system: Option<String>,
    pub region: Option<String>,
    pub letter: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page: Option<i64>,
}

const LETTERS: &[&str] = &[
    "A","B","C","D","E","F","G","H","I","J","K","L","M",
    "N","O","P","Q","R","S","T","U","V","W","X","Y","Z",
];

#[derive(Template)]
#[template(path = "discs.html")]
struct DiscsTemplate {
    current_user: Option<String>,
    discs: Vec<DiscRow>,
    systems: Vec<SystemOption>,
    letters: Vec<(String, bool)>,
    filter_system: String,
    filter_status: String,
    filter_letter: String,
    filter_q: String,
    total_count: i64,
    page: i64,
    total_pages: i64,
    prev_page: i64,
    next_page: i64,
    next_title_order: String,
    next_system_order: String,
    next_status_order: String,
}

struct DiscRow {
    id: i32,
    title: String,
    system_code: String,
    version_display: String,
    edition_display: String,
    status_class: String,
    status_display: String,
    region_flags: Vec<RegionFlag>,
    language_flags: Vec<LangFlag>,
    serial: String,
}

struct RegionFlag {
    code: String,
    name: String,
}

struct LangFlag {
    code: String,
    name: String,
}

struct SystemOption {
    code: String,
    name: String,
    selected: bool,
}

const PAGE_SIZE: i64 = 500;

async fn discs_page(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<DiscsQuery>,
) -> Html<String> {
    let page = query.page.unwrap_or(1).max(1);
    let offset = (page - 1) * PAGE_SIZE;

    let filter_system = query.system.clone().unwrap_or_default();
    let filter_status = query.status.clone().unwrap_or_default();
    let filter_letter = query.letter.clone().unwrap_or_default();
    let filter_q = query.q.clone().unwrap_or_default();

    let sys_rows: Vec<SysRow> =
        sqlx::query_as("SELECT code, name FROM systems ORDER BY sort_order, name")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();

    let systems: Vec<SystemOption> = sys_rows.into_iter().map(|s| SystemOption {
        selected: s.code == filter_system,
        code: s.code,
        name: s.name,
    }).collect();

    let mut where_clauses = vec!["1=1".to_string()];
    let mut bind_idx = 0u32;

    if !filter_system.is_empty() {
        bind_idx += 1;
        where_clauses.push(format!("s.code = ${bind_idx}"));
    }
    if filter_letter == "#" {
        where_clauses.push("d.title ~* '^[^a-zA-Z]'".to_string());
    } else if filter_letter.len() == 1 && filter_letter.chars().next().unwrap().is_ascii_alphabetic() {
        bind_idx += 1;
        where_clauses.push(format!("upper(left(d.title, 1)) = upper(${bind_idx})"));
    }
    if filter_status == "Questionable" {
        where_clauses.push("d.questionable".to_string());
    } else if filter_status == "Verified" {
        where_clauses.push("NOT d.questionable AND (SELECT COUNT(*) FROM disc_dumpers dd WHERE dd.disc_id = d.id) > 1".to_string());
    } else if filter_status == "Unverified" {
        where_clauses.push("NOT d.questionable AND (SELECT COUNT(*) FROM disc_dumpers dd WHERE dd.disc_id = d.id) <= 1".to_string());
    }
    if !filter_q.is_empty() {
        bind_idx += 1;
        where_clauses.push(format!(
            "(d.search_vector @@ plainto_tsquery('english', ${bind_idx}) OR d.title ILIKE '%' || ${bind_idx} || '%')"
        ));
    }

    if !user.is_logged_in() {
        where_clauses.push("d.enabled".to_string());
    }

    let where_sql = where_clauses.join(" AND ");

    let sort_col = match query.sort.as_deref() {
        Some("title") => "d.title",
        Some("system") => "s.code",
        Some("status") => "d.questionable",
        Some("updated") => "d.updated_at",
        _ => "d.title",
    };
    let sort_dir = match query.order.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };

    let sql_count = format!(
        "SELECT COUNT(*) FROM discs d JOIN systems s ON s.code = d.system_code WHERE {where_sql}"
    );
    let sql_select = format!(
        "SELECT d.id, d.title, s.code AS system_code, d.serial, d.version, d.edition,
                d.questionable,
                (SELECT COUNT(*) FROM disc_dumpers dd WHERE dd.disc_id = d.id) AS dumper_count
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         WHERE {where_sql}
         ORDER BY {sort_col} {sort_dir} LIMIT {PAGE_SIZE} OFFSET {offset}"
    );

    let mut count_query = sqlx::query_scalar::<_, i64>(&sql_count);
    let mut select_query = sqlx::query_as::<_, RawDiscRow>(&sql_select);

    if !filter_system.is_empty() {
        count_query = count_query.bind(filter_system.clone());
        select_query = select_query.bind(filter_system.clone());
    }
    if filter_letter != "#" && filter_letter.len() == 1 && filter_letter.chars().next().unwrap().is_ascii_alphabetic() {
        count_query = count_query.bind(filter_letter.clone());
        select_query = select_query.bind(filter_letter.clone());
    }
    if !filter_q.is_empty() {
        count_query = count_query.bind(filter_q.clone());
        select_query = select_query.bind(filter_q.clone());
    }

    let total_count = count_query.fetch_one(&state.pool).await.unwrap_or(0);
    let total_pages = (total_count + PAGE_SIZE - 1) / PAGE_SIZE;

    let raw_rows: Vec<RawDiscRow> = select_query
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let mut discs = Vec::with_capacity(raw_rows.len());
    for r in raw_rows {
        let region_rows: Vec<LangRow> = sqlx::query_as(
            "SELECT r.code, r.name FROM disc_regions dr
             JOIN regions r ON r.code = dr.region_code
             WHERE dr.disc_id = $1 ORDER BY r.sort_order",
        )
        .bind(r.id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        let lang_rows: Vec<LangRow> = sqlx::query_as(
            "SELECT l.code, l.name FROM disc_languages dl
             JOIN languages l ON l.code = dl.language_code
             WHERE dl.disc_id = $1 ORDER BY l.sort_order",
        )
        .bind(r.id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        discs.push(DiscRow {
            id: r.id,
            title: r.title,
            system_code: r.system_code,
            version_display: r.version.unwrap_or_default(),
            edition_display: r.edition.unwrap_or_default(),
            status_class: DiscStatus::compute(r.questionable, r.dumper_count).css_class().to_string(),
            status_display: DiscStatus::compute(r.questionable, r.dumper_count).to_string(),
            region_flags: region_rows.into_iter().map(|r| RegionFlag {
                code: r.code.to_lowercase(),
                name: r.name,
            }).collect(),
            language_flags: lang_rows.into_iter().map(|l| LangFlag {
                code: l.code.to_lowercase(),
                name: l.name,
            }).collect(),
            serial: r.serial.unwrap_or_default(),
        });
    }

    let next_title_order = if query.sort.as_deref() == Some("title") && query.order.as_deref() != Some("desc") { "desc" } else { "asc" }.to_string();
    let next_system_order = if query.sort.as_deref() == Some("system") && query.order.as_deref() != Some("desc") { "desc" } else { "asc" }.to_string();
    let next_status_order = if query.sort.as_deref() == Some("status") && query.order.as_deref() != Some("desc") { "desc" } else { "asc" }.to_string();

    Html(
        DiscsTemplate {
            current_user: user.user().map(|u| u.username.clone()),
            discs,
            systems,
            letters: LETTERS.iter().map(|s| (s.to_string(), filter_letter == *s)).collect(),
            filter_system,
            filter_status,
            filter_letter,
            filter_q,
            total_count,
            page,
            total_pages,
            prev_page: page - 1,
            next_page: page + 1,
            next_title_order,
            next_system_order,
            next_status_order,
        }
        .render()
        .unwrap(),
    )
}

#[derive(sqlx::FromRow)]
struct SysRow {
    code: String,
    name: String,
}

#[derive(sqlx::FromRow)]
struct RawDiscRow {
    id: i32,
    title: String,
    system_code: String,
    serial: Option<String>,
    version: Option<String>,
    edition: Option<String>,
    questionable: bool,
    dumper_count: i64,
}

#[derive(sqlx::FromRow)]
struct LangRow {
    code: String,
    name: String,
}
