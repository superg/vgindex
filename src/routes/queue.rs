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
use crate::error::AppResult;
use crate::services::{disc_service, queue_service};
use crate::AppState;

use super::disc_edit::{
    self, build_category_options, build_check_options, build_flat_changes,
    build_lang_check_options, build_media_layers_json, build_media_options, build_system_options,
    build_systems_json, fetch_ref_data, max_layers_for_media, validate_form, DiscEditForm,
    DiscEditTemplate,
};

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

    let is_mod = user.role.can_moderate();
    let is_pending = sub.status == SubmissionStatus::Pending;
    let show_review_form = is_mod && is_pending;

    if !show_review_form {
        return render_readonly_detail(
            &user.username, &sub, &submitter_name, &reviewer_name,
        )
        .await;
    }

    let ref_data = fetch_ref_data(&state.pool).await?;
    let (systems_media_json, systems_has_offset_extra_json) =
        build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);

    let changed_fields: Vec<String> = sub
        .changes
        .as_object()
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    let is_diff_format = sub.target_disc_id.is_some()
        || sub
            .changes
            .as_object()
            .and_then(|o| o.values().next())
            .and_then(|v| v.get("new"))
            .is_some();

    if let Some(disc_id) = sub.target_disc_id {
        let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;

        let mut snapshot = disc_service::build_snapshot_from_disc(&detail);
        if is_diff_format {
            if let Some(diff_obj) = sub.changes.as_object() {
                if let Some(snap_obj) = snapshot.as_object_mut() {
                    for (field, change) in diff_obj {
                        if let Some(new_val) = change.get("new") {
                            snap_obj.insert(field.clone(), new_val.clone());
                        }
                    }
                }
            }
        }

        let system_code = snapshot["system_code"].as_str().unwrap_or(&detail.disc.system_code).to_string();
        let media_type_code = snapshot["media_type"].as_str().unwrap_or(detail.disc.media_type.code()).to_string();
        let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);

        let system = disc_service::get_system(&state.pool, &system_code).await.ok();
        let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

        let template = build_review_template(
            &user.username, &sub, &submitter_name, &reviewer_name, &changed_fields,
            &snapshot, &ref_data, &systems_media_json, &systems_has_offset_extra_json,
            &media_layers_json, &system_code, &media_type_code, max_layers, has_sys,
        );

        Ok(Html(template.render().unwrap()))
    } else {
        let snapshot = &sub.changes;
        let system_code = snapshot["system_code"].as_str().unwrap_or("").to_string();
        let media_type_code = snapshot["media_type"].as_str().unwrap_or("cd").to_string();
        let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);

        let system = if !system_code.is_empty() {
            disc_service::get_system(&state.pool, &system_code).await.ok()
        } else {
            None
        };
        let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

        let template = build_review_template(
            &user.username, &sub, &submitter_name, &reviewer_name, &changed_fields,
            snapshot, &ref_data, &systems_media_json, &systems_has_offset_extra_json,
            &media_layers_json, &system_code, &media_type_code, max_layers, has_sys,
        );

        Ok(Html(template.render().unwrap()))
    }
}

