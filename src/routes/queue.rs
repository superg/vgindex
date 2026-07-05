use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use serde::Deserialize;

use crate::auth::{
    csrf,
    middleware::{AuthenticatedUser, RequireAuth},
};
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::{disc_service, queue_service};
use crate::AppState;

use super::compact_query_url;

use super::disc_edit::{
    self, approval_conflicts_to_linked_validation_errors, build_category_options,
    build_check_options, build_flat_changes, build_lang_check_options, build_media_has_pic_json,
    build_media_is_cd_json, build_media_layers_json, build_media_options,
    build_media_rom_extensions_json, build_new_disc_changes, build_sparse_edit_changes,
    build_system_options, build_systems_json, fetch_ref_data, max_layers_for_media, validate_form,
    DiscEditForm, DiscEditTemplate, ReviewAnnotation, ReviewOldMultiline,
};

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/queue", get(queue_list))
        .route("/queue/{id}", get(submission_detail))
        .route("/queue/{id}/review", post(review_submit))
}

#[derive(Deserialize, Default)]
pub struct QueueQuery {
    pub status: Option<String>,
    pub sub_type: Option<String>,
    pub system: Option<String>,
    pub submitter: Option<String>,
    pub disc_id: Option<i32>,
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
    username: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "queue.html")]
struct QueueTemplate {
    current_user: Option<AuthenticatedUser>,
    current_user_id: Option<i32>,
    page_title: String,
    is_disc_history: bool,
    can_view_all_statuses: bool,
    entries: Vec<SubmissionListRow>,
    systems: Vec<SystemOption>,
    submitters: Vec<SubmitterOption>,
    filter_disc_id: String,
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
    next_disc_id_order: String,
    next_status_order: String,
}
impl SiteConfig for QueueTemplate {}
impl QueueTemplate {
    fn url(&self, page: i64, sort: &str, order: &str) -> String {
        build_queue_url(QueueUrlOptions {
            status: &self.filter_status,
            sub_type: &self.filter_type,
            system: &self.filter_system,
            submitter: &self.filter_submitter,
            disc_id: &self.filter_disc_id,
            sort,
            order,
            page,
            is_disc_history: self.is_disc_history,
        })
    }

    fn page_url(&self, page: &i64) -> String {
        self.url(*page, &self.sort_column, &self.sort_order)
    }

    fn sort_url(&self, sort: &str, order: &str) -> String {
        self.url(1, sort, order)
    }

    fn can_open_entry(&self, entry: &SubmissionListRow) -> bool {
        self.current_user_id.is_some()
            || matches!(
                entry.status,
                SubmissionStatus::Approved | SubmissionStatus::Legacy
            )
    }

    fn type_icon_label(&self, entry: &SubmissionListRow) -> &'static str {
        queue_type_icon_label(entry)
    }

    fn type_icon_class(&self, entry: &SubmissionListRow) -> &'static str {
        queue_type_icon_class(entry)
    }
}

#[derive(Default)]
struct QueueUrlOptions<'a> {
    status: &'a str,
    sub_type: &'a str,
    system: &'a str,
    submitter: &'a str,
    disc_id: &'a str,
    sort: &'a str,
    order: &'a str,
    page: i64,
    is_disc_history: bool,
}

fn build_queue_url(options: QueueUrlOptions<'_>) -> String {
    let page = (options.page > 1)
        .then(|| options.page.to_string())
        .unwrap_or_default();
    let status = (!options.is_disc_history && options.status != "Pending")
        .then_some(options.status)
        .unwrap_or_default();
    let sort = (options.sort != "date")
        .then_some(options.sort)
        .unwrap_or_default();
    let order = (options.order != "desc")
        .then_some(options.order)
        .unwrap_or_default();

    compact_query_url(
        "/queue",
        &[
            ("status", status),
            ("sub_type", options.sub_type),
            ("system", options.system),
            ("submitter", options.submitter),
            ("disc_id", options.disc_id),
            ("sort", sort),
            ("order", order),
            ("page", &page),
        ],
    )
}

fn queue_type_icon_label(entry: &SubmissionListRow) -> &'static str {
    entry.display_kind.label()
}

fn queue_type_icon_class(entry: &SubmissionListRow) -> &'static str {
    match (entry.display_kind, entry.status) {
        (SubmissionDisplayKind::Edit, SubmissionStatus::Legacy) => {
            "submission-type-icon submission-type-icon-edit submission-type-icon-processed"
        }
        (
            SubmissionDisplayKind::NewDisc | SubmissionDisplayKind::Verification,
            SubmissionStatus::Legacy,
        ) => "submission-type-icon submission-type-icon-disc submission-type-icon-processed",
        (SubmissionDisplayKind::Edit, _) => {
            "submission-type-icon submission-type-icon-edit submission-type-icon-edit-pending"
        }
        (SubmissionDisplayKind::NewDisc, _) => {
            "submission-type-icon submission-type-icon-disc submission-type-icon-new-disc"
        }
        (SubmissionDisplayKind::Verification, _) => {
            "submission-type-icon submission-type-icon-disc submission-type-icon-verification"
        }
    }
}

#[derive(sqlx::FromRow)]
struct SysRow {
    code: String,
    manufacturer: String,
    name: String,
}

const PAGE_SIZE: i64 = 50;
pub(crate) const COMMENTS_REVIEW_DELIMITER: &str =
    "--- REVIEW NEW COMMENTS BELOW - REMOVE THIS LINE BEFORE APPROVING ---";

fn can_review_submission(user: &AuthenticatedUser, sub: &DiscSubmission) -> bool {
    user.role.can_moderate()
        || (user.role.can_edit_directly()
            && sub.submission_type == SubmissionType::Disc
            && sub.target_disc_id.is_some()
            && sub.submitter_id == user.id)
}

fn normalize_queue_type_filter(sub_type: Option<&str>, is_disc_history: bool) -> String {
    if is_disc_history {
        return String::new();
    }

    match sub_type
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" => String::new(),
        "edit" => "Edit".to_string(),
        "new disc" => "New Disc".to_string(),
        "verification" => "Verification".to_string(),
        _ => String::new(),
    }
}

fn normalize_queue_status_filter(status: Option<&str>, is_disc_history: bool) -> String {
    if is_disc_history {
        return "All Visible".to_string();
    }

    match status
        .unwrap_or("Pending")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "all statuses" => "All Statuses".to_string(),
        "pending" => "Pending".to_string(),
        "approved" => "Approved".to_string(),
        "rejected" => "Rejected".to_string(),
        "legacy" => "Legacy".to_string(),
        _ => "Pending".to_string(),
    }
}

fn normalize_queue_sort(sort: Option<&str>) -> String {
    match sort.unwrap_or("date").trim().to_ascii_lowercase().as_str() {
        "date" | "title" | "disc_id" | "system" | "submitter" | "reviewer" | "type" | "status" => {
            sort.unwrap_or("date").trim().to_ascii_lowercase()
        }
        _ => "date".to_string(),
    }
}

fn normalize_queue_order(order: Option<&str>) -> String {
    if order.unwrap_or("desc").trim().eq_ignore_ascii_case("asc") {
        "asc".to_string()
    } else {
        "desc".to_string()
    }
}

