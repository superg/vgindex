use askama::Template;
use axum::{extract::{Query, State}, response::{Html, IntoResponse, Redirect, Response}, routing::get, Router};
use serde::Deserialize;

use crate::auth::middleware::CurrentUser;
use crate::config::SiteConfig;
use crate::db::models::{DiscStatus, format_display_title};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/discs", get(discs_page))
        .route("/discs/", get(discs_page))
}

fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
    }
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
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub page: Option<i64>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub dumper: Option<i32>,
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
    regions: Vec<RegionOption>,
    dumpers: Vec<DumperOption>,
    letters: Vec<(String, bool)>,
    filter_system: String,
    filter_region: String,
    filter_status: String,
    filter_letter: String,
    filter_q: String,
    filter_dumper: String,
    filter_dumper_name: String,
    total_count: i64,
    page: i64,
    total_pages: i64,
    prev_page: i64,
    next_page: i64,
    sort_column: String,
    sort_order: String,
    next_region_order: String,
    next_title_order: String,
    next_system_order: String,
    next_version_order: String,
    next_edition_order: String,
    next_language_order: String,
    next_serial_order: String,
    next_status_order: String,
}
impl SiteConfig for DiscsTemplate {}

struct DiscRow {
    id: i32,
    title: String,
    title_foreign: String,
    system_code: String,
    system_display: String,
    dumped_by_me: bool,
    version: String,
    edition_display: String,
    status_emoji: String,
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

struct RegionOption {
    code: String,
    name: String,
    selected: bool,
}

struct DumperOption {
    id: i32,
    name: String,
    selected: bool,
}

const PAGE_SIZE: i64 = 500;

async fn discs_page(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<DiscsQuery>,
) -> Response {
    let page = query.page.unwrap_or(1).max(1);
    let offset = (page - 1) * PAGE_SIZE;

    let filter_system = query.system.clone().unwrap_or_default();
    let filter_region = query.region.clone().unwrap_or_default();
    let filter_status = query.status.clone().unwrap_or_default();
    let filter_letter = query.letter.clone().unwrap_or_default();
    let filter_q = query.q.clone().unwrap_or_default().trim().to_string();
    let filter_dumper_id = query.dumper;
    let filter_dumper = filter_dumper_id.map(|id| id.to_string()).unwrap_or_default();

    let filter_dumper_name = if let Some(dumper_id) = filter_dumper_id {
        sqlx::query_scalar::<_, String>("SELECT username FROM users WHERE id = $1")
            .bind(dumper_id)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None)
            .unwrap_or_default()
    } else {
        String::new()
    };

    let sys_rows: Vec<SystemDropdownRow> = sqlx::query_as(
        "SELECT code, manufacturer, name FROM systems
         ORDER BY LOWER(manufacturer), manufacturer, LOWER(name), name",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let systems: Vec<SystemOption> = sys_rows
        .into_iter()
        .map(|s| SystemOption {
            selected: s.code == filter_system,
            name: crate::db::models::build_system_name(&s.manufacturer, &s.name),
            code: s.code,
        })
        .collect();

    let region_rows: Vec<SysRow> =
        sqlx::query_as("SELECT code, name FROM regions ORDER BY LOWER(name)")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();

    let regions: Vec<RegionOption> = region_rows.into_iter().map(|r| RegionOption {
        selected: r.code.trim() == filter_region,
        code: r.code.trim().to_string(),
        name: r.name,
    }).collect();

    let dumper_rows: Vec<DumperRow> = sqlx::query_as(
        "SELECT u.id, u.username AS name
         FROM users u
         WHERE EXISTS (SELECT 1 FROM disc_dumpers dd WHERE dd.user_id = u.id)
         ORDER BY LOWER(u.username)"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let dumpers: Vec<DumperOption> = dumper_rows.into_iter().map(|d| DumperOption {
        selected: filter_dumper_id == Some(d.id),
        id: d.id,
        name: d.name,
    }).collect();

    let mut where_clauses = vec!["1=1".to_string()];
    let mut bind_idx = 0u32;