fn build_review_template(
    username: &str,
    sub: &DiscSubmission,
    submitter_name: &str,
    reviewer_name: &str,
    changed_fields: &[String],
    snapshot: &serde_json::Value,
    ref_data: &disc_edit::EditRefData,
    systems_media_json: &str,
    systems_has_offset_extra_json: &str,
    media_layers_json: &str,
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

    let edc_value = match &snapshot["edc"] {
        serde_json::Value::Bool(true) => "true".to_string(),
        serde_json::Value::Bool(false) => "false".to_string(),
        _ => String::new(),
    };

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
        systems_has_offset_extra_json: systems_has_offset_extra_json.to_string(),

        title: json_str("title"),
        show_title_foreign: has_sys(|s| s.has_title_foreign),
        title_foreign: json_opt_str("title_foreign"),
        show_disc_number: has_sys(|s| s.has_disc_number),
        disc_number: json_opt_str("disc_number"),
        show_disc_title: has_sys(|s| s.has_disc_title),
        disc_title: json_opt_str("disc_title"),
        filename_suffix: json_opt_str("filename_suffix"),

        show_serial: has_sys(|s| s.has_serial),
        serials: json_str_vec("serial"),
        show_version: has_sys(|s| s.has_version),
        version: json_opt_str("version"),
        show_edition: has_sys(|s| s.has_edition),
        editions: json_str_vec("edition"),
        show_barcode: has_sys(|s| s.has_barcode),
        barcodes: json_str_vec("barcode"),

        ring_codes_json,

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
        show_pic: has_sys(|s| s.has_pic),
        pic_hex: json_opt_str("pic"),
        show_bca: has_sys(|s| s.has_bca),
        bca_hex: json_opt_str("bca"),
        show_header: has_sys(|s| s.has_header),
        header_hex: json_opt_str("header"),

        show_protection: has_sys(|s| s.has_protection),
        protection: json_opt_str("protection"),
        show_sector_ranges: has_sys(|s| s.has_sector_ranges),
        sector_ranges_text,
        show_sbi: has_sys(|s| s.has_sbi),
        sbi: json_opt_str("sbi"),
        protection_key_disc_key,
        protection_key_disc_id,
        has_sample_start: has_sys(|s| s.has_sample_start),

        cue: json_opt_str("cue"),
        files_xml: json_opt_str("files_xml"),

        questionable,
        enabled,

        is_add_mode: false,
        dump_log: String::new(),
        extra_upload_url: String::new(),

        validation_errors: vec![],

        is_review_mode: true,
        changed_fields: changed_fields.to_vec(),
        submission_id: sub.id,
        submission_type_display: sub.submission_type.to_string(),
        submitter_name: submitter_name.to_string(),
        submitter_comment: sub.submitter_comment.clone().unwrap_or_default(),
        dump_log_display: sub.dump_log.clone().unwrap_or_default(),
        extra_upload_url_display: sub.extra_upload_url.clone().unwrap_or_default(),
        submission_status: sub.status.to_string(),
        reviewer_name: reviewer_name.to_string(),
        review_comment_display: sub.review_comment.clone().unwrap_or_default(),
        created_at_display: sub.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        reviewed_at_display: sub
            .reviewed_at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default(),
    }
}

// ── Read-only submission detail ────────────────────────────────────────

#[derive(Template)]
#[template(path = "queue_detail.html")]
struct QueueDetailTemplate {
    current_user: Option<String>,
    submission_id: i32,
    submission_type_display: String,
    submitter_name: String,
    submitter_comment: String,
    dump_log_display: String,
    extra_upload_url_display: String,
    submission_status: String,
    reviewer_name: String,
    review_comment_display: String,
    created_at_display: String,
    reviewed_at_display: String,
    target_disc_id: i32,
    changes_summary: Vec<ChangeSummaryRow>,
    is_diff_format: bool,
}
impl SiteConfig for QueueDetailTemplate {}

struct ChangeSummaryRow {
    field: String,
    old_value: String,
    new_value: String,
}

