use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use serde::Deserialize;

use crate::auth::middleware::{RequireAuth, RequireModerator};
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::{disc_service, queue_service};
use crate::AppState;

use super::disc_edit::{
    self, build_category_options, build_check_options, build_flat_changes,
    build_sparse_disc_changes, build_sparse_edit_changes,
    build_lang_check_options, build_media_has_pic_json, build_media_layers_json,
    build_media_options, build_media_rom_extensions_json, build_system_options,
    build_systems_json, fetch_ref_data, max_layers_for_media, validate_form,
    DiscEditForm, DiscEditTemplate,
};

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/queue", get(queue_list))
        .route("/queue/", get(queue_list))
        .route("/queue/{id}", get(submission_detail))
        .route("/queue/{id}/", get(submission_detail))
        .route("/queue/{id}/review", post(review_submit))
        .route("/queue/{id}/review/", post(review_submit))
}

#[derive(Deserialize, Default)]
pub struct QueueQuery {
    pub status: Option<String>,
    pub sub_type: Option<String>,
    pub system: Option<String>,
    pub submitter: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page: Option<i64>,
}

struct SystemOption {
    code: String,
    name: String,
    selected: bool,
}

struct SubmitterOption {
    id: i32,
    username: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "queue.html")]
struct QueueTemplate {
    current_user: Option<String>,
    is_moderator: bool,
    entries: Vec<SubmissionListRow>,
    systems: Vec<SystemOption>,
    submitters: Vec<SubmitterOption>,
    filter_status: String,
    filter_type: String,
    filter_system: String,
    filter_submitter: String,
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
    let filter_submitter = if is_mod { query.submitter.clone().unwrap_or_default() } else { String::new() };
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
    let submitter_for_query = if filter_submitter.is_empty() { None } else { Some(filter_submitter.as_str()) };