async fn queue_list(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<QueueQuery>,
) -> AppResult<Html<String>> {
    let current = Some(&user);
    let is_logged_in = true;
    let can_view_disabled_discs = user.role.can_view_disabled_discs();
    let disc_id_filter = query.disc_id;
    let is_disc_history = disc_id_filter.is_some();
    let can_view_all_statuses = is_logged_in && !is_disc_history;

    if let Some(disc_id) = disc_id_filter {
        disc_service::ensure_disc_id_visible(&state.pool, disc_id, can_view_disabled_discs).await?;
    }

    let page = query.page.unwrap_or(1).max(1);
    let filter_status = normalize_queue_status_filter(query.status.as_deref(), is_disc_history);
    let filter_type = normalize_queue_type_filter(query.sub_type.as_deref(), is_disc_history);
    let filter_system = if is_disc_history {
        String::new()
    } else {
        query
            .system
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase()
    };
    let filter_submitter = if is_logged_in && !is_disc_history {
        query.submitter.clone().unwrap_or_default()
    } else {
        String::new()
    };
    let sort_column = normalize_queue_sort(query.sort.as_deref());
    let sort_order = normalize_queue_order(query.order.as_deref());

    let status_for_query = if is_disc_history || filter_status == "All Statuses" {
        None
    } else {
        Some(filter_status.as_str())
    };

    let type_for_query = if filter_type.is_empty() {
        None
    } else {
        Some(filter_type.as_str())
    };
    let system_for_query = if filter_system.is_empty() {
        None
    } else {
        Some(filter_system.as_str())
    };
    let submitter_for_query = if filter_submitter.is_empty() {
        None
    } else {
        Some(filter_submitter.as_str())
    };

    let entries = queue_service::list_submissions(
        &state.pool,
        None,
        disc_id_filter,
        is_disc_history,
        !can_view_disabled_discs,
        status_for_query,
        type_for_query,
        system_for_query,
        submitter_for_query,
        &sort_column,
        &sort_order,
        page,
        PAGE_SIZE,
    )
    .await?;

    let total_count = queue_service::count_submissions(
        &state.pool,
        None,
        disc_id_filter,
        is_disc_history,
        !can_view_disabled_discs,
        status_for_query,
        type_for_query,
        system_for_query,
        submitter_for_query,
    )
    .await?;

    let total_pages = (total_count + PAGE_SIZE - 1) / PAGE_SIZE;

    let sys_rows: Vec<SysRow> = sqlx::query_as(
        "SELECT code, manufacturer, name FROM systems
         ORDER BY LOWER(CONCAT_WS(' ', NULLIF(manufacturer, ''), name))",
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

    let submitters: Vec<SubmitterOption> = if is_logged_in && !is_disc_history {
        #[derive(sqlx::FromRow)]
        struct SubRow {
            username: String,
        }
        let sub_rows: Vec<SubRow> = sqlx::query_as(
            "SELECT username FROM users \
             WHERE id IN (SELECT DISTINCT submitter_id FROM disc_submissions) \
             ORDER BY LOWER(username)",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        sub_rows
            .into_iter()
            .map(|s| SubmitterOption {
                selected: s.username == filter_submitter,
                username: s.username,
            })
            .collect()
    } else {
        Vec::new()
    };

    let is_asc = sort_order != "desc";
    let next_order = |col: &str| -> String {
        if sort_column == col && is_asc {
            "desc"
        } else {
            "asc"
        }
        .to_string()
    };

    Ok(Html(
        QueueTemplate {
            current_user: current.cloned(),
            current_user_id: current.map(|u| u.id),
            page_title: disc_id_filter
                .map(|disc_id| format!("History: Disc #{disc_id}"))
                .unwrap_or_else(|| "Queue".to_string()),
            is_disc_history,
            can_view_all_statuses,
            entries,
            systems,
            submitters,
            filter_disc_id: disc_id_filter
                .map(|disc_id| disc_id.to_string())
                .unwrap_or_default(),
            filter_status,
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
            next_disc_id_order: next_order("disc_id"),
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
    let current = Some(&user);

    let sub = queue_service::get_submission(&state.pool, id).await?;

    if let Some(disc_id) = sub.target_disc_id {
        disc_service::ensure_disc_id_visible(
            &state.pool,
            disc_id,
            user.role.can_view_disabled_discs(),
        )
        .await?;
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
    let show_review_form = current.is_some_and(|u| can_review_submission(u, &sub)) && is_pending;

    if !show_review_form {
        return render_readonly_detail(current.cloned(), &sub, &submitter_name, &reviewer_name)
            .await;
    }

    let current_user = current.cloned().ok_or(AppError::Unauthorized)?;

    let ref_data = fetch_ref_data(&state.pool).await?;
    let (systems_media_json, systems_has_flags_json) = build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_is_cd_json = build_media_is_cd_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
    let edition_suggestions_json = disc_edit::build_edition_suggestions_json(&state).await?;

    let snapshot: serde_json::Value;
    let mut db_snapshot: Option<serde_json::Value> = None;
    if let Some(disc_id) = sub.target_disc_id {
        let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;
        let current_db_snapshot = disc_service::build_snapshot_from_disc(&detail);
        snapshot =
            queue_service::resolve_submission_snapshot_for_submission(&current_db_snapshot, &sub)?;
        db_snapshot = Some(current_db_snapshot);
    } else {
        snapshot = queue_service::resolve_submission_snapshot_for_submission(
            &serde_json::json!({}),
            &sub,
        )?;
    }

    let system_code = snapshot["system_code"].as_str().unwrap_or("").to_string();
    let media_type_code = snapshot["media_type"].as_str().unwrap_or("cd").to_string();
    let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);

    let system = if !system_code.is_empty() {
        disc_service::get_system(&state.pool, &system_code)
            .await
            .ok()
    } else {
        None
    };
    let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

    let mut template = build_review_template(
        current_user,
        &sub,
        &submitter_name,
        &reviewer_name,
        &snapshot,
        &ref_data,
        &systems_media_json,
        &systems_has_flags_json,
        &edition_suggestions_json,
        &media_layers_json,
        &media_rom_extensions_json,
        &media_is_cd_json,
        &media_has_pic_json,
        &system_code,
        &media_type_code,
        max_layers,
        has_sys,
    );

    if let Some(db_snapshot) = db_snapshot {
        template.review_base_hash = queue_service::disc_snapshot_hash(&db_snapshot);
        apply_review_diff_context(
            &mut template,
            &snapshot,
            &db_snapshot,
            &ref_data,
            sub.submission_type,
            true,
        );
        let highlights = compute_field_highlights(&snapshot, &db_snapshot);
        apply_highlights(&mut template, highlights);
    }

    Ok(Html(template.render().unwrap()))
}

fn build_review_template(
    current_user: AuthenticatedUser,
    sub: &DiscSubmission,
    submitter_name: &str,
    reviewer_name: &str,
    snapshot: &serde_json::Value,
    ref_data: &disc_edit::EditRefData,
    systems_media_json: &str,
    systems_has_flags_json: &str,
    edition_suggestions_json: &str,
    media_layers_json: &str,
    media_rom_extensions_json: &str,
    media_is_cd_json: &str,
    media_has_pic_json: &str,
    system_code: &str,
    media_type_code: &str,
    max_layers: u32,
    has_sys: impl Fn(fn(&System) -> bool) -> bool,
) -> DiscEditTemplate {
    let json_str = |key: &str| snapshot[key].as_str().unwrap_or("").to_string();
    let json_opt_str = |key: &str| match &snapshot[key] {
        serde_json::Value::String(s) => s.clone(),
        _ => String::new(),
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
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_i64())
                .map(|v| v.to_string())
                .collect()
        })
        .unwrap_or_default();

    let error_count = match &snapshot["error_count"] {
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    };

    let edc_value = if snapshot["edc"].as_bool().unwrap_or(false) {
        "true".to_string()
    } else {
        "false".to_string()
    };

    let protection_key_disc_key = json_opt_str("disc_key");
    let universal_hash = json_opt_str("universal_hash");
    let protection_key_disc_id = json_opt_str("disc_id");

    let status = snapshot["status"]
        .as_str()
        .unwrap_or("Unverified")
        .to_string();

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
        current_user: Some(current_user),
        disc_id: sub.target_disc_id.unwrap_or(0),
        page_title,
        submission_token: String::new(),

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
        edition_suggestions_json: edition_suggestions_json.to_string(),
        submit_as_usernames_json: "[]".to_string(),
        media_rom_extensions_json: media_rom_extensions_json.to_string(),
        media_is_cd_json: media_is_cd_json.to_string(),

        title: json_str("title"),
        show_title_foreign: has_sys(|s| s.has_title_foreign),
        title_foreign: json_opt_str("title_foreign"),
        show_disc_number: has_sys(|s| s.has_disc_number),
        disc_number: json_opt_str("disc_number"),
        show_disc_title: has_sys(|s| s.has_disc_title),
        disc_title: json_opt_str("disc_title"),
        filename_suffix: json_opt_str("filename_suffix"),

        show_serial: has_sys(|s| s.has_serial),
        serials: json_str_vec("serial")
            .into_iter()
            .map(|s| disc_edit::HighlightedValue {
                value: s,
                highlight: String::new(),
            })
            .collect(),
        show_version: has_sys(|s| s.has_version),
        version: json_opt_str("version"),
        show_edition: has_sys(|s| s.has_edition),
        editions: json_str_vec("edition")
            .into_iter()
            .map(|s| disc_edit::HighlightedValue {
                value: s,
                highlight: String::new(),
            })
            .collect(),
        show_barcode: has_sys(|s| s.has_barcode),
        barcodes: json_str_vec("barcode")
            .into_iter()
            .map(|s| disc_edit::HighlightedValue {
                value: s,
                highlight: String::new(),
            })
            .collect(),
        ring_codes_json,
        ring_highlights_json: "[]".to_string(),
        can_add_ringcode_entries: true,

        comments: json_opt_str("comments"),
        contents: json_opt_str("contents"),

        show_error_count: ref_data
            .all_media_types
            .iter()
            .find(|m| m.code == media_type_code)
            .map_or(false, |m| is_cd_rom_extension(&m.rom_extension)),
        error_count,
        show_exe_date: has_sys(|s| s.has_exe_date),
        exe_date: json_opt_str("exe_date"),
        show_edc: has_sys(|s| s.has_edc),
        edc_value,

        layerbreaks,
        show_pvd: has_sys(|s| s.has_pvd),
        pvd_hex: json_opt_str("pvd"),
        show_pic: ref_data
            .all_media_types
            .iter()
            .find(|m| m.code == media_type_code)
            .map_or(false, |m| m.pic),
        media_has_pic_json: media_has_pic_json.to_string(),
        pic_hex: json_opt_str("pic"),
        show_bca: has_sys(|s| s.has_bca),
        bca_hex: json_opt_str("bca"),
        show_header: has_sys(|s| s.has_header),
        header_hex: json_opt_str("header"),

        show_disc_id: has_sys(|s| s.has_disc_id),
        show_key: has_sys(|s| s.has_key),
        show_universal_hash: has_sys(|s| s.has_universal_hash),
        show_protection: has_sys(|s| s.has_protection),
        protection: json_opt_str("protection"),
        show_sector_ranges: has_sys(|s| s.has_sector_ranges),
        sector_ranges_text,
        show_sbi: disc_edit::sbi_available_for_selection(
            &ref_data.all_systems,
            &ref_data.all_media_types,
            system_code,
            media_type_code,
        ),
        sbi: json_opt_str("sbi"),
        protection_key_disc_key,
        universal_hash,
        protection_key_disc_id,
        has_sample_start: has_sys(|s| s.has_sample_start),

        cue: json_opt_str("cuesheet"),
        files_xml: json_opt_str("dat"),

        status,

        is_add_mode: false,
        add_requires_system_media: false,
        dump_log: String::new(),
        dump_log_required: false,
        extra_upload_url: String::new(),
        show_submit_as: false,
        submit_as_username: String::new(),

        submit_button_text: String::new(),
        validation_errors: vec![],
        linked_validation_errors: vec![],
        validation_result: String::new(),
        validation_result_disc_id: 0,
        validation_result_disc_title: String::new(),
        validation_result_suffix: String::new(),

        is_review_mode: true,
        review_base_hash: String::new(),
        changed_fields: vec![],
        review_annotations: vec![],
        review_old_multiline: vec![],
        submission_id: sub.id,
        submission_type_display: sub.display_kind().to_string(),
        submitter_id: sub.submitter_id,
        submitter_name: submitter_name.to_string(),
        submission_comment: sub.submission_comment.clone().unwrap_or_default(),
        dump_log_display: sub.dump_log.clone().unwrap_or_default(),
        extra_upload_url_display: sub.extra_upload_url.clone().unwrap_or_default(),
        submission_status: sub.status.to_string(),
        reviewer_id: sub.reviewer_id.unwrap_or(0),
        reviewer_name: reviewer_name.to_string(),
        review_comment_display: sub.review_comment.clone().unwrap_or_default(),
        review_comment_input: String::new(),
        created_at_display: sub.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
        reviewed_at_display: sub
            .reviewed_at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_default(),
        changes_original_json: pretty_optional_json(sub.changes_original.as_ref()),
        changes_json: serde_json::to_string_pretty(&sub.changes).unwrap_or_default(),
    }
}

#[derive(Default)]
struct ReviewDiffContext {
    annotations: Vec<ReviewAnnotation>,
    old_multiline: Vec<ReviewOldMultiline>,
}

fn template_field_name(field: &str) -> String {
    match field {
        "cuesheet" => "cue".to_string(),
        "dat" => "files_xml".to_string(),
        other => other.to_string(),
    }
}

fn is_empty_review_value(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => false,
    }
}

fn review_values_equal(old: &serde_json::Value, new: &serde_json::Value) -> bool {
    if old == new {
        return true;
    }
    match (old, new) {
        (serde_json::Value::String(a), serde_json::Value::String(b)) => {
            a.trim().replace("\r\n", "\n") == b.trim().replace("\r\n", "\n")
        }
        _ => false,
    }
}

fn review_value_changed(old: &serde_json::Value, new: &serde_json::Value) -> bool {
    if is_empty_review_value(old) && is_empty_review_value(new) {
        return false;
    }
    !review_values_equal(old, new)
}

fn review_display_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Array(values) => values
            .iter()
            .map(review_display_value)
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        serde_json::Value::Object(_) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn review_annotation_value(value: &serde_json::Value) -> String {
    let display = review_display_value(value);
    if display.trim().is_empty() {
        "(empty)".to_string()
    } else {
        display
    }
}

fn add_annotation(
    context: &mut ReviewDiffContext,
    field: &str,
    label: &str,
    kind: &str,
    values: Vec<String>,
) {
    if values.is_empty() {
        return;
    }
    context.annotations.push(ReviewAnnotation {
        field: field.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
        values,
    });
}

fn add_single_annotation(
    context: &mut ReviewDiffContext,
    field: &str,
    label: &str,
    kind: &str,
    value: String,
) {
    add_annotation(context, field, label, kind, vec![value]);
}

fn array_strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn set_added_removed(old_values: &[String], new_values: &[String]) -> (Vec<String>, Vec<String>) {
    let mut added: Vec<String> = new_values
        .iter()
        .filter(|value| !old_values.iter().any(|old| old == *value))
        .cloned()
        .collect();
    let mut removed: Vec<String> = old_values
        .iter()
        .filter(|value| !new_values.iter().any(|new| new == *value))
        .cloned()
        .collect();
    added.sort_unstable_by_key(|s| s.to_lowercase());
    removed.sort_unstable_by_key(|s| s.to_lowercase());
    (added, removed)
}

fn display_region(ref_data: &disc_edit::EditRefData, code: &str) -> String {
    ref_data
        .all_regions
        .iter()
        .find(|region| region.code.trim() == code.trim())
        .map(|region| region.name.clone())
        .unwrap_or_else(|| code.to_string())
}

fn display_language(ref_data: &disc_edit::EditRefData, code: &str) -> String {
    ref_data
        .all_languages
        .iter()
        .find(|language| language.code.trim() == code.trim())
        .map(|language| language.name.clone())
        .unwrap_or_else(|| code.to_string())
}

fn display_system(ref_data: &disc_edit::EditRefData, code: &str) -> String {
    ref_data
        .all_systems
        .iter()
        .find(|system| system.code == code)
        .map(|system| system.system_name())
        .unwrap_or_else(|| code.to_string())
}

fn display_media(ref_data: &disc_edit::EditRefData, code: &str) -> String {
    ref_data
        .all_media_types
        .iter()
        .find(|media| media.code == code)
        .map(|media| media.name.clone())
        .unwrap_or_else(|| code.to_string())
}

fn review_named_value(
    field: &str,
    value: &serde_json::Value,
    ref_data: &disc_edit::EditRefData,
) -> String {
    match field {
        "system_code" => value
            .as_str()
            .map(|code| display_system(ref_data, code))
            .unwrap_or_default(),
        "media_type" => value
            .as_str()
            .map(|code| display_media(ref_data, code))
            .unwrap_or_default(),
        _ => review_display_value(value),
    }
}

fn sector_ranges_display(value: &serde_json::Value) -> String {
    value
        .as_array()
        .map(|ranges| {
            ranges
                .iter()
                .map(|range| {
                    format!(
                        "{}-{}",
                        range["start"].as_i64().unwrap_or(0),
                        range["end"].as_i64().unwrap_or(0)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn multiline_display_value(field: &str, value: &serde_json::Value) -> String {
    if field == "sector_ranges" {
        sector_ranges_display(value)
    } else {
        review_display_value(value)
    }
}

fn compose_review_comments(old: &serde_json::Value, submitted: &serde_json::Value) -> String {
    let old_text = review_display_value(old);
    let submitted_text = review_display_value(submitted);
    let mut sections = Vec::new();
    if !old_text.trim().is_empty() {
        sections.push(old_text);
    }
    sections.push(COMMENTS_REVIEW_DELIMITER.to_string());
    if !submitted_text.trim().is_empty() {
        sections.push(submitted_text);
    }
    sections.join("\n\n")
}

fn comments_text_contains_review_delimiter(comments: Option<&str>) -> bool {
    comments
        .map(|comments| comments.contains(COMMENTS_REVIEW_DELIMITER))
        .unwrap_or(false)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReviewIdentityMode {
    PreferOld,
    PreferSubmitted,
}

impl ReviewIdentityMode {
    fn from_submission_type(submission_type: SubmissionType) -> Self {
        match submission_type {
            SubmissionType::Disc => Self::PreferOld,
            SubmissionType::Edit => Self::PreferSubmitted,
        }
    }

    fn annotation_label(self) -> &'static str {
        match self {
            Self::PreferOld => "Changed to",
            Self::PreferSubmitted => "Changed from",
        }
    }

    fn annotation_kind(self) -> &'static str {
        match self {
            Self::PreferOld => "added",
            Self::PreferSubmitted => "removed",
        }
    }
}

fn build_review_diff_context(
    submitted_snapshot: &serde_json::Value,
    db_snapshot: &serde_json::Value,
    ref_data: &disc_edit::EditRefData,
    submission_type: SubmissionType,
) -> ReviewDiffContext {
    let mut context = ReviewDiffContext::default();
    let identity_mode = ReviewIdentityMode::from_submission_type(submission_type);

    for field in ["system_code", "media_type", "category"] {
        let old = &db_snapshot[field];
        let new = &submitted_snapshot[field];
        if !review_value_changed(old, new) {
            continue;
        }
        let annotation_value = match identity_mode {
            ReviewIdentityMode::PreferOld => review_named_value(field, new, ref_data),
            ReviewIdentityMode::PreferSubmitted => review_named_value(field, old, ref_data),
        };
        add_single_annotation(
            &mut context,
            field,
            identity_mode.annotation_label(),
            identity_mode.annotation_kind(),
            if annotation_value.trim().is_empty() {
                "(empty)".to_string()
            } else {
                annotation_value
            },
        );
    }

    for field in [
        "title",
        "title_foreign",
        "disc_number",
        "disc_title",
        "filename_suffix",
    ] {
        let old = &db_snapshot[field];
        let new = &submitted_snapshot[field];
        if review_value_changed(old, new) {
            let annotation_value = match identity_mode {
                ReviewIdentityMode::PreferOld => review_annotation_value(new),
                ReviewIdentityMode::PreferSubmitted => review_annotation_value(old),
            };
            add_single_annotation(
                &mut context,
                field,
                identity_mode.annotation_label(),
                identity_mode.annotation_kind(),
                annotation_value,
            );
        }
    }

    for field in [
        "version",
        "error_count",
        "exe_date",
        "disc_id",
        "disc_key",
        "universal_hash",
    ] {
        let old = &db_snapshot[field];
        let new = &submitted_snapshot[field];
        if review_value_changed(old, new) {
            add_single_annotation(
                &mut context,
                field,
                "Changed from",
                "removed",
                review_annotation_value(old),
            );
        }
    }

    {
        let old = &db_snapshot["layerbreaks"];
        let new = &submitted_snapshot["layerbreaks"];
        if review_value_changed(old, new) {
            add_single_annotation(
                &mut context,
                "layerbreaks",
                "Changed from",
                "removed",
                review_annotation_value(old),
            );
        }
    }

    for (field, display) in [
        (
            "regions",
            display_region as fn(&disc_edit::EditRefData, &str) -> String,
        ),
        (
            "languages",
            display_language as fn(&disc_edit::EditRefData, &str) -> String,
        ),
    ] {
        let old = array_strings(&db_snapshot[field]);
        let new = array_strings(&submitted_snapshot[field]);
        let (added, removed) = set_added_removed(&old, &new);
        add_annotation(
            &mut context,
            field,
            "Removed",
            "removed",
            removed
                .iter()
                .map(|value| display(ref_data, value))
                .collect(),
        );
        add_annotation(
            &mut context,
            field,
            "Added",
            "added",
            added.iter().map(|value| display(ref_data, value)).collect(),
        );
    }

    for field in ["serial", "edition", "barcode"] {
        let old = array_strings(&db_snapshot[field]);
        let new = array_strings(&submitted_snapshot[field]);
        let (added, removed) = set_added_removed(&old, &new);
        add_annotation(&mut context, field, "Removed", "removed", removed);
        add_annotation(&mut context, field, "Added", "added", added);
    }

    for field in [
        "contents",
        "protection",
        "sector_ranges",
        "sbi",
        "pvd",
        "header",
        "bca",
        "pic",
        "cuesheet",
        "dat",
    ] {
        let old = &db_snapshot[field];
        let new = &submitted_snapshot[field];
        if review_value_changed(old, new) {
            context.old_multiline.push(ReviewOldMultiline {
                field: template_field_name(field),
                value: multiline_display_value(field, old),
            });
        }
    }

    context
}

fn apply_review_diff_context(
    template: &mut DiscEditTemplate,
    submitted_snapshot: &serde_json::Value,
    db_snapshot: &serde_json::Value,
    ref_data: &disc_edit::EditRefData,
    submission_type: SubmissionType,
    apply_initial_values: bool,
) {
    let context =
        build_review_diff_context(submitted_snapshot, db_snapshot, ref_data, submission_type);
    template.review_annotations = context.annotations;
    template.review_old_multiline = context.old_multiline;

    if !apply_initial_values {
        return;
    }

    if ReviewIdentityMode::from_submission_type(submission_type) == ReviewIdentityMode::PreferOld {
        if review_value_changed(
            &db_snapshot["system_code"],
            &submitted_snapshot["system_code"],
        ) {
            let old_system_code = review_display_value(&db_snapshot["system_code"]);
            template.system_code = old_system_code.clone();
            template.systems = build_system_options(&ref_data.all_systems, &old_system_code);
            if let Some(system) = ref_data
                .all_systems
                .iter()
                .find(|system| system.code == old_system_code)
            {
                template.show_title_foreign = system.has_title_foreign;
                template.show_disc_number = system.has_disc_number;
                template.show_disc_title = system.has_disc_title;
                template.show_serial = system.has_serial;
                template.show_version = system.has_version;
                template.show_edition = system.has_edition;
                template.show_barcode = system.has_barcode;
                template.show_exe_date = system.has_exe_date;
                template.show_edc = system.has_edc;
                template.show_disc_id = system.has_disc_id;
                template.show_key = system.has_key;
                template.show_universal_hash = system.has_universal_hash;
                template.show_protection = system.has_protection;
                template.show_sector_ranges = system.has_sector_ranges;
                template.show_pvd = system.has_pvd;
                template.show_bca = system.has_bca;
                template.show_header = system.has_header;
                template.has_sample_start = system.has_sample_start;
            }
        }

        if review_value_changed(
            &db_snapshot["media_type"],
            &submitted_snapshot["media_type"],
        ) {
            let old_media_type = review_display_value(&db_snapshot["media_type"]);
            template.media_type_code = old_media_type.clone();
            template.media_types_all =
                build_media_options(&ref_data.all_media_types, &old_media_type);
            template.max_layers = max_layers_for_media(&ref_data.all_media_types, &old_media_type);
            template.show_error_count = ref_data
                .all_media_types
                .iter()
                .find(|media| media.code == old_media_type)
                .map_or(false, |media| is_cd_rom_extension(&media.rom_extension));
            template.show_pic = ref_data
                .all_media_types
                .iter()
                .find(|media| media.code == old_media_type)
                .map_or(false, |media| media.pic);
        }

        template.show_sbi = disc_edit::sbi_available_for_selection(
            &ref_data.all_systems,
            &ref_data.all_media_types,
            &template.system_code,
            &template.media_type_code,
        );

        if review_value_changed(&db_snapshot["category"], &submitted_snapshot["category"]) {
            let old_category = review_display_value(&db_snapshot["category"]);
            template.categories = build_category_options(&ref_data.all_categories, &old_category);
        }

        for field in [
            "title",
            "title_foreign",
            "disc_number",
            "disc_title",
            "filename_suffix",
        ] {
            if !review_value_changed(&db_snapshot[field], &submitted_snapshot[field]) {
                continue;
            }
            let old_value = review_display_value(&db_snapshot[field]);
            match field {
                "title" => template.title = old_value,
                "title_foreign" => template.title_foreign = old_value,
                "disc_number" => template.disc_number = old_value,
                "disc_title" => template.disc_title = old_value,
                "filename_suffix" => template.filename_suffix = old_value,
                _ => {}
            }
        }
    }

    if review_value_changed(&db_snapshot["comments"], &submitted_snapshot["comments"]) {
        template.comments =
            compose_review_comments(&db_snapshot["comments"], &submitted_snapshot["comments"]);
    }
}

// ── Read-only submission detail ────────────────────────────────────────

#[derive(Template)]
#[template(path = "queue_detail.html")]
struct QueueDetailTemplate {
    current_user: Option<AuthenticatedUser>,
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
    changes_original_json: String,
    changes_json: String,
}
impl SiteConfig for QueueDetailTemplate {}

fn pretty_optional_json(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(value) if !value.is_null() => serde_json::to_string_pretty(value).unwrap_or_default(),
        _ => String::new(),
    }
}

async fn render_readonly_detail(
    current_user: Option<AuthenticatedUser>,
    sub: &DiscSubmission,
    submitter_name: &str,
    reviewer_name: &str,
) -> AppResult<Html<String>> {
    let template = QueueDetailTemplate {
        current_user,
        submission_id: sub.id,
        submission_type_display: sub.display_kind().to_string(),
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
        changes_original_json: pretty_optional_json(sub.changes_original.as_ref()),
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
        "system_code",
        "media_type",
        "category",
        "title",
        "title_foreign",
        "disc_number",
        "disc_title",
        "filename_suffix",
        "version",
        "error_count",
        "exe_date",
        "edc",
        "comments",
        "contents",
        "protection",
        "sector_ranges",
        "sbi",
        "disc_id",
        "disc_key",
        "universal_hash",
        "pvd",
        "header",
        "bca",
        "pic",
        "cuesheet",
        "dat",
        "status",
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
            changed_fields.push(format!("{}:added", template_field_name(field)));
        } else if !db_empty && ch_empty {
            changed_fields.push(format!("{}:removed", template_field_name(field)));
        } else if !vals_equal(db_val, ch_val) {
            changed_fields.push(format!("{}:changed", template_field_name(field)));
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

    let str_set = |v: &serde_json::Value| -> std::collections::HashSet<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
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
    for s in &db_serials {
        if !ch_serials.contains(s) {
            serial_highlights.insert(s.clone(), "removed".to_string());
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
    for s in &db_editions {
        if !ch_editions.contains(s) {
            edition_highlights.insert(s.clone(), "removed".to_string());
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
    for s in &db_barcodes {
        if !ch_barcodes.contains(s) {
            barcode_highlights.insert(s.clone(), "removed".to_string());
        }
    }
    if !barcode_highlights.is_empty() {
        changed_fields.push("barcode:changed".to_string());
    }
    let classify_change =
        |old: &serde_json::Value, new: &serde_json::Value, csv_ids: bool| -> Option<&'static str> {
            let is_empty = |v: &serde_json::Value| -> bool {
                match v {
                    serde_json::Value::Null => true,
                    serde_json::Value::String(s) => s.trim().is_empty(),
                    serde_json::Value::Array(a) => a.is_empty(),
                    serde_json::Value::Object(o) => o.is_empty(),
                    serde_json::Value::Bool(_) | serde_json::Value::Number(_) => false,
                }
            };
            if old == new {
                return None;
            }
            let old_empty = is_empty(old);
            let new_empty = is_empty(new);
            if old_empty && !new_empty {
                return Some("added");
            }
            if !old_empty && new_empty {
                return Some("removed");
            }
            if csv_ids {
                let parse = |v: &serde_json::Value| -> std::collections::HashSet<String> {
                    v.as_str()
                        .unwrap_or("")
                        .split(',')
                        .map(|s| s.trim().to_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                };
                let old_set = parse(old);
                let new_set = parse(new);
                if old_set == new_set {
                    return None;
                }
                // CSV ring fields (toolstamps/mould sids/additional moulds) are compared
                // as unordered sets. We cannot reliably mark per-token add/remove in UI.
                return Some("changed");
            }
            Some("changed")
        };

    let mut ring_highlights: Vec<serde_json::Value> = Vec::new();
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
        let mut db_by_id: std::collections::HashMap<i32, &serde_json::Value> =
            std::collections::HashMap::new();
        for entry in &db_arr {
            if let Some(id) = entry.get("id").and_then(|v| v.as_i64()).map(|v| v as i32) {
                db_by_id.insert(id, entry);
            }
        }

        for ch_entry in &ch_arr_sorted {
            let mut entry_highlight = serde_json::Map::new();
            let db_entry_opt = ch_entry
                .get("id")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
                .and_then(|id| db_by_id.get(&id).copied());
            if let Some(db_entry) = db_entry_opt {
                for (field, csv_ids) in [
                    ("offset_value", false),
                    ("offset_extra_value", false),
                    ("sample_start", false),
                    ("comment", false),
                ] {
                    if let Some(status) =
                        classify_change(&db_entry[field], &ch_entry[field], csv_ids)
                    {
                        entry_highlight.insert(field.to_string(), serde_json::json!(status));
                    }
                }

                let db_layers = db_entry["layers"].as_array().cloned().unwrap_or_default();
                let ch_layers = ch_entry["layers"].as_array().cloned().unwrap_or_default();
                let max_layers = db_layers.len().max(ch_layers.len());
                let mut layer_highlights: Vec<serde_json::Value> = Vec::new();
                let mut has_layer_highlights = false;
                for li in 0..max_layers {
                    let db_layer = db_layers
                        .get(li)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    let ch_layer = ch_layers
                        .get(li)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    let mut layer_map = serde_json::Map::new();

                    for (field, csv_ids) in [
                        ("mastering_code", false),
                        ("mastering_sid", false),
                        ("toolstamps", true),
                        ("mould_sids", true),
                        ("additional_moulds", true),
                    ] {
                        if let Some(status) =
                            classify_change(&db_layer[field], &ch_layer[field], csv_ids)
                        {
                            layer_map.insert(field.to_string(), serde_json::json!(status));
                        }
                    }

                    if !layer_map.is_empty() {
                        has_layer_highlights = true;
                    }
                    layer_highlights.push(serde_json::Value::Object(layer_map));
                }

                if has_layer_highlights {
                    entry_highlight.insert(
                        "layers".to_string(),
                        serde_json::Value::Array(layer_highlights),
                    );
                }

                ring_highlights.push(serde_json::Value::Object(entry_highlight));
            } else {
                entry_highlight.insert("entry".to_string(), serde_json::json!("added"));
                ring_highlights.push(serde_json::Value::Object(entry_highlight));
            }
        }
        // Ring codes: only individual entry highlighting via ring_highlights_json,
        // no fieldset-level highlight.
    }
    let ring_highlights_json =
        serde_json::to_string(&ring_highlights).unwrap_or_else(|_| "[]".to_string());

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
    #[serde(default, rename = "_csrf")]
    pub csrf_token: String,
    pub action: String,
    pub review_comment: Option<String>,
    #[serde(default)]
    pub review_base_hash: String,
    #[serde(flatten)]
    pub disc: DiscEditForm,
}

const STALE_REVIEW_ERROR: &str = "This disc changed after the review page loaded. The page has been refreshed with the latest data; review it again before approving.";

async fn render_latest_review_with_errors(
    state: &AppState,
    current_user: &AuthenticatedUser,
    sub: &DiscSubmission,
    ref_data: &disc_edit::EditRefData,
    validation_errors: Vec<String>,
    review_comment: Option<&str>,
) -> AppResult<Response> {
    let submitter_name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(sub.submitter_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or_else(|_| format!("User #{}", sub.submitter_id));
    let reviewer_name: String = if let Some(reviewer_id) = sub.reviewer_id {
        sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
            .bind(reviewer_id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or_default()
    } else {
        String::new()
    };

    let (systems_media_json, systems_has_flags_json) = build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_is_cd_json = build_media_is_cd_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
    let edition_suggestions_json = disc_edit::build_edition_suggestions_json(state).await?;

    let snapshot: serde_json::Value;
    let mut db_snapshot: Option<serde_json::Value> = None;
    let mut review_base_hash = String::new();
    if let Some(disc_id) = sub.target_disc_id {
        let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;
        let current_db_snapshot = disc_service::build_snapshot_from_disc(&detail);
        review_base_hash = queue_service::disc_snapshot_hash(&current_db_snapshot);
        snapshot =
            queue_service::resolve_submission_snapshot_for_submission(&current_db_snapshot, sub)?;
        db_snapshot = Some(current_db_snapshot);
    } else {
        snapshot =
            queue_service::resolve_submission_snapshot_for_submission(&serde_json::json!({}), sub)?;
    }

    let system_code = snapshot["system_code"].as_str().unwrap_or("").to_string();
    let media_type_code = snapshot["media_type"].as_str().unwrap_or("cd").to_string();
    let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);
    let system = if !system_code.is_empty() {
        disc_service::get_system(&state.pool, &system_code)
            .await
            .ok()
    } else {
        None
    };
    let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

    let mut template = build_review_template(
        current_user.clone(),
        sub,
        &submitter_name,
        &reviewer_name,
        &snapshot,
        ref_data,
        &systems_media_json,
        &systems_has_flags_json,
        &edition_suggestions_json,
        &media_layers_json,
        &media_rom_extensions_json,
        &media_is_cd_json,
        &media_has_pic_json,
        &system_code,
        &media_type_code,
        max_layers,
        has_sys,
    );
    template.review_base_hash = review_base_hash;
    template.validation_errors = validation_errors;
    template.review_comment_input = review_comment.unwrap_or_default().to_string();

    if let Some(db_snapshot) = db_snapshot {
        apply_review_diff_context(
            &mut template,
            &snapshot,
            &db_snapshot,
            ref_data,
            sub.submission_type,
            true,
        );
        let highlights = compute_field_highlights(&snapshot, &db_snapshot);
        apply_highlights(&mut template, highlights);
    }

    Ok(Html(template.render().unwrap()).into_response())
}

async fn review_submit(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
    Form(form): Form<ReviewForm>,
) -> AppResult<Response> {
    csrf::verify_token(&user, &form.csrf_token)?;

    let sub = queue_service::get_submission(&state.pool, id).await?;

    if !can_review_submission(&user, &sub) {
        return Err(AppError::Forbidden);
    }

    if sub.status != SubmissionStatus::Pending {
        return Ok(Redirect::to(&format!("/queue/{id}")).into_response());
    }

    let review_comment = form
        .review_comment
        .as_deref()
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if form.action == "reject" {
        let rejected =
            queue_service::reject_submission(&state.pool, id, user.id, review_comment.as_deref())
                .await?;

        if !rejected {
            return Ok(Redirect::to(&format!("/queue/{id}")).into_response());
        }
        return Ok(Redirect::to("/queue").into_response());
    }
    if form.action != "approve" {
        return Err(AppError::BadRequest("unknown review action".into()));
    }

    let ref_data = fetch_ref_data(&state.pool).await?;
    if let Some(disc_id) = sub.target_disc_id {
        let current_hash = queue_service::current_disc_snapshot_hash(&state.pool, disc_id).await?;
        if queue_service::review_base_hash_is_stale(Some(&form.review_base_hash), &current_hash) {
            return render_latest_review_with_errors(
                &state,
                &user,
                &sub,
                &ref_data,
                vec![STALE_REVIEW_ERROR.to_string()],
                review_comment.as_deref(),
            )
            .await;
        }
    }

    let mut errors = validate_form(&form.disc, &ref_data.all_media_types, &ref_data.all_systems);
    if comments_text_contains_review_delimiter(form.disc.comments.as_deref()) {
        errors.push("Comments: remove the review delimiter before approval".to_string());
    }
    if !errors.is_empty() {
        let submitter_name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
            .bind(sub.submitter_id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or_else(|_| format!("User #{}", sub.submitter_id));

        let (systems_media_json, systems_has_flags_json) =
            build_systems_json(&ref_data.all_systems);
        let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
        let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
        let media_is_cd_json = build_media_is_cd_json(&ref_data.all_media_types);
        let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
        let edition_suggestions_json = disc_edit::build_edition_suggestions_json(&state).await?;
        let snapshot =
            build_flat_changes(&form.disc, &ref_data.all_media_types, &ref_data.all_systems);
        let system_code = form.disc.system_code.clone();
        let media_type_code = form.disc.media_type.clone();
        let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);
        let system = disc_service::get_system(&state.pool, &system_code)
            .await
            .ok();
        let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

        let mut template = build_review_template(
            user.clone(),
            &sub,
            &submitter_name,
            "",
            &snapshot,
            &ref_data,
            &systems_media_json,
            &systems_has_flags_json,
            &edition_suggestions_json,
            &media_layers_json,
            &media_rom_extensions_json,
            &media_is_cd_json,
            &media_has_pic_json,
            &system_code,
            &media_type_code,
            max_layers,
            has_sys,
        );
        template.validation_errors = errors;
        template.linked_validation_errors = vec![];
        template.review_comment_input = review_comment.clone().unwrap_or_default();

        if let Some(disc_id) = sub.target_disc_id {
            let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;
            let db_snapshot = disc_service::build_snapshot_from_disc(&detail);
            template.review_base_hash = form.review_base_hash.clone();
            let submitted_snapshot =
                queue_service::resolve_submission_snapshot_for_submission(&db_snapshot, &sub)?;
            apply_review_diff_context(
                &mut template,
                &submitted_snapshot,
                &db_snapshot,
                &ref_data,
                sub.submission_type,
                false,
            );
            let highlights = compute_field_highlights(&submitted_snapshot, &db_snapshot);
            apply_highlights(&mut template, highlights);
        }

        return Ok(Html(template.render().unwrap()).into_response());
    }

    let form_snapshot = if let Some(disc_id) = sub.target_disc_id {
        let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;
        build_sparse_edit_changes(
            &form.disc,
            &detail,
            &ref_data.all_media_types,
            &ref_data.all_systems,
        )
    } else {
        if sub.submission_type != SubmissionType::Disc {
            return Err(AppError::BadRequest(
                "edit submission is missing a target disc".into(),
            ));
        }
        build_new_disc_changes(&form.disc, &ref_data.all_media_types, &ref_data.all_systems)
    };

    let approved = queue_service::approve_submission(
        &state.pool,
        &sub,
        &form_snapshot,
        user.id,
        review_comment.as_deref(),
        sub.target_disc_id.map(|_| form.review_base_hash.as_str()),
    )
    .await?;

    match approved {
        queue_service::ApprovalOutcome::Approved(disc_id) => {
            Ok(Redirect::to(&format!("/disc/{disc_id}")).into_response())
        }
        queue_service::ApprovalOutcome::AlreadyProcessed => {
            Ok(Redirect::to(&format!("/queue/{id}")).into_response())
        }
        queue_service::ApprovalOutcome::StaleDiscState => {
            render_latest_review_with_errors(
                &state,
                &user,
                &sub,
                &ref_data,
                vec![STALE_REVIEW_ERROR.to_string()],
                review_comment.as_deref(),
            )
            .await
        }
        queue_service::ApprovalOutcome::Conflicts(conflicts) => {
            let submitter_name: String =
                sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
                    .bind(sub.submitter_id)
                    .fetch_one(&state.pool)
                    .await
                    .unwrap_or_else(|_| format!("User #{}", sub.submitter_id));

            let (systems_media_json, systems_has_flags_json) =
                build_systems_json(&ref_data.all_systems);
            let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
            let media_rom_extensions_json =
                build_media_rom_extensions_json(&ref_data.all_media_types);
            let media_is_cd_json = build_media_is_cd_json(&ref_data.all_media_types);
            let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
            let edition_suggestions_json =
                disc_edit::build_edition_suggestions_json(&state).await?;
            let snapshot =
                build_flat_changes(&form.disc, &ref_data.all_media_types, &ref_data.all_systems);
            let system_code = form.disc.system_code.clone();
            let media_type_code = form.disc.media_type.clone();
            let max_layers = max_layers_for_media(&ref_data.all_media_types, &media_type_code);
            let system = disc_service::get_system(&state.pool, &system_code)
                .await
                .ok();
            let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

            let mut template = build_review_template(
                user.clone(),
                &sub,
                &submitter_name,
                "",
                &snapshot,
                &ref_data,
                &systems_media_json,
                &systems_has_flags_json,
                &edition_suggestions_json,
                &media_layers_json,
                &media_rom_extensions_json,
                &media_is_cd_json,
                &media_has_pic_json,
                &system_code,
                &media_type_code,
                max_layers,
                has_sys,
            );
            template.linked_validation_errors =
                approval_conflicts_to_linked_validation_errors(conflicts);
            template.review_comment_input = review_comment.unwrap_or_default();

            if let Some(disc_id) = sub.target_disc_id {
                let detail = disc_service::get_disc_detail(&state.pool, disc_id).await?;
                let db_snapshot = disc_service::build_snapshot_from_disc(&detail);
                template.review_base_hash = form.review_base_hash.clone();
                let submitted_snapshot =
                    queue_service::resolve_submission_snapshot_for_submission(&db_snapshot, &sub)?;
                apply_review_diff_context(
                    &mut template,
                    &submitted_snapshot,
                    &db_snapshot,
                    &ref_data,
                    sub.submission_type,
                    false,
                );
                let highlights = compute_field_highlights(&submitted_snapshot, &db_snapshot);
                apply_highlights(&mut template, highlights);
            }

            Ok(Html(template.render().unwrap()).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    #[test]
    fn compact_queue_urls_omit_empty_and_default_state() {
        assert_eq!(
            build_queue_url(QueueUrlOptions {
                status: "Pending",
                sort: "date",
                order: "desc",
                page: 1,
                ..Default::default()
            }),
            "/queue"
        );
    }

    #[test]
    fn compact_queue_urls_preserve_filters_navigation_and_encoding() {
        assert_eq!(
            build_queue_url(QueueUrlOptions {
                status: "Approved",
                sub_type: "New Disc",
                system: "PS2",
                submitter: "A/B",
                sort: "title",
                order: "asc",
                page: 3,
                ..Default::default()
            }),
            "/queue?status=Approved&sub_type=New%20Disc&system=PS2&submitter=A%2FB&sort=title&order=asc&page=3"
        );
        assert_eq!(
            build_queue_url(QueueUrlOptions {
                status: "All Visible",
                disc_id: "42",
                sort: "date",
                order: "desc",
                page: 1,
                is_disc_history: true,
                ..Default::default()
            }),
            "/queue?disc_id=42"
        );
    }

    fn test_state() -> AppState {
        let database_url = "postgres://postgres:postgres@localhost/postgres".to_string();

        AppState {
            pool: sqlx::postgres::PgPoolOptions::new()
                .connect_lazy(&database_url)
                .unwrap(),
            config: Arc::new(crate::config::Config {
                site_name: "localhost".to_string(),
                database_url,
                site_url: "http://localhost".to_string(),
                base_url: "http://localhost".to_string(),
                wiki_url: "#".to_string(),
                forum_url: "#".to_string(),
                news_feed_url: "#".to_string(),
                port: 0,
                oidc_provider_url: "#".to_string(),
                oidc_client_id: "test".to_string(),
                oidc_client_secret: "test".to_string(),
            }),
            http: reqwest::Client::new(),
            edition_suggestions: crate::services::disc_service::EditionSuggestionsCache::new(
                Duration::from_secs(60),
            ),
            news_cache: crate::services::news_service::NewsCache::new(Duration::from_secs(
                crate::services::news_service::NEWS_FEED_TTL_SECONDS,
            )),
            homepage_cache: crate::routes::main_page::HomepageCache::new(Duration::from_secs(60)),
            discs_cache: crate::routes::discs::DiscsCache::new(
                Duration::from_secs(60),
                Duration::from_secs(60),
                Duration::from_secs(60),
            ),
            transliteration: Arc::new(
                crate::transliteration::TransliterationRegistry::new().unwrap(),
            ),
        }
    }

    #[tokio::test]
    async fn guest_disc_history_returns_unauthorized() {
        let app = routes().with_state(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/queue?disc_id=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn guest_submission_detail_returns_unauthorized_without_db_lookup() {
        let app = routes().with_state(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/queue/42")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    fn test_system(code: &str, name: &str) -> System {
        System {
            code: code.to_string(),
            system_type: "Console".to_string(),
            manufacturer: "Test".to_string(),
            name: name.to_string(),
            short_name: code.to_string(),
            media_types: vec!["DVD".to_string(), "BD".to_string()],
            has_exe_date: true,
            has_sbi: true,
            has_pvd: true,
            has_edc: true,
            has_disc_id: true,
            has_key: true,
            has_universal_hash: true,
            has_title_foreign: true,
            has_disc_title: true,
            has_disc_number: true,
            has_serial: true,
            has_barcode: true,
            has_version: true,
            has_edition: true,
            has_protection: true,
            has_sector_ranges: true,
            has_header: true,
            has_bca: true,
            has_sample_start: true,
            has_offset_extra: true,
            archives_dirty: false,
        }
    }

    fn ref_data() -> disc_edit::EditRefData {
        disc_edit::EditRefData {
            all_systems: vec![
                test_system("OLD", "Old System"),
                test_system("NEW", "New System"),
            ],
            all_media_types: vec![
                disc_edit::EditMediaTypeRow {
                    code: "DVD".to_string(),
                    name: "DVD-ROM".to_string(),
                    layer_count: 2,
                    pic: false,
                    rom_extension: "iso".to_string(),
                },
                disc_edit::EditMediaTypeRow {
                    code: "BD".to_string(),
                    name: "Blu-ray".to_string(),
                    layer_count: 2,
                    pic: false,
                    rom_extension: "iso".to_string(),
                },
            ],
            all_categories: vec![
                disc_edit::CategoryRow {
                    id: 1,
                    name: "Games".to_string(),
                },
                disc_edit::CategoryRow {
                    id: 2,
                    name: "Demos".to_string(),
                },
            ],
            all_regions: vec![
                Region {
                    code: "EU".to_string(),
                    name: "Europe".to_string(),
                    flag_code: "eu".to_string(),
                    sort_order: 0,
                },
                Region {
                    code: "JP".to_string(),
                    name: "Japan".to_string(),
                    flag_code: "jp".to_string(),
                    sort_order: 1,
                },
                Region {
                    code: "US".to_string(),
                    name: "USA".to_string(),
                    flag_code: "us".to_string(),
                    sort_order: 2,
                },
            ],
            all_languages: vec![
                Language {
                    code: "en".to_string(),
                    name: "English".to_string(),
                    flag_code: "gb".to_string(),
                    sort_order: 0,
                },
                Language {
                    code: "ja".to_string(),
                    name: "Japanese".to_string(),
                    flag_code: "jp".to_string(),
                    sort_order: 1,
                },
            ],
        }
    }

    fn old_snapshot() -> serde_json::Value {
        serde_json::json!({
            "system_code": "OLD",
            "media_type": "DVD",
            "title": "Old Game",
            "category": "Games",
            "title_foreign": "Old Foreign",
            "disc_number": "1",
            "disc_title": "Old Disc",
            "filename_suffix": "Old Suffix",
            "serial": ["OLD-001", "KEEP-002"],
            "version": "1.0",
            "edition": ["Original"],
            "barcode": ["111111111111"],
            "comments": "old comment",
            "contents": "old contents",
            "error_count": 1,
            "exe_date": "2020-01-01",
            "edc": true,
            "layerbreaks": [10, 20],
            "pvd": "old pvd",
            "pic": "old pic",
            "bca": "old bca",
            "header": "old header",
            "protection": "old protection",
            "sbi": "old sbi",
            "disc_id": "old-disc-id",
            "disc_key": "1234",
            "universal_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "cuesheet": "old cue",
            "status": "Unverified",
            "regions": ["EU", "US"],
            "languages": ["en"],
            "ring_codes": [],
            "sector_ranges": [{"start": 100, "end": 200}],
            "dat": "<rom name=\"old.iso\" size=\"1\" crc=\"11111111\" md5=\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" sha1=\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\" />"
        })
    }

    fn submitted_snapshot() -> serde_json::Value {
        serde_json::json!({
            "system_code": "NEW",
            "media_type": "BD",
            "title": "New Game",
            "category": "Demos",
            "title_foreign": "New Foreign",
            "disc_number": "2",
            "disc_title": "New Disc",
            "filename_suffix": "New Suffix",
            "serial": ["KEEP-002", "NEW-003"],
            "version": "2.0",
            "edition": ["Original", "Rerelease"],
            "barcode": ["222222222222"],
            "comments": "new comment",
            "contents": "new contents",
            "error_count": 2,
            "exe_date": "2024-01-01",
            "edc": false,
            "layerbreaks": [10, 30],
            "pvd": "new pvd",
            "pic": "new pic",
            "bca": "new bca",
            "header": "new header",
            "protection": "new protection",
            "sbi": "new sbi",
            "disc_id": "new-disc-id",
            "disc_key": "abcd",
            "universal_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "cuesheet": "new cue",
            "status": "Unverified",
            "regions": ["JP", "US"],
            "languages": ["en", "ja"],
            "ring_codes": [],
            "sector_ranges": [{"start": 150, "end": 250}],
            "dat": "<rom name=\"new.iso\" size=\"2\" crc=\"22222222\" md5=\"cccccccccccccccccccccccccccccccc\" sha1=\"dddddddddddddddddddddddddddddddddddddddd\" />"
        })
    }

    fn test_submission() -> DiscSubmission {
        DiscSubmission {
            id: 42,
            submission_type: SubmissionType::Edit,
            submitter_id: 7,
            submission_comment: None,
            target_disc_id: Some(1),
            changes_original: None,
            changes: serde_json::json!({}),
            dump_log: None,
            extra_upload_url: None,
            submission_token: None,
            submission_fingerprint: None,
            status: SubmissionStatus::Pending,
            reviewer_id: None,
            review_comment: None,
            created_at: chrono::Utc::now(),
            reviewed_at: None,
        }
    }

    fn queue_row(
        display_kind: SubmissionDisplayKind,
        status: SubmissionStatus,
    ) -> SubmissionListRow {
        let submission_type = match display_kind {
            SubmissionDisplayKind::Edit => SubmissionType::Edit,
            SubmissionDisplayKind::NewDisc | SubmissionDisplayKind::Verification => {
                SubmissionType::Disc
            }
        };

        SubmissionListRow {
            id: 42,
            submission_type,
            display_kind,
            title: "Test Game".to_string(),
            system_code: "DVD".to_string(),
            system_display: "DVD".to_string(),
            submitter: "submitter".to_string(),
            submitter_id: 7,
            reviewer: Some("moderator".to_string()),
            reviewer_id: Some(8),
            status,
            target_disc_id: Some(123),
            created_at: chrono::Utc::now(),
        }
    }

    fn annotation_values(template: &DiscEditTemplate, field: &str, label: &str) -> Vec<String> {
        template
            .review_annotations
            .iter()
            .find(|annotation| annotation.field == field && annotation.label == label)
            .map(|annotation| annotation.values.clone())
            .unwrap_or_default()
    }

    #[test]
    fn queue_type_filter_normalizes_public_values() {
        assert_eq!(normalize_queue_type_filter(None, false), "");
        assert_eq!(normalize_queue_type_filter(Some(""), false), "");
        assert_eq!(normalize_queue_type_filter(Some("Edit"), false), "Edit");
        assert_eq!(normalize_queue_type_filter(Some("edit"), false), "Edit");
        assert_eq!(
            normalize_queue_type_filter(Some("New Disc"), false),
            "New Disc"
        );
        assert_eq!(
            normalize_queue_type_filter(Some("new disc"), false),
            "New Disc"
        );
        assert_eq!(
            normalize_queue_type_filter(Some("Verification"), false),
            "Verification"
        );
        assert_eq!(
            normalize_queue_type_filter(Some("verification"), false),
            "Verification"
        );
        assert_eq!(normalize_queue_type_filter(Some("Disc"), false), "");
        assert_eq!(normalize_queue_type_filter(Some("Unknown"), false), "");
        assert_eq!(normalize_queue_type_filter(Some("Verification"), true), "");
    }

    #[test]
    fn queue_url_filters_are_case_insensitive() {
        assert_eq!(
            normalize_queue_status_filter(Some("approved"), false),
            "Approved"
        );
        assert_eq!(
            normalize_queue_status_filter(Some("ALL STATUSES"), false),
            "All Statuses"
        );
        assert_eq!(
            normalize_queue_status_filter(Some("approved"), true),
            "All Visible"
        );
        assert_eq!(normalize_queue_sort(Some("DISC_ID")), "disc_id");
        assert_eq!(normalize_queue_order(Some("ASC")), "asc");
        assert_eq!(normalize_queue_order(Some("DESC")), "desc");
    }

    #[tokio::test]
    async fn approved_disc_submission_with_dat_add_displays_as_new_disc() {
        let mut sub = test_submission();
        sub.submission_type = SubmissionType::Disc;
        sub.target_disc_id = Some(123);
        sub.status = SubmissionStatus::Approved;
        sub.changes = serde_json::json!({
            "dat": { "add": { "new": "<rom name=\"Track.iso\" />" } }
        });

        assert_eq!(sub.display_kind(), SubmissionDisplayKind::NewDisc);

        let Html(html) = render_readonly_detail(
            Some(auth_user(8, UserRole::Moderator)),
            &sub,
            "submitter",
            "moderator",
        )
        .await
        .unwrap();

        assert!(html.contains("<td><strong>Submission Type</strong></td><td>New Disc</td>"));
    }

    #[tokio::test]
    async fn disc_submission_without_dat_add_displays_as_verification() {
        let mut sub = test_submission();
        sub.submission_type = SubmissionType::Disc;
        sub.target_disc_id = Some(123);
        sub.status = SubmissionStatus::Approved;
        sub.changes = serde_json::json!({
            "title": { "add": { "new": "Test Game" } }
        });

        assert_eq!(sub.display_kind(), SubmissionDisplayKind::Verification);

        let Html(html) = render_readonly_detail(
            Some(auth_user(8, UserRole::Moderator)),
            &sub,
            "submitter",
            "moderator",
        )
        .await
        .unwrap();

        assert!(html.contains("<td><strong>Submission Type</strong></td><td>Verification</td>"));
    }

    #[test]
    fn queue_type_icon_classes_keep_type_colors_except_legacy() {
        let approved_new_disc =
            queue_row(SubmissionDisplayKind::NewDisc, SubmissionStatus::Approved);
        assert_eq!(queue_type_icon_label(&approved_new_disc), "New Disc");
        assert_eq!(
            queue_type_icon_class(&approved_new_disc),
            "submission-type-icon submission-type-icon-disc submission-type-icon-new-disc"
        );

        let rejected_verification = queue_row(
            SubmissionDisplayKind::Verification,
            SubmissionStatus::Rejected,
        );
        assert_eq!(
            queue_type_icon_label(&rejected_verification),
            "Verification"
        );
        assert_eq!(
            queue_type_icon_class(&rejected_verification),
            "submission-type-icon submission-type-icon-disc submission-type-icon-verification"
        );

        let approved_edit = queue_row(SubmissionDisplayKind::Edit, SubmissionStatus::Approved);
        assert_eq!(queue_type_icon_label(&approved_edit), "Edit");
        assert_eq!(
            queue_type_icon_class(&approved_edit),
            "submission-type-icon submission-type-icon-edit submission-type-icon-edit-pending"
        );

        let legacy_new_disc = queue_row(SubmissionDisplayKind::NewDisc, SubmissionStatus::Legacy);
        assert_eq!(
            queue_type_icon_class(&legacy_new_disc),
            "submission-type-icon submission-type-icon-disc submission-type-icon-processed"
        );
    }

    fn auth_user(id: i32, role: UserRole) -> AuthenticatedUser {
        AuthenticatedUser {
            id,
            username: format!("user{id}"),
            role,
            csrf_token: "test-csrf-token".to_string(),
            avatar_url: None,
        }
    }

    #[test]
    fn review_permission_allows_moderators_and_own_user_plus_verification_only() {
        let mut sub = test_submission();
        sub.submission_type = SubmissionType::Disc;
        sub.target_disc_id = Some(1);
        sub.submitter_id = 7;

        assert!(can_review_submission(
            &auth_user(7, UserRole::UserPlus),
            &sub
        ));
        assert!(!can_review_submission(
            &auth_user(8, UserRole::UserPlus),
            &sub
        ));
        assert!(!can_review_submission(&auth_user(7, UserRole::User), &sub));
        assert!(can_review_submission(
            &auth_user(8, UserRole::Moderator),
            &sub
        ));
        assert!(can_review_submission(&auth_user(8, UserRole::Admin), &sub));

        sub.target_disc_id = None;
        assert!(!can_review_submission(
            &auth_user(7, UserRole::UserPlus),
            &sub
        ));

        sub.target_disc_id = Some(1);
        sub.submission_type = SubmissionType::Edit;
        assert!(!can_review_submission(
            &auth_user(7, UserRole::UserPlus),
            &sub
        ));
    }

    fn selected_system(template: &DiscEditTemplate) -> String {
        template
            .systems
            .iter()
            .find(|system| system.selected)
            .map(|system| system.code.clone())
            .unwrap_or_default()
    }

    fn selected_media(template: &DiscEditTemplate) -> String {
        template
            .media_types_all
            .iter()
            .find(|media| media.selected)
            .map(|media| media.code.clone())
            .unwrap_or_default()
    }

    fn selected_category(template: &DiscEditTemplate) -> String {
        template
            .categories
            .iter()
            .find(|category| category.selected)
            .map(|category| category.value.clone())
            .unwrap_or_default()
    }

    fn build_template(snapshot: &serde_json::Value) -> DiscEditTemplate {
        let ref_data = ref_data();
        let mut template = build_review_template(
            AuthenticatedUser::template_only("moderator"),
            &test_submission(),
            "submitter",
            "",
            snapshot,
            &ref_data,
            "{}",
            "{}",
            "{}",
            "{}",
            "{}",
            "{}",
            "{}",
            snapshot["system_code"].as_str().unwrap_or(""),
            snapshot["media_type"].as_str().unwrap_or(""),
            2,
            |_flag: fn(&System) -> bool| true,
        );
        template.review_base_hash = queue_service::disc_snapshot_hash(&old_snapshot());
        template
    }

    #[test]
    fn queue_detail_template_preserves_current_user_avatar() {
        let template = QueueDetailTemplate {
            current_user: Some(AuthenticatedUser {
                id: 42,
                username: "moderator".to_string(),
                role: UserRole::Moderator,
                csrf_token: "test-csrf-token".to_string(),
                avatar_url: Some("https://example.test/avatar.png".to_string()),
            }),
            submission_id: 1,
            submission_type_display: "Edit".to_string(),
            submitter_id: 2,
            submitter_name: "submitter".to_string(),
            submission_comment: String::new(),
            dump_log_display: String::new(),
            extra_upload_url_display: String::new(),
            submission_status: "Approved".to_string(),
            reviewer_id: 42,
            reviewer_name: "moderator".to_string(),
            review_comment_display: String::new(),
            created_at_display: "2026-01-01 00:00 UTC".to_string(),
            reviewed_at_display: String::new(),
            target_disc_id: 3,
            changes_original_json: String::new(),
            changes_json: "{}".to_string(),
        };

        let html = template.render().unwrap();

        assert!(html.contains(r#"src="https://example.test/avatar.png""#));
    }

    #[test]
    fn queue_detail_template_shows_original_json_before_final_json() {
        let template = QueueDetailTemplate {
            current_user: None,
            submission_id: 1,
            submission_type_display: "Edit".to_string(),
            submitter_id: 2,
            submitter_name: "submitter".to_string(),
            submission_comment: String::new(),
            dump_log_display: String::new(),
            extra_upload_url_display: String::new(),
            submission_status: "Approved".to_string(),
            reviewer_id: 42,
            reviewer_name: "moderator".to_string(),
            review_comment_display: String::new(),
            created_at_display: "2026-01-01 00:00 UTC".to_string(),
            reviewed_at_display: "2026-01-02 00:00 UTC".to_string(),
            target_disc_id: 3,
            changes_original_json: "{\n  \"status\": \"original\"\n}".to_string(),
            changes_json: "{\n  \"status\": \"final\"\n}".to_string(),
        };

        let html = template.render().unwrap();

        let original_pos = html.find("Original JSON").unwrap();
        let final_pos = html.find("Final JSON").unwrap();
        assert!(original_pos < final_pos);
        assert!(!html.contains("Raw JSON"));
        assert!(html.contains("2026-01-01 00:00 UTC"));
        assert!(html.contains("2026-01-02 00:00 UTC"));
    }

    #[test]
    fn review_template_shows_final_json_without_original_json_when_absent() {
        let html = build_template(&submitted_snapshot()).render().unwrap();

        assert!(!html.contains("Original JSON"));
        assert!(html.contains("Final JSON"));
        assert!(!html.contains("Raw JSON"));
    }

    #[test]
    fn changes_original_json_hides_none_and_json_null() {
        assert_eq!(pretty_optional_json(None), "");
        assert_eq!(pretty_optional_json(Some(&serde_json::Value::Null)), "");
    }

    #[test]
    fn review_disc_edit_form_includes_csrf_field() {
        let html = build_template(&submitted_snapshot()).render().unwrap();

        assert!(html.contains(r#"<form method="post" action="/queue/42/review""#));
        assert!(html.contains(r#"<input type="hidden" name="_csrf" value="test-csrf-token">"#));
        assert!(html.contains(" UTC"));
    }

    #[test]
    fn review_disc_edit_form_includes_base_hash_for_target_disc() {
        let hash = queue_service::disc_snapshot_hash(&old_snapshot());
        let html = build_template(&submitted_snapshot()).render().unwrap();

        assert!(html.contains(&format!(
            r#"<input type="hidden" name="review_base_hash" value="{hash}">"#
        )));
    }

    #[test]
    fn stale_review_reload_uses_latest_db_and_stored_changes_not_posted_fields() {
        let mut latest_db = old_snapshot();
        latest_db["title"] = serde_json::json!("Latest DB Title");
        let mut sub = test_submission();
        sub.changes = serde_json::json!({
            "title": {
                "modify": {
                    "old": "Historical Old Title That Does Not Match Current DB",
                    "new": "Stored Submission Title"
                }
            }
        });
        let mut posted_snapshot =
            queue_service::resolve_submission_snapshot_for_submission(&old_snapshot(), &sub)
                .unwrap();
        posted_snapshot["title"] = serde_json::json!("Moderator Posted Stale Title");

        let reloaded_snapshot =
            queue_service::resolve_submission_snapshot_for_submission(&latest_db, &sub).unwrap();

        assert_eq!(
            reloaded_snapshot["title"],
            serde_json::json!("Stored Submission Title")
        );
        assert_ne!(reloaded_snapshot["title"], posted_snapshot["title"]);
    }

    #[test]
    fn review_disc_initial_values_prefer_old_identity_fields_and_annotate_submitted_values() {
        let db = old_snapshot();
        let submitted = submitted_snapshot();
        let ref_data = ref_data();
        let mut template = build_template(&submitted);

        apply_review_diff_context(
            &mut template,
            &submitted,
            &db,
            &ref_data,
            SubmissionType::Disc,
            true,
        );

        assert_eq!(template.system_code, "OLD");
        assert_eq!(template.media_type_code, "DVD");
        assert_eq!(selected_system(&template), "OLD");
        assert_eq!(selected_media(&template), "DVD");
        assert_eq!(selected_category(&template), "Games");
        assert_eq!(template.title, "Old Game");
        assert_eq!(template.title_foreign, "Old Foreign");
        assert_eq!(template.disc_number, "1");
        assert_eq!(template.disc_title, "Old Disc");
        assert_eq!(template.filename_suffix, "Old Suffix");
        assert_eq!(
            annotation_values(&template, "title", "Changed to"),
            vec!["New Game".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "system_code", "Changed to"),
            vec!["Test New System".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "media_type", "Changed to"),
            vec!["Blu-ray".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "category", "Changed to"),
            vec!["Demos".to_string()]
        );
    }

    #[test]
    fn review_edit_initial_values_keep_submitted_identity_fields_and_annotate_old_values() {
        let db = old_snapshot();
        let submitted = submitted_snapshot();
        let ref_data = ref_data();
        let mut template = build_template(&submitted);

        apply_review_diff_context(
            &mut template,
            &submitted,
            &db,
            &ref_data,
            SubmissionType::Edit,
            true,
        );

        assert_eq!(template.system_code, "NEW");
        assert_eq!(template.media_type_code, "BD");
        assert_eq!(selected_system(&template), "NEW");
        assert_eq!(selected_media(&template), "BD");
        assert_eq!(selected_category(&template), "Demos");
        assert_eq!(template.title, "New Game");
        assert_eq!(template.title_foreign, "New Foreign");
        assert_eq!(template.disc_number, "2");
        assert_eq!(template.disc_title, "New Disc");
        assert_eq!(template.filename_suffix, "New Suffix");
        assert_eq!(
            annotation_values(&template, "title", "Changed from"),
            vec!["Old Game".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "system_code", "Changed from"),
            vec!["Test Old System".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "media_type", "Changed from"),
            vec!["DVD-ROM".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "category", "Changed from"),
            vec!["Games".to_string()]
        );
    }

    #[test]
    fn review_annotations_include_added_removed_sets_and_old_multiline_sidecars() {
        let db = old_snapshot();
        let submitted = submitted_snapshot();
        let ref_data = ref_data();
        let mut template = build_template(&submitted);

        apply_review_diff_context(
            &mut template,
            &submitted,
            &db,
            &ref_data,
            SubmissionType::Disc,
            true,
        );

        assert_eq!(
            annotation_values(&template, "regions", "Removed"),
            vec!["Europe".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "regions", "Added"),
            vec!["Japan".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "serial", "Removed"),
            vec!["OLD-001".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "serial", "Added"),
            vec!["NEW-003".to_string()]
        );
        assert_eq!(
            annotation_values(&template, "universal_hash", "Changed from"),
            vec!["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()]
        );
        assert!(template
            .review_old_multiline
            .iter()
            .any(|old| old.field == "contents" && old.value == "old contents"));
        assert!(template
            .review_old_multiline
            .iter()
            .any(|old| old.field == "cue" && old.value == "old cue"));
        assert!(template
            .review_old_multiline
            .iter()
            .any(|old| { old.field == "files_xml" && old.value.contains("old.iso") }));
        assert!(!template
            .review_old_multiline
            .iter()
            .any(|old| old.field == "comments"));
    }

    #[test]
    fn review_comments_are_additive_only_on_initial_display() {
        let db = old_snapshot();
        let submitted = submitted_snapshot();
        let ref_data = ref_data();
        let mut template = build_template(&submitted);

        apply_review_diff_context(
            &mut template,
            &submitted,
            &db,
            &ref_data,
            SubmissionType::Disc,
            true,
        );

        assert_eq!(
            template.comments,
            format!("old comment\n\n{COMMENTS_REVIEW_DELIMITER}\n\nnew comment")
        );

        let mut posted = submitted.clone();
        posted["title"] = serde_json::json!("Moderator Title");
        posted["comments"] = serde_json::json!("moderator edited comments");
        let mut posted_template = build_template(&posted);
        apply_review_diff_context(
            &mut posted_template,
            &submitted,
            &db,
            &ref_data,
            SubmissionType::Edit,
            false,
        );

        assert_eq!(posted_template.title, "Moderator Title");
        assert_eq!(posted_template.comments, "moderator edited comments");
    }

    #[test]
    fn universal_hash_highlights_add_change_and_remove() {
        let mut db = old_snapshot();
        let mut submitted = db.clone();
        submitted["universal_hash"] = serde_json::json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");

        let highlights = compute_field_highlights(&submitted, &db);
        assert!(highlights
            .changed_fields
            .contains(&"universal_hash:changed".to_string()));

        db["universal_hash"] = serde_json::Value::Null;
        let highlights = compute_field_highlights(&submitted, &db);
        assert!(highlights
            .changed_fields
            .contains(&"universal_hash:added".to_string()));

        db["universal_hash"] = serde_json::json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        submitted["universal_hash"] = serde_json::Value::Null;
        let highlights = compute_field_highlights(&submitted, &db);
        assert!(highlights
            .changed_fields
            .contains(&"universal_hash:removed".to_string()));
    }

    #[test]
    fn review_ignores_hexadecimal_case_only_submission_changes() {
        let mut db = old_snapshot();
        db["disc_id"] = serde_json::json!("aabbccdd");
        db["disc_key"] = serde_json::json!("aabbccdd");
        db["sbi"] = serde_json::json!("MSF: 02:03:04 Q-Data: A1B2C3 0A:0B:0C 00 0D:0E:0F ABCD");
        let changes = serde_json::json!({
            "disc_id": {"modify": {"old": "aabbccdd", "new": "AABBCCDD"}},
            "disc_key": {"modify": {"old": "aabbccdd", "new": "AABBCCDD"}},
            "universal_hash": {"modify": {"old": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "new": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}},
            "sbi": {"modify": {"old": db["sbi"], "new": "MSF: 02:03:04 Q-Data: a1b2c3 0a:0b:0c 00 0d:0e:0f abcd"}},
            "dat": {"modify": {"old": db["dat"], "new": "<rom name=\"old.iso\" size=\"1\" crc=\"11111111\" md5=\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\" sha1=\"BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB\" />"}}
        });
        let canonical_db =
            queue_service::resolve_submission_snapshot(&db, &serde_json::json!({})).unwrap();
        let submitted = queue_service::resolve_submission_snapshot(&db, &changes).unwrap();

        let highlights = compute_field_highlights(&submitted, &canonical_db);
        let context =
            build_review_diff_context(&submitted, &canonical_db, &ref_data(), SubmissionType::Edit);

        assert!(highlights.changed_fields.is_empty());
        assert!(context.annotations.is_empty());
        assert!(context.old_multiline.is_empty());
    }

    #[test]
    fn comments_delimiter_validation_detects_unreviewed_comments() {
        assert!(comments_text_contains_review_delimiter(Some(&format!(
            "old\n\n{COMMENTS_REVIEW_DELIMITER}\n\nnew"
        ))));
        assert!(!comments_text_contains_review_delimiter(Some("old\n\nnew")));
        assert!(!comments_text_contains_review_delimiter(None));
    }

    #[test]
    fn review_textarea_assets_autosize_sidecars_without_manual_resize_wrapper() {
        let css = include_str!("../../static/css/app.css");
        assert!(css.contains("textarea.auto-expand {\n    overflow: hidden;\n    resize: none;\n}"));
        assert!(!css.contains("textarea-resize"));
        assert!(css.contains(".review-field-annotation {\n    flex: 0 0 100%;\n    width: 100%;\n    display: flex;\n    flex-wrap: nowrap;\n    align-items: center;\n    gap: 0.25rem;\n    overflow-x: auto;\n    white-space: nowrap;\n}"));
        assert!(
            css.contains(".multiline-review-field {\n    display: flex;\n    flex-wrap: nowrap;")
        );
        assert!(css.contains(".inline-field-values > .review-field-annotation"));
        assert!(css.contains(
            ".inline-field-values > .review-field-annotation-combined {\n    flex-basis: 100%;\n    width: 100%;\n}"
        ));

        let template = include_str!("../../templates/disc_edit.html");
        assert!(template.contains("review-field-annotation-combined"));
        assert!(template.contains("{% if !loop.first %}, {% endif %}{{ ann.label }}:"));
        assert!(!template.contains("review-field-annotation-separator"));
        assert!(template.contains("self.has_annotations_for(\"regions\")"));
        assert!(template.contains("self.has_annotations_for(\"languages\")"));
        assert!(template.contains("self.has_annotations_for(\"serial\")"));
        assert!(template.contains("self.has_annotations_for(\"edition\")"));
        assert!(template.contains("self.has_annotations_for(\"barcode\")"));
        assert!(template.contains("class=\"review-old-textarea\""));
        assert!(template.contains(
            "<textarea rows=\"5\" class=\"hex-dump-input auto-expand fixed-80\" readonly>{{ self.old_multiline(\"contents\") }}</textarea>"
        ));
        assert!(css.contains(
            ".review-field-annotation-combined {\n    flex-basis: auto;\n    width: auto;\n}"
        ));

        let js = include_str!("../../static/js/disc_edit.js");
        assert!(!js.contains("initManualTextareaResize"));
        assert!(!js.contains("manualResized"));
        assert!(!js.contains("textarea-resize"));
        assert!(js.contains("container.insertBefore(input, annotation);"));
        assert!(js.contains("cueField.querySelectorAll('textarea')"));
        assert!(js.contains("autoExpand(ta);"));
    }
}

impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for SubmissionListRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        let submission_type: SubmissionType = row.try_get("submission_type")?;
        let submission_has_dat_add: bool = row.try_get("submission_has_dat_add")?;
        let system_code: String = row.try_get("system_code")?;
        let system_short_name: Option<String> = row.try_get("system_short_name").ok();
        let system_display = crate::db::models::short_system_display(
            system_short_name.as_deref().unwrap_or(""),
            &system_code,
        );
        Ok(Self {
            id: row.try_get("id")?,
            submission_type,
            display_kind: SubmissionDisplayKind::from_parts(
                submission_type,
                submission_has_dat_add,
            ),
            title: row.try_get("title")?,
            system_code,
            system_display,
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