fn format_json_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Array(arr) => {
            arr.iter()
                .filter_map(|v| match v {
                    serde_json::Value::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn build_changes_summary(changes: &serde_json::Value, is_diff: bool) -> Vec<ChangeSummaryRow> {
    let obj = match changes.as_object() {
        Some(o) => o,
        None => return vec![],
    };

    let mut rows: Vec<ChangeSummaryRow> = obj
        .iter()
        .map(|(field, val)| {
            if is_diff {
                ChangeSummaryRow {
                    field: field.clone(),
                    old_value: format_json_value(&val["old"]),
                    new_value: format_json_value(&val["new"]),
                }
            } else {
                ChangeSummaryRow {
                    field: field.clone(),
                    old_value: String::new(),
                    new_value: format_json_value(val),
                }
            }
        })
        .collect();
    rows.sort_by(|a, b| a.field.cmp(&b.field));
    rows
}

async fn render_readonly_detail(
    username: &str,
    sub: &DiscSubmission,
    submitter_name: &str,
    reviewer_name: &str,
) -> AppResult<Html<String>> {
    let is_diff = sub
        .changes
        .as_object()
        .and_then(|o| o.values().next())
        .and_then(|v| v.get("new"))
        .is_some();

    let changes_summary = build_changes_summary(&sub.changes, is_diff);

    let template = QueueDetailTemplate {
        current_user: Some(username.to_string()),
        submission_id: sub.id,
        submission_type_display: sub.submission_type.to_string(),
        submitter_name: submitter_name.to_string(),
        submitter_comment: sub.submitter_comment.clone().unwrap_or_default(),
        dump_log_display: sub.dump_log.clone().unwrap_or_default(),
        extra_upload_url_display: sub.extra_upload_url.clone().unwrap_or_default(),
        submission_status: sub.status.to_string(),
        reviewer_name: reviewer_name.to_string(),
        review_comment_display: sub.review_comment.clone().unwrap_or_default(),
        created_at_display: sub.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        reviewed_at_display: sub
            .reviewed_at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default(),
        target_disc_id: sub.target_disc_id.unwrap_or(0),
        changes_summary,
        is_diff_format: is_diff,
    };

    Ok(Html(template.render().unwrap()))
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
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    if form.action == "reject" {
        sqlx::query(
            "UPDATE disc_submissions SET status = 'Rejected', reviewer_id = $1,
             review_comment = $2, reviewed_at = NOW()
             WHERE id = $3",
        )
        .bind(user.id)
        .bind(review_comment)
        .bind(id)
        .execute(&state.pool)
        .await?;

        return Ok(Redirect::to(&format!("/queue/{id}/")).into_response());
    }

    let errors = validate_form(&form.disc);
    if !errors.is_empty() {
        let submitter_name: String =
            sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
                .bind(sub.submitter_id)
                .fetch_one(&state.pool)
                .await
                .unwrap_or_else(|_| format!("User #{}", sub.submitter_id));

        let ref_data = fetch_ref_data(&state.pool).await?;
        let (systems_media_json, systems_has_offset_extra_json) =
            build_systems_json(&ref_data.all_systems);
        let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
        let changed_fields: Vec<String> = sub
            .changes
            .as_object()
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();
        let snapshot = build_flat_changes(&form.disc);
        let system_code = form.disc.system_code.clone();
        let media_type_code = form.disc.media_type.clone();
        let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);
        let system = disc_service::get_system(&state.pool, &system_code).await.ok();
        let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

        let mut template = build_review_template(
            &user.username, &sub, &submitter_name, "", &changed_fields,
            &snapshot, &ref_data, &systems_media_json, &systems_has_offset_extra_json,
            &media_layers_json, &system_code, &media_type_code, max_layers, has_sys,
        );
        template.validation_errors = errors;

        return Ok(Html(template.render().unwrap()).into_response());
    }

    let form_snapshot = build_flat_changes(&form.disc);

    match sub.submission_type {
        SubmissionType::Edit => {
            if let Some(disc_id) = sub.target_disc_id {
                disc_service::update_disc(&state.pool, disc_id, &form_snapshot).await?;
            }

            sqlx::query(
                "UPDATE disc_submissions SET status = 'Approved', reviewer_id = $1,
                 review_comment = $2, reviewed_at = NOW()
                 WHERE id = $3",
            )
            .bind(user.id)
            .bind(review_comment)
            .bind(id)
            .execute(&state.pool)
            .await?;
        }
        SubmissionType::Disc => {
            if let Some(disc_id) = sub.target_disc_id {
                disc_service::update_disc(&state.pool, disc_id, &form_snapshot).await?;

                sqlx::query(
                    "INSERT INTO disc_dumpers (disc_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                )
                .bind(disc_id)
                .bind(sub.submitter_id)
                .execute(&state.pool)
                .await?;

                sqlx::query(
                    "UPDATE disc_submissions SET status = 'Approved', reviewer_id = $1,
                     review_comment = $2, reviewed_at = NOW()
                     WHERE id = $3",
                )
                .bind(user.id)
                .bind(review_comment)
                .bind(id)
                .execute(&state.pool)
                .await?;
            } else {
                let disc_id = disc_service::create_disc_from_submission(
                    &state.pool,
                    &form_snapshot,
                    sub.submitter_id,
                )
                .await?;

                sqlx::query(
                    "UPDATE disc_submissions SET status = 'Approved', reviewer_id = $1,
                     review_comment = $2, reviewed_at = NOW(), target_disc_id = $3
                     WHERE id = $4",
                )
                .bind(user.id)
                .bind(review_comment)
                .bind(disc_id)
                .bind(id)
                .execute(&state.pool)
                .await?;
            }
        }
    }

    Ok(Redirect::to(&format!("/queue/{id}/")).into_response())
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