    let entries = queue_service::list_submissions(
        &state.pool,
        if is_mod { None } else { Some(user.id) },
        status_for_query,
        type_for_query,
        system_for_query,
        submitter_for_query,
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
        submitter_for_query,
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

    let submitters: Vec<SubmitterOption> = if is_mod {
        #[derive(sqlx::FromRow)]
        struct SubRow { id: i32, username: String }
        let sub_rows: Vec<SubRow> = sqlx::query_as(
            "SELECT id, username FROM users \
             WHERE id IN (SELECT DISTINCT submitter_id FROM disc_submissions) \
             ORDER BY LOWER(username)"
        )
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default();

        sub_rows.into_iter().map(|s| SubmitterOption {
            selected: s.username == filter_submitter,
            id: s.id,
            username: s.username,
        }).collect()
    } else {
        Vec::new()
    };

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
            submitters,
            filter_status: if filter_status.is_empty() { "Pending".to_string() } else { filter_status },
            filter_type,
            filter_system,
            filter_submitter,
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

// ── Submission detail (GET /queue/{id}/) ───────────────────────────────

async fn submission_detail(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let sub = queue_service::get_submission(&state.pool, id).await?;

    let is_mod = user.role.can_moderate();
    if !is_mod && sub.submitter_id != user.id {
        return Err(AppError::Forbidden);
    }

    let submitter_name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(sub.submitter_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or_else(|_| format!("User #{}", sub.submitter_id));

    let reviewer_name: String = if let Some(rid) = sub.reviewer_id {
        sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
            .bind(rid)
            .fetch_one(&state.pool)
            .await
            .unwrap_or_default()
    } else {
        String::new()
    };

    let is_pending = sub.status == SubmissionStatus::Pending;
    let show_review_form = is_mod && is_pending;

    if !show_review_form {
        return render_readonly_detail(
            &user.username, &sub, &submitter_name, &reviewer_name,
        )
        .await;
    }

    let ref_data = fetch_ref_data(&state.pool).await?;
    let (systems_media_json, systems_has_flags_json) =
        build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);

    let mut snapshot = sub.changes.clone();
    let mut db_snapshot: Option<serde_json::Value> = None;
    if let Some(disc_id) = sub.target_disc_id {
        let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;
        let current_db_snapshot = disc_service::build_snapshot_from_disc(&detail);
        snapshot = queue_service::resolve_submission_snapshot(
            sub.submission_type,
            &current_db_snapshot,
            &sub.changes,
        )?;
        db_snapshot = Some(current_db_snapshot);
    }

    let system_code = snapshot["system_code"].as_str().unwrap_or("").to_string();
    let media_type_code = snapshot["media_type"].as_str().unwrap_or("cd").to_string();
    let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);

    let system = if !system_code.is_empty() {
        disc_service::get_system(&state.pool, &system_code).await.ok()
    } else {
        None
    };
    let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

    let mut template = build_review_template(
        &user.username, &sub, &submitter_name, &reviewer_name,
        &snapshot, &ref_data, &systems_media_json, &systems_has_flags_json,
        &media_layers_json, &media_rom_extensions_json, &media_has_pic_json,
        &system_code, &media_type_code, max_layers, has_sys,
    );

    if let Some(db_snapshot) = db_snapshot {
        let highlights = compute_field_highlights(&snapshot, &db_snapshot);
        apply_highlights(&mut template, highlights);
    }

    Ok(Html(template.render().unwrap()))
}

fn build_review_template(
    username: &str,
    sub: &DiscSubmission,
    submitter_name: &str,
    reviewer_name: &str,
    snapshot: &serde_json::Value,
    ref_data: &disc_edit::EditRefData,
    systems_media_json: &str,
    systems_has_flags_json: &str,
    media_layers_json: &str,
    media_rom_extensions_json: &str,
    media_has_pic_json: &str,
    system_code: &str,
    media_type_code: &str,
    max_layers: u32,
    has_sys: impl Fn(fn(&System) -> bool) -> bool,
) -> DiscEditTemplate {
    let json_str = |key: &str| snapshot[key].as_str().unwrap_or("").to_string();
    let json_opt_str = |key: &str| {
        match &snapshot[key] {
            serde_json::Value::String(s) => s.clone(),
            _ => String::new(),
        }
    };
    let json_str_vec = |key: &str| -> Vec<String> {
        snapshot[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default()
    };

    let regions_codes = json_str_vec("regions");
    let languages_codes = json_str_vec("languages");

    let ring_codes_json = snapshot
        .get("ring_codes")
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".into()))
        .unwrap_or_else(|| "[]".into());

    let layerbreaks: Vec<String> = snapshot["layerbreaks"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_i64()).map(|v| v.to_string()).collect())
        .unwrap_or_default();

    let error_count = match &snapshot["error_count"] {
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    };

    let edc_value = snapshot["edc"].as_bool().unwrap_or(false);

    let keys = json_str_vec("keys");
    let protection_key_disc_key = keys.first().cloned().unwrap_or_default();
    let protection_key_disc_id = keys.get(1).cloned().unwrap_or_default();

    let questionable = snapshot["questionable"].as_bool().unwrap_or(false);
    let enabled = snapshot["enabled"].as_bool().unwrap_or(true);

    let sector_ranges_text = snapshot["sector_ranges"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    format!(
                        "{}-{}",
                        r["start"].as_i64().unwrap_or(0),
                        r["end"].as_i64().unwrap_or(0)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let page_title = format_display_title(
        snapshot["title"].as_str().unwrap_or(""),
        snapshot["disc_number"].as_str(),
        snapshot["disc_title"].as_str(),
        snapshot["filename_suffix"].as_str(),
    );

    DiscEditTemplate {
        current_user: Some(username.to_string()),
        disc_id: sub.target_disc_id.unwrap_or(0),
        page_title,

        systems: build_system_options(&ref_data.all_systems, system_code),
        media_types_all: build_media_options(&ref_data.all_media_types, media_type_code),
        categories: build_category_options(
            &ref_data.all_categories,
            snapshot["category"].as_str().unwrap_or("Games"),
        ),
        regions: build_check_options(&ref_data.all_regions, &regions_codes),
        languages: build_lang_check_options(&ref_data.all_languages, &languages_codes),

        system_code: system_code.to_string(),
        media_type_code: media_type_code.to_string(),
        max_layers,
        media_layers_json: media_layers_json.to_string(),
        systems_media_json: systems_media_json.to_string(),
        systems_has_flags_json: systems_has_flags_json.to_string(),
        media_rom_extensions_json: media_rom_extensions_json.to_string(),

        title: json_str("title"),
        show_title_foreign: has_sys(|s| s.has_title_foreign),
        title_foreign: json_opt_str("title_foreign"),
        show_disc_number: has_sys(|s| s.has_disc_number),
        disc_number: json_opt_str("disc_number"),
        show_disc_title: has_sys(|s| s.has_disc_title),
        disc_title: json_opt_str("disc_title"),
        filename_suffix: json_opt_str("filename_suffix"),

        show_serial: has_sys(|s| s.has_serial),
        serials: json_str_vec("serial").into_iter()
            .map(|s| disc_edit::HighlightedValue { value: s, highlight: String::new() })
            .collect(),
        show_version: has_sys(|s| s.has_version),
        version: json_opt_str("version"),
        show_edition: has_sys(|s| s.has_edition),
        editions: json_str_vec("edition").into_iter()
            .map(|s| disc_edit::HighlightedValue { value: s, highlight: String::new() })
            .collect(),
        show_barcode: has_sys(|s| s.has_barcode),
        barcodes: json_str_vec("barcode").into_iter()
            .map(|s| disc_edit::HighlightedValue { value: s, highlight: String::new() })
            .collect(),

        ring_codes_json,
        ring_highlights_json: "[]".to_string(),

        comments: json_opt_str("comments"),
        contents: json_opt_str("contents"),

        show_error_count: has_sys(|s| s.has_error_count),
        error_count,
        show_exe_date: has_sys(|s| s.has_exe_date),
        exe_date: json_opt_str("exe_date"),
        show_edc: has_sys(|s| s.has_edc),
        edc_value,

        layerbreaks,
        show_pvd: has_sys(|s| s.has_pvd),
        pvd_hex: json_opt_str("pvd"),
        show_pic: ref_data.all_media_types.iter()
            .find(|m| m.code == media_type_code)
            .map_or(false, |m| m.pic),
        media_has_pic_json: media_has_pic_json.to_string(),
        pic_hex: json_opt_str("pic"),
        show_bca: has_sys(|s| s.has_bca),
        bca_hex: json_opt_str("bca"),
        show_header: has_sys(|s| s.has_header),
        header_hex: json_opt_str("header"),

        show_keys: has_sys(|s| s.has_keys),
        show_protection: has_sys(|s| s.has_protection),
        protection: json_opt_str("protection"),
        show_sector_ranges: has_sys(|s| s.has_sector_ranges),
        sector_ranges_text,
        show_sbi: has_sys(|s| s.has_sbi),
        sbi: json_opt_str("sbi"),
        protection_key_disc_key,
        protection_key_disc_id,
        has_sample_start: has_sys(|s| s.has_sample_start),

        cue: json_opt_str("cuesheet"),
        files_xml: json_opt_str("dat"),

        questionable,
        enabled,

        is_add_mode: false,
        dump_log: String::new(),
        dump_log_required: false,
        extra_upload_url: String::new(),

        submit_button_text: String::new(),
        validation_errors: vec![],

        is_review_mode: true,
        changed_fields: vec![],
        submission_id: sub.id,
        submission_type_display: sub.submission_type.to_string(),
        submitter_id: sub.submitter_id,
        submitter_name: submitter_name.to_string(),
        submission_comment: sub.submission_comment.clone().unwrap_or_default(),
        dump_log_display: sub.dump_log.clone().unwrap_or_default(),
        extra_upload_url_display: sub.extra_upload_url.clone().unwrap_or_default(),
        submission_status: sub.status.to_string(),
        reviewer_id: sub.reviewer_id.unwrap_or(0),
        reviewer_name: reviewer_name.to_string(),
        review_comment_display: sub.review_comment.clone().unwrap_or_default(),
        created_at_display: sub.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        reviewed_at_display: sub
            .reviewed_at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default(),
        changes_json: serde_json::to_string_pretty(&sub.changes).unwrap_or_default(),
    }
}

// ── Read-only submission detail ────────────────────────────────────────

#[derive(Template)]
#[template(path = "queue_detail.html")]
struct QueueDetailTemplate {
    current_user: Option<String>,
    submission_id: i32,
    submission_type_display: String,
    submitter_id: i32,
    submitter_name: String,
    submission_comment: String,
    dump_log_display: String,
    extra_upload_url_display: String,
    submission_status: String,
    reviewer_id: i32,
    reviewer_name: String,
    review_comment_display: String,
    created_at_display: String,
    reviewed_at_display: String,
    target_disc_id: i32,
    changes_json: String,
}
impl SiteConfig for QueueDetailTemplate {}

async fn render_readonly_detail(
    username: &str,
    sub: &DiscSubmission,
    submitter_name: &str,
    reviewer_name: &str,
) -> AppResult<Html<String>> {
    let template = QueueDetailTemplate {
        current_user: Some(username.to_string()),
        submission_id: sub.id,
        submission_type_display: sub.submission_type.to_string(),
        submitter_id: sub.submitter_id,
        submitter_name: submitter_name.to_string(),
        submission_comment: sub.submission_comment.clone().unwrap_or_default(),
        dump_log_display: sub.dump_log.clone().unwrap_or_default(),
        extra_upload_url_display: sub.extra_upload_url.clone().unwrap_or_default(),
        submission_status: sub.status.to_string(),
        reviewer_id: sub.reviewer_id.unwrap_or(0),
        reviewer_name: reviewer_name.to_string(),
        review_comment_display: sub.review_comment.clone().unwrap_or_default(),
        created_at_display: sub.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        reviewed_at_display: sub
            .reviewed_at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default(),
        target_disc_id: sub.target_disc_id.unwrap_or(0),
        changes_json: serde_json::to_string_pretty(&sub.changes).unwrap_or_default(),
    };

    Ok(Html(template.render().unwrap()))
}

// ── Diff highlighting ──────────────────────────────────────────────────

struct FieldHighlights {
    changed_fields: Vec<String>,
    region_highlights: std::collections::HashMap<String, String>,
    language_highlights: std::collections::HashMap<String, String>,
    serial_highlights: std::collections::HashMap<String, String>,
    edition_highlights: std::collections::HashMap<String, String>,
    barcode_highlights: std::collections::HashMap<String, String>,
    ring_highlights_json: String,
}

fn compute_field_highlights(
    changes: &serde_json::Value,
    db_snapshot: &serde_json::Value,
) -> FieldHighlights {
    let mut changed_fields = Vec::new();

    let simple_fields = [
        "system_code", "media_type", "category", "title", "title_foreign",
        "disc_number", "disc_title", "filename_suffix", "version",
        "error_count", "exe_date", "edc", "comments", "contents",
        "protection", "sector_ranges", "sbi", "pvd", "header", "bca",
        "pic", "cuesheet", "dat", "enabled", "questionable",
    ];

    let is_empty_val = |v: &serde_json::Value| -> bool {
        match v {
            serde_json::Value::Null => true,
            serde_json::Value::String(s) => s.trim().is_empty(),
            serde_json::Value::Bool(_) => false,
            serde_json::Value::Number(_) => false,
            serde_json::Value::Array(a) => a.is_empty(),
            serde_json::Value::Object(o) => o.is_empty(),
        }
    };

    let vals_equal = |a: &serde_json::Value, b: &serde_json::Value| -> bool {
        if a == b {
            return true;
        }
        match (a, b) {
            (serde_json::Value::String(sa), serde_json::Value::String(sb)) => {
                sa.trim().replace("\r\n", "\n") == sb.trim().replace("\r\n", "\n")
            }
            _ => false,
        }
    };

    for field in &simple_fields {
        let db_val = &db_snapshot[*field];
        let ch_val = &changes[*field];
        let db_empty = is_empty_val(db_val);
        let ch_empty = is_empty_val(ch_val);

        if db_empty && ch_empty {
            continue;
        } else if db_empty && !ch_empty {
            changed_fields.push(format!("{}:added", field));
        } else if !db_empty && ch_empty {
            changed_fields.push(format!("{}:removed", field));
        } else if !vals_equal(db_val, ch_val) {
            changed_fields.push(format!("{}:changed", field));
        }
    }

    let layerbreaks_field = "layerbreaks";
    let db_lb = &db_snapshot[layerbreaks_field];
    let ch_lb = &changes[layerbreaks_field];
    if db_lb != ch_lb {
        let db_lb_empty = db_lb.as_array().map_or(true, |a| a.is_empty());
        let ch_lb_empty = ch_lb.as_array().map_or(true, |a| a.is_empty());
        if db_lb_empty && !ch_lb_empty {
            changed_fields.push(format!("{}:added", layerbreaks_field));
        } else if !db_lb_empty && ch_lb_empty {
            changed_fields.push(format!("{}:removed", layerbreaks_field));
        } else {
            changed_fields.push(format!("{}:changed", layerbreaks_field));
        }
    }

    let keys_field = "keys";
    let db_keys = &db_snapshot[keys_field];
    let ch_keys = &changes[keys_field];
    if db_keys != ch_keys {
        let db_keys_empty = db_keys.as_array().map_or(true, |a| a.iter().all(|v| is_empty_val(v)));
        let ch_keys_empty = ch_keys.as_array().map_or(true, |a| a.iter().all(|v| is_empty_val(v)));
        if db_keys_empty && !ch_keys_empty {
            changed_fields.push(format!("{}:added", keys_field));
        } else if !db_keys_empty && ch_keys_empty {
            changed_fields.push(format!("{}:removed", keys_field));
        } else {
            changed_fields.push(format!("{}:changed", keys_field));
        }
    }

    let str_set = |v: &serde_json::Value| -> std::collections::HashSet<String> {
        v.as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default()
    };

    let mut region_highlights = std::collections::HashMap::new();
    let db_regions = str_set(&db_snapshot["regions"]);
    let ch_regions = str_set(&changes["regions"]);
    for code in &ch_regions {
        if !db_regions.contains(code) {
            region_highlights.insert(code.clone(), "added".to_string());
        }
    }
    for code in &db_regions {
        if !ch_regions.contains(code) {
            region_highlights.insert(code.clone(), "removed".to_string());
        }
    }
    let mut language_highlights = std::collections::HashMap::new();
    let db_langs = str_set(&db_snapshot["languages"]);
    let ch_langs = str_set(&changes["languages"]);
    for code in &ch_langs {
        if !db_langs.contains(code) {
            language_highlights.insert(code.clone(), "added".to_string());
        }
    }
    for code in &db_langs {
        if !ch_langs.contains(code) {
            language_highlights.insert(code.clone(), "removed".to_string());
        }
    }
    if !region_highlights.is_empty() {
        changed_fields.push("regions:changed".to_string());
    }
    if !language_highlights.is_empty() {
        changed_fields.push("languages:changed".to_string());
    }

    let mut serial_highlights = std::collections::HashMap::new();
    let db_serials = str_set(&db_snapshot["serial"]);
    let ch_serials = str_set(&changes["serial"]);
    for s in &ch_serials {
        if !db_serials.contains(s) {
            serial_highlights.insert(s.clone(), "added".to_string());
        }
    }
    if !serial_highlights.is_empty() {
        changed_fields.push("serial:changed".to_string());
    }

    let mut edition_highlights = std::collections::HashMap::new();
    let db_editions = str_set(&db_snapshot["edition"]);
    let ch_editions = str_set(&changes["edition"]);
    for s in &ch_editions {
        if !db_editions.contains(s) {
            edition_highlights.insert(s.clone(), "added".to_string());
        }
    }
    if !edition_highlights.is_empty() {
        changed_fields.push("edition:changed".to_string());
    }

    let mut barcode_highlights = std::collections::HashMap::new();
    let db_barcodes = str_set(&db_snapshot["barcode"]);
    let ch_barcodes = str_set(&changes["barcode"]);
    for s in &ch_barcodes {
        if !db_barcodes.contains(s) {
            barcode_highlights.insert(s.clone(), "added".to_string());
        }
    }
    if !barcode_highlights.is_empty() {
        changed_fields.push("barcode:changed".to_string());
    }

    let mut ring_highlights: Vec<String> = Vec::new();
    let db_rings = db_snapshot["ring_codes"].as_array();
    let ch_rings = changes["ring_codes"].as_array();
    if let Some(ch_arr) = ch_rings {
        let mut db_arr = db_rings.cloned().unwrap_or_default();
        let mut ch_arr_sorted = ch_arr.clone();
        let max_layers = db_arr
            .iter()
            .chain(ch_arr_sorted.iter())
            .map(|e| e["layers"].as_array().map(|a| a.len()).unwrap_or(0))
            .max()
            .unwrap_or(0);
        disc_service::sort_ring_codes_json(&mut db_arr, max_layers);
        disc_service::sort_ring_codes_json(&mut ch_arr_sorted, max_layers);

        for (idx, ch_entry) in ch_arr_sorted.iter().enumerate() {
            if idx >= db_arr.len() {
                ring_highlights.push("added".to_string());
            } else if db_arr[idx] != *ch_entry {
                ring_highlights.push("changed".to_string());
            } else {
                ring_highlights.push(String::new());
            }
        }
        // Ring codes: only individual entry highlighting via ring_highlights_json,
        // no fieldset-level highlight.
    }
    let ring_highlights_json = serde_json::to_string(&ring_highlights).unwrap_or_else(|_| "[]".to_string());

    FieldHighlights {
        changed_fields,
        region_highlights,
        language_highlights,
        serial_highlights,
        edition_highlights,
        barcode_highlights,
        ring_highlights_json,
    }
}

fn apply_highlights(template: &mut DiscEditTemplate, highlights: FieldHighlights) {
    template.changed_fields = highlights.changed_fields;
    template.ring_highlights_json = highlights.ring_highlights_json;

    for opt in &mut template.regions {
        if let Some(hl) = highlights.region_highlights.get(&opt.value) {
            opt.highlight = hl.clone();
        }
    }
    for opt in &mut template.languages {
        if let Some(hl) = highlights.language_highlights.get(&opt.value) {
            opt.highlight = hl.clone();
        }
    }
    for item in &mut template.serials {
        if let Some(hl) = highlights.serial_highlights.get(&item.value) {
            item.highlight = hl.clone();
        }
    }
    for item in &mut template.editions {
        if let Some(hl) = highlights.edition_highlights.get(&item.value) {
            item.highlight = hl.clone();
        }
    }
    for item in &mut template.barcodes {
        if let Some(hl) = highlights.barcode_highlights.get(&item.value) {
            item.highlight = hl.clone();
        }
    }
}

// ── Review submission (POST /queue/{id}/review/) ───────────────────────

#[derive(Deserialize)]
pub struct ReviewForm {
    pub action: String,
    pub review_comment: Option<String>,
    #[serde(flatten)]
    pub disc: DiscEditForm,
}

async fn review_submit(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Path(id): Path<i32>,
    Form(form): Form<ReviewForm>,
) -> AppResult<Response> {
    let sub = queue_service::get_submission(&state.pool, id).await?;

    if sub.status != SubmissionStatus::Pending {
        return Ok(Redirect::to(&format!("/queue/{id}/")).into_response());
    }

    let review_comment = form
        .review_comment
        .as_deref()
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if form.action == "reject" {
        let rejected = queue_service::reject_submission(
            &state.pool, id, user.id, review_comment.as_deref(),
        ).await?;

        if !rejected {
            return Ok(Redirect::to(&format!("/queue/{id}/")).into_response());
        }
        return Ok(Redirect::to("/queue/").into_response());
    }

    let ref_data = fetch_ref_data(&state.pool).await?;
    let errors = validate_form(&form.disc, &ref_data.all_media_types);
    if !errors.is_empty() {
        let submitter_name: String =
            sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
                .bind(sub.submitter_id)
                .fetch_one(&state.pool)
                .await
                .unwrap_or_else(|_| format!("User #{}", sub.submitter_id));

        let (systems_media_json, systems_has_flags_json) =
            build_systems_json(&ref_data.all_systems);
        let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
        let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
        let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
        let snapshot = build_flat_changes(&form.disc, &ref_data.all_media_types);
        let system_code = form.disc.system_code.clone();
        let media_type_code = form.disc.media_type.clone();
        let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);
        let system = disc_service::get_system(&state.pool, &system_code).await.ok();
        let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

        let mut template = build_review_template(
            &user.username, &sub, &submitter_name, "",
            &snapshot, &ref_data, &systems_media_json, &systems_has_flags_json,
            &media_layers_json, &media_rom_extensions_json, &media_has_pic_json,
            &system_code, &media_type_code, max_layers, has_sys,
        );
        template.validation_errors = errors;

        return Ok(Html(template.render().unwrap()).into_response());
    }

    let (form_snapshot, is_sparse_changes) = if let Some(disc_id) = sub.target_disc_id {
        let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;
        let sparse = match sub.submission_type {
            SubmissionType::Edit => {
                build_sparse_edit_changes(&form.disc, &detail, &ref_data.all_media_types)
            }
            SubmissionType::Disc => {
                build_sparse_disc_changes(&form.disc, &detail, &ref_data.all_media_types)
            }
        };
        (sparse, true)
    } else {
        // New-disc create has no baseline to diff against.
        (build_flat_changes(&form.disc, &ref_data.all_media_types), false)
    };

    let approved = queue_service::approve_submission(
        &state.pool,
        &sub,
        &form_snapshot,
        is_sparse_changes,
        user.id,
        review_comment.as_deref(),
    )
    .await?;

    match approved {
        Some(_) => Ok(Redirect::to("/queue/").into_response()),
        None => Ok(Redirect::to(&format!("/queue/{id}/")).into_response()),
    }
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for SubmissionListRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            id: row.try_get("id")?,
            submission_type: row.try_get("submission_type")?,
            title: row.try_get("title")?,
            system_code: row.try_get("system_code")?,
            submitter: row.try_get("submitter")?,
            submitter_id: row.try_get("submitter_id")?,
            reviewer: row.try_get("reviewer")?,
            reviewer_id: row.try_get("reviewer_id")?,
            status: row.try_get("status")?,
            target_disc_id: row.try_get("target_disc_id")?,
            created_at: row.try_get("created_at")?,
        })
    }
}