    if !filter_system.is_empty() {
        bind_idx += 1;
        where_clauses.push(format!("s.code = ${bind_idx}"));
    }
    if !filter_region.is_empty() {
        bind_idx += 1;
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM disc_regions dr WHERE dr.disc_id = d.id AND dr.region_code = ${bind_idx})"
        ));
    }
    if filter_letter == "#" {
        where_clauses.push("d.title ~* '^[^a-zA-Z]'".to_string());
    } else if filter_letter.len() == 1 && filter_letter.chars().next().unwrap().is_ascii_alphabetic() {
        bind_idx += 1;
        where_clauses.push(format!("upper(left(d.title, 1)) = upper(${bind_idx})"));
    }
    if filter_status == "Disabled" {
        where_clauses.push("NOT d.enabled".to_string());
    } else if filter_status == "All Statuses" {
        // no filter — show both enabled and disabled
    } else if filter_status == "Questionable" {
        where_clauses.push("d.enabled AND d.questionable".to_string());
    } else if filter_status == "Verified" {
        where_clauses.push("d.enabled AND NOT d.questionable AND (SELECT COUNT(*) FROM disc_dumpers dd WHERE dd.disc_id = d.id) > 1".to_string());
    } else if filter_status == "Unverified" {
        where_clauses.push("d.enabled AND NOT d.questionable AND (SELECT COUNT(*) FROM disc_dumpers dd WHERE dd.disc_id = d.id) <= 1".to_string());
    } else {
        where_clauses.push("d.enabled".to_string());
    }
    if !filter_q.is_empty() {
        bind_idx += 1;
        where_clauses.push(format!(
            r#"(d.title ILIKE '%' || ${bind_idx} || '%'
             OR COALESCE(d.title_foreign, '') ILIKE '%' || ${bind_idx} || '%'
             OR COALESCE(d.disc_title, '') ILIKE '%' || ${bind_idx} || '%'
             OR EXISTS (
                 SELECT 1
                 FROM unnest(d.serial) elem
                 WHERE regexp_replace(elem, '\s', '', 'g')
                       ILIKE '%' || regexp_replace(${bind_idx}, '\s', '', 'g') || '%'
             )
             OR EXISTS (
                 SELECT 1
                 FROM unnest(d.barcode) elem
                 WHERE regexp_replace(elem, '\s', '', 'g')
                       ILIKE '%' || regexp_replace(${bind_idx}, '\s', '', 'g') || '%'
             ))"#
        ));
    }
    if filter_dumper_id.is_some() {
        bind_idx += 1;
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM disc_dumpers dd2 WHERE dd2.disc_id = d.id AND dd2.user_id = ${bind_idx})"
        ));
    }

    let where_sql = where_clauses.join(" AND ");
    let current_user_id = user.user().map(|u| u.id);
    let dumped_by_me_sql = if current_user_id.is_some() {
        bind_idx += 1;
        format!(
            "EXISTS (SELECT 1 FROM disc_dumpers dd_self WHERE dd_self.disc_id = d.id AND dd_self.user_id = ${bind_idx})"
        )
    } else {
        "FALSE".to_string()
    };

    let sort_column = query.sort.clone().unwrap_or_else(|| "title".to_string());
    let sort_order_str = query.order.clone().unwrap_or_else(|| "asc".to_string());

    let sort_col = match sort_column.as_str() {
        "region"   => "(SELECT MIN(r.sort_order) FROM disc_regions dr JOIN regions r ON r.code = dr.region_code WHERE dr.disc_id = d.id)",
        "title"    => "LOWER(d.title)",
        "system"   => "LOWER(s.manufacturer), s.manufacturer, LOWER(s.name), s.name",
        "version"  => "LOWER(d.version)",
        "edition"  => "LOWER(array_to_string(d.edition, ', '))",
        "language" => "(SELECT MIN(l.sort_order) FROM disc_languages dl JOIN languages l ON l.code = dl.language_code WHERE dl.disc_id = d.id)",
        "serial"   => "LOWER(array_to_string(d.serial, ', '))",
        "status"   => "CASE WHEN d.questionable THEN 3 WHEN (SELECT COUNT(*) FROM disc_dumpers dd WHERE dd.disc_id = d.id) > 1 THEN 1 ELSE 2 END",
        // Filter out DUMPER_CREDIT_SENTINEL_TS (1970-01-02) rows so the
        // synthetic credit-marker submissions added by the importer's
        // dumper-credit and Green-status fallback passes never surface as a
        // disc's "added" or "updated" sort key. The 1970-01-01 sentinel is
        // intentionally retained: it survives MIN() so the disc-view
        // template can recognize it and suppress the "Added" row.
        "added"    => "(SELECT MIN(created_at) FROM disc_submissions WHERE target_disc_id = d.id AND created_at != '1970-01-02 00:00:00+00')",
        "updated"  => "(SELECT MAX(created_at) FROM disc_submissions WHERE target_disc_id = d.id AND created_at != '1970-01-02 00:00:00+00')",
        _ => "LOWER(d.title)",
    };
    let sort_dir = match query.order.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };

    let sql_count = format!(
        "SELECT COUNT(*) FROM discs d JOIN systems s ON s.code = d.system_code WHERE {where_sql}"
    );
    let sql_select = format!(
        "SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix,
                d.title_foreign,
                s.has_title_foreign, s.has_disc_number, s.has_disc_title, s.has_edition, s.has_serial,
                s.code AS system_code,
                s.short_name AS system_short_name,
                array_to_string(d.serial, ', ') AS serial,
                d.version,
                array_to_string(d.edition, ', ') AS edition,
                d.enabled, d.questionable,
                (SELECT COUNT(*) FROM disc_dumpers dd WHERE dd.disc_id = d.id) AS dumper_count,
                {dumped_by_me_sql} AS dumped_by_me
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
    if !filter_region.is_empty() {
        count_query = count_query.bind(filter_region.clone());
        select_query = select_query.bind(filter_region.clone());
    }
    if filter_letter != "#" && filter_letter.len() == 1 && filter_letter.chars().next().unwrap().is_ascii_alphabetic() {
        count_query = count_query.bind(filter_letter.clone());
        select_query = select_query.bind(filter_letter.clone());
    }
    if !filter_q.is_empty() {
        count_query = count_query.bind(filter_q.clone());
        select_query = select_query.bind(filter_q.clone());
    }
    if let Some(dumper_id) = filter_dumper_id {
        count_query = count_query.bind(dumper_id);
        select_query = select_query.bind(dumper_id);
    }
    if let Some(current_user_id) = current_user_id {
        select_query = select_query.bind(current_user_id);
    }

    let total_count = count_query.fetch_one(&state.pool).await.unwrap_or(0);
    let total_pages = (total_count + PAGE_SIZE - 1) / PAGE_SIZE;

    let raw_rows: Vec<RawDiscRow> = select_query
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    if !filter_q.is_empty() && total_count == 1 {
        if let Some(row) = raw_rows.first() {
            return Redirect::to(&format!("/disc/{}/", row.id)).into_response();
        }
    }

    let mut discs = Vec::with_capacity(raw_rows.len());
    for r in raw_rows {
        let region_rows: Vec<LangRow> = sqlx::query_as(
            "SELECT r.flag_code AS code, r.name FROM disc_regions dr
             JOIN regions r ON r.code = dr.region_code
             WHERE dr.disc_id = $1 ORDER BY r.sort_order",
        )
        .bind(r.id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        let lang_rows: Vec<LangRow> = sqlx::query_as(
            "SELECT l.flag_code AS code, l.name FROM disc_languages dl
             JOIN languages l ON l.code = dl.language_code
             WHERE dl.disc_id = $1 ORDER BY l.sort_order",
        )
        .bind(r.id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        discs.push(DiscRow {
            id: r.id,
            title: format_display_title(
                &r.title,
                if r.has_disc_number { r.disc_number.as_deref() } else { None },
                if r.has_disc_title { r.disc_title.as_deref() } else { None },
                r.filename_suffix.as_deref(),
            ),
            title_foreign: if r.has_title_foreign {
                r.title_foreign.unwrap_or_default()
            } else {
                String::new()
            },
            system_display: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            system_code: r.system_code,
            dumped_by_me: r.dumped_by_me,
            version: r.version.unwrap_or_default(),
            edition_display: if r.has_edition {
                r.edition.unwrap_or_default()
            } else {
                String::new()
            },
            status_emoji: if r.enabled {
                DiscStatus::compute(r.questionable, r.dumper_count).emoji().to_string()
            } else {
                "🔴".to_string()
            },
            status_display: if r.enabled {
                DiscStatus::compute(r.questionable, r.dumper_count).to_string()
            } else {
                "Disabled".to_string()
            },
            region_flags: region_rows.into_iter().map(|r| RegionFlag {
                code: r.code.to_lowercase(),
                name: r.name,
            }).collect(),
            language_flags: lang_rows.into_iter().map(|l| LangFlag {
                code: l.code.to_lowercase(),
                name: l.name,
            }).collect(),
            serial: if r.has_serial {
                r.serial.unwrap_or_default()
            } else {
                String::new()
            },
        });
    }

    let is_asc = sort_order_str != "desc";
    let next_order = |col: &str| -> String {
        if sort_column == col && is_asc { "desc" } else { "asc" }.to_string()
    };

    Html(
        DiscsTemplate {
            current_user: user.user().map(|u| u.username.clone()),
            discs,
            systems,
            regions,
            dumpers,
            letters: LETTERS.iter().map(|s| (s.to_string(), filter_letter == *s)).collect(),
            filter_system,
            filter_region,
            filter_status,
            filter_letter,
            filter_q,
            filter_dumper,
            filter_dumper_name,
            total_count,
            page,
            total_pages,
            prev_page: page - 1,
            next_page: page + 1,
            sort_column: sort_column.clone(),
            sort_order: sort_order_str,
            next_region_order: next_order("region"),
            next_title_order: next_order("title"),
            next_system_order: next_order("system"),
            next_version_order: next_order("version"),
            next_edition_order: next_order("edition"),
            next_language_order: next_order("language"),
            next_serial_order: next_order("serial"),
            next_status_order: next_order("status"),
        }
        .render()
        .unwrap(),
    )
    .into_response()
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
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    title_foreign: Option<String>,
    has_title_foreign: bool,
    has_disc_number: bool,
    has_disc_title: bool,
    has_edition: bool,
    has_serial: bool,
    system_code: String,
    system_short_name: String,
    serial: Option<String>,
    version: Option<String>,
    edition: Option<String>,
    enabled: bool,
    questionable: bool,
    dumper_count: i64,
    dumped_by_me: bool,
}

#[derive(sqlx::FromRow)]
struct SystemDropdownRow {
    code: String,
    manufacturer: String,
    name: String,
}

#[derive(sqlx::FromRow)]
struct LangRow {
    code: String,
    name: String,
}

#[derive(sqlx::FromRow)]
struct DumperRow {
    id: i32,
    name: String,
}
