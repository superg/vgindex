use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use axum_extra::extract::Form;
use serde::Deserialize;

use crate::auth::middleware::RequireAuth;
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::{disc_service, queue_service, validation};
use crate::AppState;

fn one_or_many_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct Visitor;
    impl<'de> de::Visitor<'de> for Visitor {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or sequence of strings")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }
        fn visit_seq<S: de::SeqAccess<'de>>(self, mut seq: S) -> Result<Vec<String>, S::Error> {
            let mut v = Vec::new();
            while let Some(s) = seq.next_element()? {
                v.push(s);
            }
            Ok(v)
        }
    }

    deserializer.deserialize_any(Visitor)
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/disc/{id}/edit", get(edit_page).post(edit_submit))
        .route("/disc/{id}/edit/", get(edit_page).post(edit_submit))
        .route("/disc/submit", get(add_page).post(add_submit))
        .route("/disc/submit/", get(add_page).post(add_submit))
}

fn user_queue_url(username: &str) -> String {
    format!("/queue/?submitter={}", urlencoding::encode(username))
}

#[derive(Template)]
#[template(path = "disc_edit.html")]
pub(crate) struct DiscEditTemplate {
    pub current_user: Option<String>,
    pub disc_id: i32,
    pub page_title: String,

    pub systems: Vec<SystemOption>,
    pub media_types_all: Vec<MediaTypeOption>,
    pub categories: Vec<SelectOption>,
    pub regions: Vec<CheckOption>,
    pub languages: Vec<CheckOption>,

    pub system_code: String,
    pub media_type_code: String,
    pub max_layers: u32,
    pub media_layers_json: String,
    pub systems_media_json: String,
    pub systems_has_flags_json: String,
    pub edition_suggestions_json: String,
    pub submit_as_usernames_json: String,
    pub media_rom_extensions_json: String,
    pub media_is_cd_json: String,

    pub title: String,
    pub show_title_foreign: bool,
    pub title_foreign: String,
    pub show_disc_number: bool,
    pub disc_number: String,
    pub show_disc_title: bool,
    pub disc_title: String,
    pub filename_suffix: String,

    pub show_serial: bool,
    pub serials: Vec<HighlightedValue>,
    pub show_version: bool,
    pub version: String,
    pub show_edition: bool,
    pub editions: Vec<HighlightedValue>,
    pub show_barcode: bool,
    pub barcodes: Vec<HighlightedValue>,

    pub ring_codes_json: String,
    pub ring_highlights_json: String,

    pub comments: String,
    pub contents: String,

    pub show_error_count: bool,
    pub error_count: String,
    pub show_exe_date: bool,
    pub exe_date: String,
    pub show_edc: bool,
    pub edc_value: String,

    pub layerbreaks: Vec<String>,
    pub show_pvd: bool,
    pub pvd_hex: String,
    pub show_pic: bool,
    pub media_has_pic_json: String,
    pub pic_hex: String,
    pub show_bca: bool,
    pub bca_hex: String,
    pub show_header: bool,
    pub header_hex: String,

    pub show_disc_id: bool,
    pub show_key: bool,
    pub show_protection: bool,
    pub protection: String,
    pub show_sector_ranges: bool,
    pub sector_ranges_text: String,
    pub show_sbi: bool,
    pub sbi: String,
    pub protection_key_disc_key: String,
    pub protection_key_disc_id: String,
    pub has_sample_start: bool,

    pub cue: String,
    pub files_xml: String,

    pub status: String,

    pub is_add_mode: bool,
    pub dump_log: String,
    pub dump_log_required: bool,
    pub extra_upload_url: String,
    pub show_submit_as: bool,
    pub submit_as_username: String,

    pub submit_button_text: String,
    pub validation_errors: Vec<String>,
    pub linked_validation_errors: Vec<LinkedValidationError>,
    pub validation_result: String,
    pub validation_result_disc_id: i32,
    pub validation_result_disc_title: String,

    pub is_review_mode: bool,
    pub changed_fields: Vec<String>,
    pub review_annotations: Vec<ReviewAnnotation>,
    pub review_old_multiline: Vec<ReviewOldMultiline>,
    pub submission_id: i32,
    pub submission_type_display: String,
    pub submitter_id: i32,
    pub submitter_name: String,
    pub submission_comment: String,
    pub dump_log_display: String,
    pub extra_upload_url_display: String,
    pub submission_status: String,
    pub reviewer_id: i32,
    pub reviewer_name: String,
    pub review_comment_display: String,
    pub review_comment_input: String,
    pub created_at_display: String,
    pub reviewed_at_display: String,
    pub changes_json: String,
}
impl SiteConfig for DiscEditTemplate {}

impl DiscEditTemplate {
    pub fn highlight_class(&self, field: &str) -> &str {
        for f in &self.changed_fields {
            if f.len() > field.len() && f.as_bytes()[field.len()] == b':' && f.starts_with(field) {
                return match &f[field.len() + 1..] {
                    "changed" => "field-changed",
                    "added" => "field-added",
                    "removed" => "field-removed",
                    _ => "",
                };
            }
        }
        ""
    }

    pub fn annotations_for(&self, field: &str) -> Vec<ReviewAnnotation> {
        self.review_annotations
            .iter()
            .filter(|annotation| annotation.field == field)
            .cloned()
            .collect()
    }

    pub fn has_annotations_for(&self, field: &str) -> bool {
        self.review_annotations
            .iter()
            .any(|annotation| annotation.field == field)
    }

    pub fn has_old_multiline(&self, field: &str) -> bool {
        self.review_old_multiline
            .iter()
            .any(|old_text| old_text.field == field)
    }

    pub fn old_multiline(&self, field: &str) -> String {
        self.review_old_multiline
            .iter()
            .find(|old_text| old_text.field == field)
            .map(|old_text| old_text.value.clone())
            .unwrap_or_default()
    }
}

pub(crate) struct SystemOption {
    pub code: String,
    pub name: String,
    pub selected: bool,
}

pub(crate) struct MediaTypeOption {
    pub code: String,
    pub name: String,
    pub selected: bool,
}

pub(crate) struct SelectOption {
    pub value: String,
    pub name: String,
    pub selected: bool,
}

pub(crate) struct CheckOption {
    pub value: String,
    pub name: String,
    pub code: String,
    pub selected: bool,
    pub highlight: String,
    pub common: bool,
}

pub(crate) struct HighlightedValue {
    pub value: String,
    pub highlight: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewAnnotation {
    pub field: String,
    pub label: String,
    pub kind: String,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewOldMultiline {
    pub field: String,
    pub value: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct EditMediaTypeRow {
    pub code: String,
    pub name: String,
    pub layer_count: i32,
    pub pic: bool,
    pub rom_extension: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct CategoryRow {
    #[allow(dead_code)]
    pub id: i32,
    pub name: String,
}

pub(crate) struct EditRefData {
    pub all_systems: Vec<System>,
    pub all_media_types: Vec<EditMediaTypeRow>,
    pub all_categories: Vec<CategoryRow>,
    pub all_regions: Vec<Region>,
    pub all_languages: Vec<Language>,
}

pub(crate) async fn fetch_ref_data(pool: &sqlx::PgPool) -> AppResult<EditRefData> {
    let all_systems = disc_service::get_all_systems(pool).await?;
    let all_media_types: Vec<EditMediaTypeRow> = sqlx::query_as(
        "SELECT code, name, layer_count, pic, rom_extension FROM media_types ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    let all_categories: Vec<CategoryRow> =
        sqlx::query_as("SELECT id, name FROM categories ORDER BY name")
            .fetch_all(pool)
            .await?;
    let all_regions: Vec<Region> = sqlx::query_as("SELECT * FROM regions ORDER BY sort_order")
        .fetch_all(pool)
        .await?;
    let all_languages: Vec<Language> =
        sqlx::query_as("SELECT * FROM languages ORDER BY sort_order")
            .fetch_all(pool)
            .await?;
    Ok(EditRefData {
        all_systems,
        all_media_types,
        all_categories,
        all_regions,
        all_languages,
    })
}

pub(crate) fn build_systems_json(all_systems: &[System]) -> (String, String) {
    let mut systems_media_map = serde_json::Map::new();
    let mut systems_has_flags_map = serde_json::Map::new();
    for s in all_systems {
        systems_media_map.insert(s.code.clone(), serde_json::json!(s.media_types));
        systems_has_flags_map.insert(
            s.code.clone(),
            serde_json::json!({
                "has_title_foreign": s.has_title_foreign,
                "has_disc_number": s.has_disc_number,
                "has_disc_title": s.has_disc_title,
                "has_serial": s.has_serial,
                "has_version": s.has_version,
                "has_edition": s.has_edition,
                "has_barcode": s.has_barcode,
                "has_exe_date": s.has_exe_date,
                "has_edc": s.has_edc,
                "has_disc_id": s.has_disc_id,
                "has_key": s.has_key,
                "has_pvd": s.has_pvd,
                "has_bca": s.has_bca,
                "has_header": s.has_header,
                "has_protection": s.has_protection,
                "has_sector_ranges": s.has_sector_ranges,
                "has_sbi": s.has_sbi,
                "has_sample_start": s.has_sample_start,
                "has_offset_extra": s.has_offset_extra,
            }),
        );
    }
    let systems_media_json =
        serde_json::to_string(&systems_media_map).unwrap_or_else(|_| "{}".into());
    let systems_has_flags_json =
        serde_json::to_string(&systems_has_flags_map).unwrap_or_else(|_| "{}".into());
    (systems_media_json, systems_has_flags_json)
}

pub(crate) async fn build_edition_suggestions_json(state: &AppState) -> AppResult<String> {
    let suggestions = state.edition_suggestions.get(&state.pool).await?;
    Ok(serde_json::to_string(&suggestions).unwrap_or_else(|_| "{}".into()))
}

pub(crate) async fn build_submit_as_usernames_json(pool: &sqlx::PgPool) -> AppResult<String> {
    let usernames: Vec<String> =
        sqlx::query_scalar("SELECT username FROM users ORDER BY LOWER(username), username")
            .fetch_all(pool)
            .await?;
    Ok(serde_json::to_string(&usernames).unwrap_or_else(|_| "[]".into()))
}

const FIND_OR_CREATE_USER_SQL: &str = "INSERT INTO users (username)
         VALUES ($1)
         ON CONFLICT (username) DO UPDATE SET username = EXCLUDED.username
         RETURNING id";

fn normalize_submit_as_username(username: Option<&str>) -> Result<String, &'static str> {
    let username = username.unwrap_or("").trim();
    if username.is_empty() {
        return Err("cannot be empty");
    }
    if username.chars().count() > 64 {
        return Err("must be 64 characters or fewer");
    }
    Ok(username.to_string())
}

fn trimmed_nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn is_valid_http_url(value: &str) -> bool {
    reqwest::Url::parse(value)
        .map(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .unwrap_or(false)
}

fn validate_add_submission_logs(form: &DiscEditForm, logs_optional: bool) -> Vec<String> {
    let dump_log = trimmed_nonempty(form.dump_log.as_deref());
    let logs_url = trimmed_nonempty(form.extra_upload_url.as_deref());

    let mut errors = Vec::new();
    if !logs_optional && dump_log.is_none() {
        errors.push("Dump Log: cannot be empty".to_string());
    }
    if !logs_optional && logs_url.is_none() {
        errors.push("Logs Archive URL: cannot be empty".to_string());
    }
    if let Some(logs_url) = logs_url {
        if !is_valid_http_url(logs_url) {
            errors.push("Logs Archive URL: must be a valid URL".to_string());
        }
    }
    errors
}

fn generated_name_key(name: &str) -> String {
    name.to_lowercase()
}

fn generated_disc_name(
    title: &str,
    region_names: &[String],
    language_codes: &[String],
    disc_number: Option<&str>,
    disc_title: Option<&str>,
    filename_suffix: Option<&str>,
) -> String {
    build_rom_base_name(
        title.trim(),
        region_names,
        language_codes,
        disc_number.map(str::trim).filter(|s| !s.is_empty()),
        disc_title.map(str::trim).filter(|s| !s.is_empty()),
        filename_suffix.map(str::trim).filter(|s| !s.is_empty()),
    )
}

async fn selected_region_names(
    pool: &sqlx::PgPool,
    region_codes: &[String],
) -> AppResult<Vec<String>> {
    let region_codes = norm_str_vec(region_codes.to_vec());
    if region_codes.is_empty() {
        return Ok(Vec::new());
    }

    Ok(sqlx::query_scalar::<_, String>(
        "SELECT name FROM regions WHERE code = ANY($1) ORDER BY sort_order",
    )
    .bind(&region_codes)
    .fetch_all(pool)
    .await?)
}

async fn selected_language_codes(
    pool: &sqlx::PgPool,
    language_codes: &[String],
) -> AppResult<Vec<String>> {
    let language_codes = norm_str_vec(language_codes.to_vec());
    if language_codes.is_empty() {
        return Ok(Vec::new());
    }

    Ok(sqlx::query_scalar::<_, String>(
        "SELECT code FROM languages WHERE code = ANY($1) ORDER BY sort_order",
    )
    .bind(&language_codes)
    .fetch_all(pool)
    .await?)
}

pub(crate) async fn validate_generated_name_unique(
    pool: &sqlx::PgPool,
    form: &DiscEditForm,
    current_disc_id: Option<i32>,
    proposed_is_active: bool,
) -> AppResult<Vec<LinkedValidationError>> {
    if !proposed_is_active {
        return Ok(Vec::new());
    }
    let system_code = form.system_code.trim();
    if system_code.is_empty() || form.title.trim().is_empty() {
        return Ok(Vec::new());
    }

    let region_names = selected_region_names(pool, &form.regions).await?;
    let language_codes = selected_language_codes(pool, &form.languages).await?;
    let proposed_name = generated_disc_name(
        &form.title,
        &region_names,
        &language_codes,
        form.disc_number.as_deref(),
        form.disc_title.as_deref(),
        form.filename_suffix.as_deref(),
    );
    let proposed_key = generated_name_key(&proposed_name);

    let candidates: Vec<DuplicateNameDiscRow> = sqlx::query_as(
        "SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix,
                COALESCE((
                    SELECT array_agg(r.name ORDER BY r.sort_order)
                    FROM disc_regions dr
                    JOIN regions r ON r.code = dr.region_code
                    WHERE dr.disc_id = d.id
                ), ARRAY[]::TEXT[]) AS region_names,
                COALESCE((
                    SELECT array_agg(l.code ORDER BY l.sort_order)
                    FROM disc_languages dl
                    JOIN languages l ON l.code = dl.language_code
                    WHERE dl.disc_id = d.id
                ), ARRAY[]::TEXT[]) AS language_codes
         FROM discs d
         WHERE d.system_code = $1
           AND d.status != 'Disabled'
           AND ($2::INT IS NULL OR d.id <> $2)
         ORDER BY d.id",
    )
    .bind(system_code)
    .bind(current_disc_id)
    .fetch_all(pool)
    .await?;

    for candidate in candidates {
        let candidate_name = generated_disc_name(
            &candidate.title,
            &candidate.region_names,
            &candidate.language_codes,
            candidate.disc_number.as_deref(),
            candidate.disc_title.as_deref(),
            candidate.filename_suffix.as_deref(),
        );
        if generated_name_key(&candidate_name) == proposed_key {
            return Ok(vec![LinkedValidationError {
                text: "Generated name already exists:".to_string(),
                disc_id: candidate.id,
                disc_title: candidate_name,
            }]);
        }
    }

    Ok(Vec::new())
}

pub(crate) fn form_status_is_active(form: &DiscEditForm) -> bool {
    normalized_disc_status(&form.status) != "Disabled"
}

fn validate_submit_as_for_add(form: &DiscEditForm, can_submit_as: bool) -> Vec<String> {
    if !can_submit_as {
        return Vec::new();
    }
    match normalize_submit_as_username(form.submit_as.as_deref()) {
        Ok(_) => Vec::new(),
        Err(message) => vec![format!("Submit As: {message}")],
    }
}

fn submit_as_username_for_form(username: &str, form: &DiscEditForm, can_submit_as: bool) -> String {
    if !can_submit_as {
        return String::new();
    }
    form.submit_as.as_deref().unwrap_or(username).to_string()
}

async fn find_or_create_submit_as_user(pool: &sqlx::PgPool, username: &str) -> AppResult<i32> {
    let username = normalize_submit_as_username(Some(username))
        .map_err(|message| AppError::BadRequest(format!("Submit As: {message}")))?;
    let user_id: i32 = sqlx::query_scalar(FIND_OR_CREATE_USER_SQL)
        .bind(username)
        .fetch_one(pool)
        .await?;
    Ok(user_id)
}

pub(crate) fn build_media_rom_extensions_json(all_media_types: &[EditMediaTypeRow]) -> String {
    let mut map = serde_json::Map::new();
    for m in all_media_types {
        map.insert(m.code.clone(), serde_json::json!(m.rom_extension));
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
}

pub(crate) fn build_media_is_cd_json(all_media_types: &[EditMediaTypeRow]) -> String {
    let mut map = serde_json::Map::new();
    for m in all_media_types {
        map.insert(
            m.code.clone(),
            serde_json::json!(is_cd_rom_extension(&m.rom_extension)),
        );
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
}

pub(crate) fn build_media_layers_json(all_media_types: &[EditMediaTypeRow]) -> String {
    let mut media_layers_map = serde_json::Map::new();
    for m in all_media_types {
        media_layers_map.insert(m.code.clone(), serde_json::json!(m.layer_count));
    }
    serde_json::to_string(&media_layers_map).unwrap_or_else(|_| "{}".into())
}

pub(crate) fn build_media_has_pic_json(all_media_types: &[EditMediaTypeRow]) -> String {
    let mut map = serde_json::Map::new();
    for m in all_media_types {
        map.insert(m.code.clone(), serde_json::json!(m.pic));
    }
    serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
}

fn media_shows_error_count(all_media_types: &[EditMediaTypeRow], media_code: &str) -> bool {
    all_media_types
        .iter()
        .find(|m| m.code == media_code)
        .map_or(false, |m| is_cd_rom_extension(&m.rom_extension))
}

pub(crate) fn build_system_options(all_systems: &[System], selected: &str) -> Vec<SystemOption> {
    let mut systems: Vec<SystemOption> = all_systems
        .iter()
        .map(|s| SystemOption {
            code: s.code.clone(),
            name: s.system_name(),
            selected: s.code == selected,
        })
        .collect();
    systems.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    systems
}

pub(crate) fn build_media_options(
    all_media_types: &[EditMediaTypeRow],
    selected: &str,
) -> Vec<MediaTypeOption> {
    all_media_types
        .iter()
        .map(|m| MediaTypeOption {
            code: m.code.clone(),
            name: m.name.clone(),
            selected: m.code == selected,
        })
        .collect()
}

pub(crate) fn build_category_options(
    all_categories: &[CategoryRow],
    selected: &str,
) -> Vec<SelectOption> {
    all_categories
        .iter()
        .map(|c| SelectOption {
            value: c.name.clone(),
            name: c.name.clone(),
            selected: selected == c.name,
        })
        .collect()
}

pub(crate) fn build_check_options(all: &[Region], selected_codes: &[String]) -> Vec<CheckOption> {
    const COMMON_REGIONS: &[&str] = &[
        "Asia",
        "Australia",
        "Europe",
        "France",
        "Germany",
        "Italy",
        "Japan",
        "Korea",
        "Netherlands",
        "Poland",
        "Portugal",
        "Russia",
        "Spain",
        "UK",
        "USA",
    ];
    let mut options: Vec<CheckOption> = all
        .iter()
        .map(|r| CheckOption {
            value: r.code.trim().to_string(),
            name: r.name.clone(),
            code: r.flag_code.trim().to_lowercase(),
            selected: selected_codes.iter().any(|c| c.trim() == r.code.trim()),
            highlight: String::new(),
            common: COMMON_REGIONS.iter().any(|name| *name == r.name),
        })
        .collect();
    options.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    options
}

pub(crate) fn build_lang_check_options(
    all: &[Language],
    selected_codes: &[String],
) -> Vec<CheckOption> {
    const COMMON_LANGUAGES: &[&str] = &[
        "Danish",
        "Dutch",
        "English",
        "Finnish",
        "French",
        "German",
        "Italian",
        "Japanese",
        "Korean",
        "Norwegian",
        "Polish",
        "Portuguese",
        "Russian",
        "Spanish",
        "Swedish",
    ];
    let mut options: Vec<CheckOption> = all
        .iter()
        .map(|l| CheckOption {
            value: l.code.trim().to_string(),
            name: l.name.clone(),
            code: l.flag_code.trim().to_lowercase(),
            selected: selected_codes.iter().any(|c| c.trim() == l.code.trim()),
            highlight: String::new(),
            common: COMMON_LANGUAGES.iter().any(|name| *name == l.name),
        })
        .collect();
    options.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    options
}

pub(crate) fn max_layers_for_media(all_media_types: &[EditMediaTypeRow], code: &str) -> u32 {
    all_media_types
        .iter()
        .find(|m| m.code == code)
        .map(|m| m.layer_count as u32)
        .unwrap_or(1)
}

pub(crate) fn ring_layers(media_layers: u32) -> u32 {
    media_layers + 1
}

async fn edit_page(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    let ref_data = fetch_ref_data(&state.pool).await?;

    let disc_region_codes: Vec<String> =
        sqlx::query_scalar("SELECT region_code FROM disc_regions WHERE disc_id = $1")
            .bind(id)
            .fetch_all(&state.pool)
            .await?;
    let disc_lang_codes: Vec<String> =
        sqlx::query_scalar("SELECT language_code FROM disc_languages WHERE disc_id = $1")
            .bind(id)
            .fetch_all(&state.pool)
            .await?;

    let (systems_media_json, systems_has_flags_json) = build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_is_cd_json = build_media_is_cd_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
    let edition_suggestions_json = build_edition_suggestions_json(&state).await?;
    let max_layers = detail.disc.media_type.max_layers();
    let ring_layer_count = ring_layers(max_layers);

    let mut sorted_ring_entries = detail.ring_entries.clone();
    disc_service::sort_ring_entry_views(&mut sorted_ring_entries, ring_layer_count as usize);
    let ring_data: Vec<serde_json::Value> = sorted_ring_entries
        .iter()
        .map(|e| {
            let layers: Vec<serde_json::Value> = (0..ring_layer_count)
                .map(|li| {
                    let layer = e.layers.iter().find(|l| l.layer == li as i32);
                    serde_json::json!({
                        "mastering_code": layer.and_then(|l| l.mastering_code.as_deref()).unwrap_or(""),
                        "mastering_sid": layer.and_then(|l| l.mastering_sid.as_deref()).unwrap_or(""),
                        "mould_sids": layer.map(|l| normalize_csv_field(&l.mould_sids)).unwrap_or_default(),
                        "toolstamps": layer.map(|l| normalize_csv_field(&l.toolstamps)).unwrap_or_default(),
                        "additional_moulds": layer.map(|l| normalize_csv_field(&l.additional_moulds)).unwrap_or_default(),
                    })
                })
                .collect();
            serde_json::json!({
                "id": e.id,
                "offset_value": e.offset_value.map(|v| v.to_string()).unwrap_or_default(),
                "offset_extra_value": e.offset_extra_value.map(|v| v.to_string()).unwrap_or_default(),
                "sample_start": e.sample_data_start.map(|v| v.to_string()).unwrap_or_default(),
                "comment": e.comment.clone().unwrap_or_default(),
                "layers": layers,
            })
        })
        .collect();
    let ring_codes_json = serde_json::to_string(&ring_data).unwrap_or_else(|_| "[]".into());

    let rom_extension = detail.disc.media_type.rom_extension();
    let total_tracks = detail
        .files
        .iter()
        .filter(|f| f.track_number.is_some())
        .count();
    let files_xml = detail
        .files
        .iter()
        .filter(|f| f.track_number.is_some())
        .map(|f| {
            let name =
                build_simple_track_name(f.track_number.as_deref(), total_tracks, rom_extension);
            format!(
                r#"<rom name="{}" size="{}" crc="{}" md5="{}" sha1="{}" />"#,
                name, f.size, f.crc32, f.md5, f.sha1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let sector_ranges_text = detail
        .sector_ranges
        .iter()
        .map(|r| format!("{}-{}", r.range_start, r.range_end))
        .collect::<Vec<_>>()
        .join("\n");

    let page_title = format_display_title(
        &detail.disc.title,
        detail.disc.disc_number.as_deref(),
        detail.disc.disc_title.as_deref(),
        detail.disc.filename_suffix.as_deref(),
    );

    Ok(Html(
        DiscEditTemplate {
            current_user: Some(user.username.clone()),
            disc_id: id,
            page_title,

            systems: build_system_options(&ref_data.all_systems, &detail.disc.system_code),
            media_types_all: build_media_options(
                &ref_data.all_media_types,
                detail.disc.media_type.code(),
            ),
            categories: build_category_options(
                &ref_data.all_categories,
                &detail.disc.category.to_string(),
            ),
            regions: build_check_options(&ref_data.all_regions, &disc_region_codes),
            languages: build_lang_check_options(&ref_data.all_languages, &disc_lang_codes),

            system_code: detail.disc.system_code.clone(),
            media_type_code: detail.disc.media_type.code().to_string(),
            max_layers,
            media_layers_json,
            systems_media_json,
            systems_has_flags_json,
            edition_suggestions_json,
            submit_as_usernames_json: "[]".to_string(),
            media_rom_extensions_json,
            media_is_cd_json,

            title: detail.disc.title.clone(),
            show_title_foreign: detail.system.has_title_foreign,
            title_foreign: detail.disc.title_foreign.clone().unwrap_or_default(),
            show_disc_number: detail.system.has_disc_number,
            disc_number: detail.disc.disc_number.clone().unwrap_or_default(),
            show_disc_title: detail.system.has_disc_title,
            disc_title: detail.disc.disc_title.clone().unwrap_or_default(),
            filename_suffix: detail.disc.filename_suffix.clone().unwrap_or_default(),

            show_serial: detail.system.has_serial,
            serials: detail
                .disc
                .serial
                .iter()
                .cloned()
                .map(|s| HighlightedValue {
                    value: s,
                    highlight: String::new(),
                })
                .collect(),
            show_version: detail.system.has_version,
            version: detail.disc.version.clone().unwrap_or_default(),
            show_edition: detail.system.has_edition,
            editions: detail
                .disc
                .edition
                .iter()
                .cloned()
                .map(|s| HighlightedValue {
                    value: s,
                    highlight: String::new(),
                })
                .collect(),
            show_barcode: detail.system.has_barcode,
            barcodes: detail
                .disc
                .barcode
                .iter()
                .cloned()
                .map(|s| HighlightedValue {
                    value: s,
                    highlight: String::new(),
                })
                .collect(),
            ring_codes_json,
            ring_highlights_json: "[]".to_string(),

            comments: detail.disc.comments.clone().unwrap_or_default(),
            contents: detail.disc.contents.clone().unwrap_or_default(),

            show_error_count: detail.disc.media_type.is_cd(),
            error_count: detail
                .disc
                .error_count
                .map(|e| e.to_string())
                .unwrap_or_default(),
            show_exe_date: detail.system.has_exe_date,
            exe_date: detail.disc.exe_date.clone().unwrap_or_default(),
            show_edc: detail.system.has_edc,
            edc_value: bool_edc_value(detail.disc.edc),

            layerbreaks: detail
                .disc
                .layerbreaks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|v| v.to_string())
                .collect(),
            show_pvd: detail.system.has_pvd,
            pvd_hex: detail
                .disc
                .pvd
                .as_ref()
                .map(|data| format_pvd_hex_dump(data))
                .unwrap_or_default(),
            show_pic: detail.disc.media_type.has_pic(),
            media_has_pic_json: media_has_pic_json.clone(),
            pic_hex: detail
                .disc
                .pic
                .as_ref()
                .map(|data| format_header_hex_dump(data))
                .unwrap_or_default(),
            show_bca: detail.system.has_bca,
            bca_hex: detail
                .disc
                .bca
                .as_ref()
                .map(|data| format_header_hex_dump(data))
                .unwrap_or_default(),
            show_header: detail.system.has_header,
            header_hex: detail
                .disc
                .header
                .as_ref()
                .map(|data| format_header_hex_dump(data))
                .unwrap_or_default(),

            show_disc_id: detail.system.has_disc_id,
            show_key: detail.system.has_key,
            show_protection: detail.system.has_protection,
            protection: detail.disc.protection.clone().unwrap_or_default(),
            show_sector_ranges: detail.system.has_sector_ranges,
            sector_ranges_text,
            show_sbi: detail.system.has_sbi,
            sbi: detail.disc.sbi.clone().unwrap_or_default(),
            has_sample_start: detail.system.has_sample_start,
            protection_key_disc_key: detail
                .disc
                .disc_key
                .as_ref()
                .map(|bytes| {
                    bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                })
                .unwrap_or_default(),
            protection_key_disc_id: detail.disc.disc_id.clone().unwrap_or_default(),

            cue: detail
                .disc
                .cue
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|c| simplify_cue(c, rom_extension))
                .unwrap_or_default(),
            files_xml,

            status: detail.disc.status.to_string(),

            is_add_mode: false,
            dump_log: String::new(),
            dump_log_required: false,
            extra_upload_url: String::new(),
            show_submit_as: false,
            submit_as_username: String::new(),

            submit_button_text: if user.role.can_edit_directly() {
                "Save".into()
            } else {
                "Submit".into()
            },
            validation_errors: vec![],
            linked_validation_errors: vec![],
            validation_result: String::new(),
            validation_result_disc_id: 0,
            validation_result_disc_title: String::new(),

            is_review_mode: false,
            changed_fields: vec![],
            review_annotations: vec![],
            review_old_multiline: vec![],
            submission_id: 0,
            submission_type_display: String::new(),
            submitter_id: 0,
            submitter_name: String::new(),
            submission_comment: String::new(),
            dump_log_display: String::new(),
            extra_upload_url_display: String::new(),
            submission_status: String::new(),
            reviewer_id: 0,
            reviewer_name: String::new(),
            review_comment_display: String::new(),
            review_comment_input: String::new(),
            created_at_display: String::new(),
            reviewed_at_display: String::new(),
            changes_json: String::new(),
        }
        .render()
        .unwrap(),
    ))
}

#[derive(Deserialize)]
pub struct DiscEditForm {
    pub system_code: String,
    pub media_type: String,
    pub title: String,
    pub title_foreign: Option<String>,
    pub disc_number: Option<String>,
    pub disc_title: Option<String>,
    pub filename_suffix: Option<String>,
    pub category: String,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub regions: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub languages: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub serial: Vec<String>,
    pub version: Option<String>,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub edition: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub barcode: Vec<String>,
    pub ring_codes_json: Option<String>,
    pub comments: Option<String>,
    pub contents: Option<String>,
    pub error_count: Option<String>,
    pub exe_date: Option<String>,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub edc: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub layerbreak: Vec<String>,
    pub pvd: Option<String>,
    pub pic: Option<String>,
    pub bca: Option<String>,
    pub header: Option<String>,
    pub protection: Option<String>,
    pub sector_ranges: Option<String>,
    pub sbi: Option<String>,
    pub protection_key_disc_key: Option<String>,
    pub protection_key_disc_id: Option<String>,
    #[serde(rename = "cuesheet")]
    pub cue: Option<String>,
    #[serde(rename = "dat")]
    pub files_xml: Option<String>,
    #[serde(default)]
    pub status: String,
    pub submission_comment: Option<String>,
    pub submit_as: Option<String>,
    pub dump_log: Option<String>,
    pub extra_upload_url: Option<String>,
}

#[derive(Deserialize)]
pub struct DiscEditPostForm {
    #[serde(default)]
    pub action: String,
    #[serde(flatten)]
    pub disc: DiscEditForm,
}

#[derive(Default)]
pub(crate) struct ValidationResultMessage {
    pub text: String,
    pub disc_id: i32,
    pub disc_title: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LinkedValidationError {
    pub text: String,
    pub disc_id: i32,
    pub disc_title: String,
}

#[derive(sqlx::FromRow)]
struct ValidationResultDiscTitleRow {
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    has_disc_number: bool,
    has_disc_title: bool,
}

#[derive(sqlx::FromRow)]
struct DuplicateNameDiscRow {
    id: i32,
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    region_names: Vec<String>,
    language_codes: Vec<String>,
}

pub(crate) fn validate_form(
    form: &DiscEditForm,
    all_media_types: &[EditMediaTypeRow],
    all_systems: &[System],
) -> Vec<String> {
    let mut errors = Vec::new();

    if form.system_code.trim().is_empty() {
        errors.push("System: must be selected".into());
    }

    if form.title.trim().is_empty() {
        errors.push("Title: cannot be empty".into());
    }

    if form.regions.is_empty() {
        errors.push("Regions: at least one region must be selected".into());
    }

    if let Some(ref s) = form.error_count {
        let s = s.trim();
        if !s.is_empty() {
            if validation::validate_non_negative_int(s).is_err() {
                errors.push("Error Count: must be a non-negative integer".into());
            }
        }
    }

    for (i, lb) in form.layerbreak.iter().enumerate() {
        let s = lb.trim();
        if !s.is_empty() {
            if validation::validate_non_negative_int(s).is_err() {
                errors.push(format!(
                    "Layerbreak {}: must be a non-negative integer",
                    i + 1
                ));
            }
        }
    }

    if let Some(ref json_str) = form.ring_codes_json {
        let ring_errors = validation::validate_ring_code_offsets(json_str);
        errors.extend(ring_errors);
    }

    if let Some(ref text) = form.sector_ranges {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_sector_ranges(text) {
                errors.push(format!("Sector Ranges: {}", e));
            }
        }
    }

    if let Some(ref text) = form.sbi {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_sbi(text) {
                errors.push(format!("SBI: {}", e));
            }
        }
    }

    if let Some(ref text) = form.pvd {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_hex_dump(text) {
                errors.push(format!("PVD: {}", e));
            }
        }
    }

    if let Some(ref text) = form.header {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_hex_dump(text) {
                errors.push(format!("Header: {}", e));
            }
        }
    }

    if let Some(ref text) = form.bca {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_hex_dump(text) {
                errors.push(format!("BCA: {}", e));
            }
        }
    }

    if let Some(ref text) = form.pic {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_hex_dump(text) {
                errors.push(format!("PIC: {}", e));
            }
        }
    }

    if let Some(ref text) = form.protection_key_disc_key {
        let text = text.trim();
        if !text.is_empty() {
            if !text.chars().all(|c| c.is_ascii_hexdigit()) {
                errors.push("Disc Key: must contain only hexadecimal characters".into());
            } else if text.len() % 2 != 0 {
                errors.push("Disc Key: must have an even number of hexadecimal characters".into());
            }
        }
    }

    let system_has_edc = all_systems
        .iter()
        .find(|s| s.code == form.system_code)
        .map_or(false, |s| s.has_edc);
    if system_has_edc && form_edc_selection(form).is_none() {
        errors.push("EDC: select Yes or No".into());
    }

    let is_cd_media = all_media_types
        .iter()
        .find(|m| m.code == form.media_type)
        .map_or(false, |m| is_cd_rom_extension(&m.rom_extension));

    if is_cd_media {
        match form.cue.as_deref().map(|s| s.trim()) {
            None | Some("") => {
                errors.push("Cuesheet: required for this media type".into());
            }
            Some(text) => {
                if let Err(e) = validation::validate_cuesheet(text) {
                    errors.push(format!("Cuesheet: {}", e));
                }
            }
        }
    } else if let Some(ref text) = form.cue {
        if !text.trim().is_empty() {
            errors.push("Cuesheet: not applicable for this media type".into());
        }
    }

    match form.files_xml.as_deref().map(|s| s.trim()) {
        None | Some("") => {
            errors.push("Dat: must contain at least one entry".into());
        }
        Some(text) => {
            if !text.lines().any(|l| l.trim().starts_with("<rom ")) {
                errors.push("Dat: must contain at least one entry".into());
            } else if let Err(e) = validation::validate_dat(text) {
                errors.push(format!("Dat: {}", e));
            }
        }
    }

    errors
}

async fn render_form_with_errors(
    state: &AppState,
    id: i32,
    username: &str,
    form: &DiscEditForm,
    errors: Vec<String>,
    linked_validation_errors: Vec<LinkedValidationError>,
    validation_result: ValidationResultMessage,
    is_add_mode: bool,
    can_edit_directly: bool,
    can_moderate: bool,
) -> AppResult<Response> {
    let pool = &state.pool;
    let ref_data = fetch_ref_data(pool).await?;
    let system = disc_service::get_system(pool, &form.system_code).await.ok();

    let (systems_media_json, systems_has_flags_json) = build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_is_cd_json = build_media_is_cd_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
    let edition_suggestions_json = build_edition_suggestions_json(&state).await?;
    let submit_as_usernames_json = if is_add_mode && can_moderate {
        build_submit_as_usernames_json(pool).await?
    } else {
        "[]".to_string()
    };
    let max_layers = max_layers_for_media(&ref_data.all_media_types, &form.media_type);

    let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

    let media_pic = ref_data
        .all_media_types
        .iter()
        .find(|m| m.code == form.media_type)
        .map_or(false, |m| m.pic);
    let show_error_count = media_shows_error_count(&ref_data.all_media_types, &form.media_type);

    let page_title = format_display_title(
        &form.title,
        form.disc_number.as_deref(),
        form.disc_title.as_deref(),
        form.filename_suffix.as_deref(),
    );

    let template = DiscEditTemplate {
        current_user: Some(username.to_string()),
        disc_id: id,
        page_title,

        systems: build_system_options(&ref_data.all_systems, &form.system_code),
        media_types_all: build_media_options(&ref_data.all_media_types, &form.media_type),
        categories: build_category_options(&ref_data.all_categories, &form.category),
        regions: build_check_options(&ref_data.all_regions, &form.regions),
        languages: build_lang_check_options(&ref_data.all_languages, &form.languages),

        system_code: form.system_code.clone(),
        media_type_code: form.media_type.clone(),
        max_layers,
        media_layers_json,
        systems_media_json,
        systems_has_flags_json,
        edition_suggestions_json,
        submit_as_usernames_json,
        media_rom_extensions_json,
        media_is_cd_json,

        title: form.title.clone(),
        show_title_foreign: has_sys(|s| s.has_title_foreign),
        title_foreign: form.title_foreign.clone().unwrap_or_default(),
        show_disc_number: has_sys(|s| s.has_disc_number),
        disc_number: form.disc_number.clone().unwrap_or_default(),
        show_disc_title: has_sys(|s| s.has_disc_title),
        disc_title: form.disc_title.clone().unwrap_or_default(),
        filename_suffix: form.filename_suffix.clone().unwrap_or_default(),

        show_serial: has_sys(|s| s.has_serial),
        serials: form
            .serial
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| HighlightedValue {
                value: s,
                highlight: String::new(),
            })
            .collect(),
        show_version: has_sys(|s| s.has_version),
        version: form.version.clone().unwrap_or_default(),
        show_edition: has_sys(|s| s.has_edition),
        editions: form
            .edition
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| HighlightedValue {
                value: s,
                highlight: String::new(),
            })
            .collect(),
        show_barcode: has_sys(|s| s.has_barcode),
        barcodes: form
            .barcode
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| HighlightedValue {
                value: s,
                highlight: String::new(),
            })
            .collect(),
        ring_codes_json: form.ring_codes_json.clone().unwrap_or_else(|| "[]".into()),
        ring_highlights_json: "[]".to_string(),

        comments: form.comments.clone().unwrap_or_default(),
        contents: form.contents.clone().unwrap_or_default(),

        show_error_count,
        error_count: form.error_count.clone().unwrap_or_default(),
        show_exe_date: has_sys(|s| s.has_exe_date),
        exe_date: form.exe_date.clone().unwrap_or_default(),
        show_edc: has_sys(|s| s.has_edc),
        edc_value: form_edc_value(form),

        layerbreaks: form.layerbreak.clone(),
        show_pvd: has_sys(|s| s.has_pvd),
        pvd_hex: form.pvd.clone().unwrap_or_default(),
        show_pic: media_pic,
        media_has_pic_json,
        pic_hex: form.pic.clone().unwrap_or_default(),
        show_bca: has_sys(|s| s.has_bca),
        bca_hex: form.bca.clone().unwrap_or_default(),
        show_header: has_sys(|s| s.has_header),
        header_hex: form.header.clone().unwrap_or_default(),

        show_disc_id: has_sys(|s| s.has_disc_id),
        show_key: has_sys(|s| s.has_key),
        show_protection: has_sys(|s| s.has_protection),
        protection: form.protection.clone().unwrap_or_default(),
        show_sector_ranges: has_sys(|s| s.has_sector_ranges),
        sector_ranges_text: form.sector_ranges.clone().unwrap_or_default(),
        show_sbi: has_sys(|s| s.has_sbi),
        sbi: form.sbi.clone().unwrap_or_default(),
        has_sample_start: has_sys(|s| s.has_sample_start),
        protection_key_disc_key: form.protection_key_disc_key.clone().unwrap_or_default(),
        protection_key_disc_id: form.protection_key_disc_id.clone().unwrap_or_default(),

        cue: form.cue.clone().unwrap_or_default(),
        files_xml: form.files_xml.clone().unwrap_or_default(),

        status: normalized_disc_status(&form.status),

        is_add_mode,
        dump_log: form.dump_log.clone().unwrap_or_default(),
        dump_log_required: is_add_mode && !can_moderate,
        extra_upload_url: form.extra_upload_url.clone().unwrap_or_default(),
        show_submit_as: is_add_mode && can_moderate,
        submit_as_username: submit_as_username_for_form(username, form, can_moderate),

        submit_button_text: if can_edit_directly {
            "Save".into()
        } else {
            "Submit".into()
        },
        validation_errors: errors,
        linked_validation_errors,
        validation_result: validation_result.text,
        validation_result_disc_id: validation_result.disc_id,
        validation_result_disc_title: validation_result.disc_title,

        is_review_mode: false,
        changed_fields: vec![],
        review_annotations: vec![],
        review_old_multiline: vec![],
        submission_id: 0,
        submission_type_display: String::new(),
        submitter_id: 0,
        submitter_name: String::new(),
        submission_comment: form.submission_comment.clone().unwrap_or_default(),
        dump_log_display: String::new(),
        extra_upload_url_display: String::new(),
        submission_status: String::new(),
        reviewer_id: 0,
        reviewer_name: String::new(),
        review_comment_display: String::new(),
        review_comment_input: String::new(),
        created_at_display: String::new(),
        reviewed_at_display: String::new(),
        changes_json: String::new(),
    };

    let status =
        if template.validation_errors.is_empty() && template.linked_validation_errors.is_empty() {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        };
    let html = template.render().unwrap();
    Ok((status, Html(html)).into_response())
}

fn valid_dat_for_matching(form: &DiscEditForm) -> Option<&str> {
    let text = form.files_xml.as_deref()?.trim();
    if text.is_empty() || !text.lines().any(|l| l.trim().starts_with("<rom ")) {
        return None;
    }
    validation::validate_dat(text).ok()?;
    Some(text)
}

async fn build_add_validation_result_for_target(
    pool: &sqlx::PgPool,
    target_disc_id: Option<i32>,
    has_valid_dat: bool,
) -> AppResult<ValidationResultMessage> {
    if !has_valid_dat {
        return Ok(ValidationResultMessage::default());
    }
    let Some(disc_id) = target_disc_id else {
        return Ok(add_validation_result_for_match(None));
    };

    let disc_title = fetch_validation_result_disc_title(pool, disc_id)
        .await?
        .unwrap_or_else(|| format!("disc #{disc_id}"));
    Ok(add_validation_result_for_match(Some((disc_id, disc_title))))
}

async fn fetch_validation_result_disc_title(
    pool: &sqlx::PgPool,
    disc_id: i32,
) -> AppResult<Option<String>> {
    let Some(row) = sqlx::query_as::<_, ValidationResultDiscTitleRow>(
        "SELECT d.title, d.disc_number, d.disc_title, d.filename_suffix,
                s.has_disc_number, s.has_disc_title
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         WHERE d.id = $1",
    )
    .bind(disc_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    Ok(Some(format_display_title(
        &row.title,
        if row.has_disc_number {
            row.disc_number.as_deref()
        } else {
            None
        },
        if row.has_disc_title {
            row.disc_title.as_deref()
        } else {
            None
        },
        row.filename_suffix.as_deref(),
    )))
}

fn add_validation_result_for_match(target_disc: Option<(i32, String)>) -> ValidationResultMessage {
    match target_disc {
        Some((disc_id, disc_title)) => ValidationResultMessage {
            text: "This is a verification submission for".to_string(),
            disc_id,
            disc_title,
        },
        None => ValidationResultMessage {
            text: "This is a new disc submission.".to_string(),
            disc_id: 0,
            disc_title: String::new(),
        },
    }
}

pub(crate) fn norm_opt_str(s: Option<&str>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn norm_opt_multiline_str(s: Option<&str>) -> Option<String> {
    s.map(normalize_newlines)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(crate) fn norm_str_vec(v: Vec<String>) -> Vec<String> {
    v.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn norm_str_vec_keep_order(v: Vec<String>) -> Vec<String> {
    v.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn norm_str_vec_keep_order_with_internal_blanks(v: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = v.into_iter().map(|s| s.trim().to_string()).collect();
    while out.last().map(|s| s.is_empty()).unwrap_or(false) {
        out.pop();
    }
    out
}

fn form_edc_selection(form: &DiscEditForm) -> Option<bool> {
    form.edc.iter().find_map(|v| match v.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

fn form_edc_value(form: &DiscEditForm) -> String {
    match form_edc_selection(form) {
        Some(true) => "true".to_string(),
        Some(false) => "false".to_string(),
        None => String::new(),
    }
}

fn bool_edc_value(value: bool) -> String {
    if value {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

fn form_edc_bool(form: &DiscEditForm) -> bool {
    form_edc_selection(form).unwrap_or(false)
}

fn normalized_disc_status(raw: &str) -> String {
    match raw {
        "Disabled" => "Disabled".to_string(),
        "Questionable" => "Questionable".to_string(),
        "Verified" => "Verified".to_string(),
        _ => "Unverified".to_string(),
    }
}

/// Normalize multiline text for comparison: strip \r, trim each line's trailing whitespace,
/// trim leading/trailing blank lines.
fn normalize_multiline(s: Option<&str>) -> Option<String> {
    s.map(normalize_newlines)
        .map(|text| {
            text.lines()
                .map(|line| line.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty())
}

fn build_ring_codes_json_from_detail(detail: &DiscDetail) -> serde_json::Value {
    let ring_layer_count = ring_layers(detail.disc.media_type.max_layers());
    let mut sorted_ring_entries = detail.ring_entries.clone();
    disc_service::sort_ring_entry_views(&mut sorted_ring_entries, ring_layer_count as usize);
    let entries: Vec<serde_json::Value> = sorted_ring_entries
        .iter()
        .map(|e| {
            let layers: Vec<serde_json::Value> = (0..ring_layer_count)
                .map(|li| {
                    let layer = e.layers.iter().find(|l| l.layer == li as i32);
                    serde_json::json!({
                        "mastering_code": layer.and_then(|l| l.mastering_code.as_deref()).unwrap_or(""),
                        "mastering_sid": layer.and_then(|l| l.mastering_sid.as_deref()).unwrap_or(""),
                        "mould_sids": layer.map(|l| normalize_csv_field(&l.mould_sids)).unwrap_or_default(),
                        "toolstamps": layer.map(|l| normalize_csv_field(&l.toolstamps)).unwrap_or_default(),
                        "additional_moulds": layer.map(|l| normalize_csv_field(&l.additional_moulds)).unwrap_or_default(),
                    })
                })
                .collect();
            serde_json::json!({
                "id": e.id,
                "offset_value": e.offset_value.map(|v| v.to_string()).unwrap_or_default(),
                "offset_extra_value": e.offset_extra_value.map(|v| v.to_string()).unwrap_or_default(),
                "sample_start": e.sample_data_start.map(|v| v.to_string()).unwrap_or_default(),
                "comment": e.comment.clone().unwrap_or_default(),
                "layers": layers,
            })
        })
        .collect();
    serde_json::json!(entries)
}

async fn edit_submit(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
    Form(post): Form<DiscEditPostForm>,
) -> AppResult<Response> {
    let form = post.disc;
    let ref_data = fetch_ref_data(&state.pool).await?;
    let errors = validate_form(&form, &ref_data.all_media_types, &ref_data.all_systems);
    let linked_validation_errors =
        validate_generated_name_unique(&state.pool, &form, Some(id), form_status_is_active(&form))
            .await?;
    if !errors.is_empty() || !linked_validation_errors.is_empty() {
        return render_form_with_errors(
            &state,
            id,
            &user.username,
            &form,
            errors,
            linked_validation_errors,
            ValidationResultMessage::default(),
            false,
            user.role.can_edit_directly(),
            false,
        )
        .await;
    }

    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    let changes = build_sparse_edit_changes(&form, &detail, &ref_data.all_media_types);

    let submission_comment = form
        .submission_comment
        .as_deref()
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let sub = queue_service::create_submission(
        &state.pool,
        SubmissionType::Edit,
        user.id,
        Some(id),
        changes,
        submission_comment.as_deref(),
        None,
        None,
    )
    .await?;

    if user.role.can_edit_directly() {
        let disc_id = queue_service::approve_submission(
            &state.pool,
            &sub,
            &sub.changes,
            user.id,
            None,
            &state.archive_tx,
        )
        .await?
        .ok_or(AppError::Internal(
            "submission was already processed".into(),
        ))?;
        Ok(Redirect::to(&format!("/disc/{disc_id}/")).into_response())
    } else {
        Ok(Redirect::to(&user_queue_url(&user.username)).into_response())
    }
}

async fn add_page(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
) -> AppResult<Html<String>> {
    let ref_data = fetch_ref_data(&state.pool).await?;

    let (systems_media_json, systems_has_flags_json) = build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_is_cd_json = build_media_is_cd_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
    let edition_suggestions_json = build_edition_suggestions_json(&state).await?;
    let can_submit_as = user.role.can_moderate();
    let submit_as_usernames_json = if can_submit_as {
        build_submit_as_usernames_json(&state.pool).await?
    } else {
        "[]".to_string()
    };
    let username = user.username.clone();

    let default_system = ref_data.all_systems.iter().find(|s| s.code == "PC");
    let has_sys = |f: fn(&System) -> bool| default_system.map_or(true, f);
    let show_error_count = false;

    Ok(Html(
        DiscEditTemplate {
            current_user: Some(username.clone()),
            disc_id: 0,
            page_title: String::new(),

            systems: build_system_options(&ref_data.all_systems, "PC"),
            media_types_all: build_media_options(&ref_data.all_media_types, ""),
            categories: build_category_options(&ref_data.all_categories, "Games"),
            regions: build_check_options(&ref_data.all_regions, &[]),
            languages: build_lang_check_options(&ref_data.all_languages, &[]),

            system_code: "PC".to_string(),
            media_type_code: String::new(),
            max_layers: 1,
            media_layers_json,
            systems_media_json,
            systems_has_flags_json,
            edition_suggestions_json,
            submit_as_usernames_json,
            media_rom_extensions_json,
            media_is_cd_json,

            title: String::new(),
            show_title_foreign: has_sys(|s| s.has_title_foreign),
            title_foreign: String::new(),
            show_disc_number: has_sys(|s| s.has_disc_number),
            disc_number: String::new(),
            show_disc_title: has_sys(|s| s.has_disc_title),
            disc_title: String::new(),
            filename_suffix: String::new(),

            show_serial: has_sys(|s| s.has_serial),
            serials: vec![],
            show_version: has_sys(|s| s.has_version),
            version: String::new(),
            show_edition: has_sys(|s| s.has_edition),
            editions: vec![],
            show_barcode: has_sys(|s| s.has_barcode),
            barcodes: vec![],
            ring_codes_json: "[]".to_string(),
            ring_highlights_json: "[]".to_string(),

            comments: String::new(),
            contents: String::new(),

            show_error_count,
            error_count: String::new(),
            show_exe_date: has_sys(|s| s.has_exe_date),
            exe_date: String::new(),
            show_edc: has_sys(|s| s.has_edc),
            edc_value: String::new(),

            layerbreaks: vec![],
            show_pvd: has_sys(|s| s.has_pvd),
            pvd_hex: String::new(),
            show_pic: false,
            media_has_pic_json,
            pic_hex: String::new(),
            show_bca: has_sys(|s| s.has_bca),
            bca_hex: String::new(),
            show_header: has_sys(|s| s.has_header),
            header_hex: String::new(),

            show_disc_id: has_sys(|s| s.has_disc_id),
            show_key: has_sys(|s| s.has_key),
            show_protection: has_sys(|s| s.has_protection),
            protection: String::new(),
            show_sector_ranges: has_sys(|s| s.has_sector_ranges),
            sector_ranges_text: String::new(),
            show_sbi: has_sys(|s| s.has_sbi),
            sbi: String::new(),
            protection_key_disc_key: String::new(),
            protection_key_disc_id: String::new(),
            has_sample_start: has_sys(|s| s.has_sample_start),

            cue: String::new(),
            files_xml: String::new(),

            status: "Unverified".to_string(),

            is_add_mode: true,
            dump_log: String::new(),
            dump_log_required: !user.role.can_moderate(),
            extra_upload_url: String::new(),
            show_submit_as: can_submit_as,
            submit_as_username: if can_submit_as {
                username
            } else {
                String::new()
            },

            submit_button_text: if user.role.can_edit_directly() {
                "Save".into()
            } else {
                "Submit".into()
            },
            validation_errors: vec![],
            linked_validation_errors: vec![],
            validation_result: String::new(),
            validation_result_disc_id: 0,
            validation_result_disc_title: String::new(),

            is_review_mode: false,
            changed_fields: vec![],
            review_annotations: vec![],
            review_old_multiline: vec![],
            submission_id: 0,
            submission_type_display: String::new(),
            submitter_id: 0,
            submitter_name: String::new(),
            submission_comment: String::new(),
            dump_log_display: String::new(),
            extra_upload_url_display: String::new(),
            submission_status: String::new(),
            reviewer_id: 0,
            reviewer_name: String::new(),
            review_comment_display: String::new(),
            review_comment_input: String::new(),
            created_at_display: String::new(),
            reviewed_at_display: String::new(),
            changes_json: String::new(),
        }
        .render()
        .unwrap(),
    ))
}

async fn add_submit(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Form(post): Form<DiscEditPostForm>,
) -> AppResult<Response> {
    let form = post.disc;
    let ref_data = fetch_ref_data(&state.pool).await?;
    let mut errors = validate_form(&form, &ref_data.all_media_types, &ref_data.all_systems);
    let can_submit_as = user.role.can_moderate();
    errors.extend(validate_submit_as_for_add(&form, can_submit_as));
    errors.extend(validate_add_submission_logs(
        &form,
        user.role.can_moderate(),
    ));

    let target_disc_id = match valid_dat_for_matching(&form) {
        Some(files_xml) => queue_service::find_matching_disc(&state.pool, files_xml).await,
        None => None,
    };
    let linked_validation_errors =
        validate_generated_name_unique(&state.pool, &form, target_disc_id, true).await?;

    let dump_log_text = trimmed_nonempty(form.dump_log.as_deref());
    let extra_upload_url_text = trimmed_nonempty(form.extra_upload_url.as_deref());

    let validation_result = if post.action == "validate"
        || !errors.is_empty()
        || !linked_validation_errors.is_empty()
    {
        build_add_validation_result_for_target(
            &state.pool,
            target_disc_id,
            valid_dat_for_matching(&form).is_some(),
        )
        .await?
    } else {
        ValidationResultMessage::default()
    };

    if post.action == "validate" || !errors.is_empty() || !linked_validation_errors.is_empty() {
        return render_form_with_errors(
            &state,
            0,
            &user.username,
            &form,
            errors,
            linked_validation_errors,
            validation_result,
            true,
            user.role.can_edit_directly(),
            user.role.can_moderate(),
        )
        .await;
    }

    let changes = build_new_disc_changes(&form, &ref_data.all_media_types);
    let submitter_id = if can_submit_as {
        let submit_as_username = normalize_submit_as_username(form.submit_as.as_deref())
            .map_err(|message| AppError::BadRequest(format!("Submit As: {message}")))?;
        find_or_create_submit_as_user(&state.pool, &submit_as_username).await?
    } else {
        user.id
    };

    let submission_comment = form
        .submission_comment
        .as_deref()
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let sub = queue_service::create_submission(
        &state.pool,
        SubmissionType::Disc,
        submitter_id,
        target_disc_id,
        changes,
        submission_comment.as_deref(),
        dump_log_text,
        extra_upload_url_text,
    )
    .await?;

    if user.role.can_edit_directly() {
        let disc_id = queue_service::approve_submission(
            &state.pool,
            &sub,
            &sub.changes,
            user.id,
            None,
            &state.archive_tx,
        )
        .await?
        .ok_or(AppError::Internal(
            "submission was already processed".into(),
        ))?;
        Ok(Redirect::to(&format!("/disc/{disc_id}/")).into_response())
    } else {
        Ok(Redirect::to(&user_queue_url(&user.username)).into_response())
    }
}

pub(crate) fn build_flat_changes(
    form: &DiscEditForm,
    all_media_types: &[EditMediaTypeRow],
) -> serde_json::Value {
    let new_edc = form_edc_bool(form);
    let new_error_count: serde_json::Value = form
        .error_count
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| s.trim().parse::<i32>().ok())
        .map(|v| serde_json::json!(v))
        .unwrap_or(serde_json::Value::Null);
    let new_layerbreaks: Vec<i32> = form
        .layerbreak
        .iter()
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                s.parse::<i32>().ok()
            }
        })
        .collect();
    let new_disc_key = norm_opt_str(form.protection_key_disc_key.as_deref());
    let new_disc_id = norm_opt_str(form.protection_key_disc_id.as_deref());
    let new_ring_codes = form
        .ring_codes_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .unwrap_or(serde_json::json!([]));
    let new_sector_ranges: Vec<serde_json::Value> =
        validation::parse_sector_range_pairs(form.sector_ranges.as_deref().unwrap_or(""))
            .into_iter()
            .map(|(start, end)| serde_json::json!({"start": start, "end": end}))
            .collect();
    let new_regions = norm_str_vec(form.regions.clone());
    let new_languages = norm_str_vec(form.languages.clone());
    let new_serials = norm_str_vec_keep_order(form.serial.clone());
    let new_editions = norm_str_vec_keep_order(form.edition.clone());
    let new_barcodes = norm_str_vec_keep_order(form.barcode.clone());

    let rom_ext = all_media_types
        .iter()
        .find(|m| m.code == form.media_type)
        .map(|m| m.rom_extension.as_str())
        .unwrap_or("");
    let new_cue = norm_opt_multiline_str(form.cue.as_deref()).map(|c| simplify_cue(&c, rom_ext));

    serde_json::json!({
        "system_code": form.system_code.trim(),
        "media_type": form.media_type.trim(),
        "title": form.title.trim(),
        "category": form.category.trim(),
        "title_foreign": norm_opt_str(form.title_foreign.as_deref()),
        "disc_number": norm_opt_str(form.disc_number.as_deref()),
        "disc_title": norm_opt_str(form.disc_title.as_deref()),
        "filename_suffix": norm_opt_str(form.filename_suffix.as_deref()),
        "serial": new_serials,
        "version": norm_opt_str(form.version.as_deref()),
        "edition": new_editions,
        "barcode": new_barcodes,
        "comments": norm_opt_multiline_str(form.comments.as_deref()),
        "contents": norm_opt_multiline_str(form.contents.as_deref()),
        "error_count": new_error_count,
        "exe_date": norm_opt_multiline_str(form.exe_date.as_deref()),
        "edc": new_edc,
        "layerbreaks": new_layerbreaks,
        "pvd": norm_opt_multiline_str(form.pvd.as_deref()).map(|s| normalize_pvd_hex_dump(&s)),
        "pic": norm_opt_multiline_str(form.pic.as_deref()),
        "bca": norm_opt_multiline_str(form.bca.as_deref()),
        "header": norm_opt_multiline_str(form.header.as_deref()),
        "protection": norm_opt_multiline_str(form.protection.as_deref()),
        "sbi": norm_opt_multiline_str(form.sbi.as_deref()),
        "disc_id": new_disc_id,
        "disc_key": new_disc_key,
        "cuesheet": new_cue,
        "dat": norm_opt_multiline_str(form.files_xml.as_deref()).map(|s| simplify_files_xml(&s, rom_ext)),
        "regions": new_regions,
        "languages": new_languages,
        "ring_codes": new_ring_codes,
        "sector_ranges": new_sector_ranges,
        "status": normalized_disc_status(&form.status),
    })
}

fn is_empty_json(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.trim().is_empty(),
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => false,
    }
}

fn scalar_operation_change(
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> Option<serde_json::Value> {
    if old == new {
        return None;
    }
    if is_empty_json(old) && is_empty_json(new) {
        return None;
    }
    if is_empty_json(old) {
        return Some(serde_json::json!({ "add": { "new": new } }));
    }
    if is_empty_json(new) {
        return Some(serde_json::json!({ "remove": { "old": old } }));
    }
    Some(serde_json::json!({ "modify": { "old": old, "new": new } }))
}

fn json_str_vec(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn set_operation_change(
    old_values: &[String],
    new_values: &[String],
    allow_removal: bool,
) -> Option<serde_json::Value> {
    let mut out = serde_json::Map::new();
    let add_values: Vec<String> = new_values
        .iter()
        .filter(|value| !old_values.iter().any(|old| old == *value))
        .cloned()
        .collect();
    let remove_values: Vec<String> = if allow_removal {
        old_values
            .iter()
            .filter(|value| !new_values.iter().any(|new| new == *value))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    if !add_values.is_empty() {
        out.insert("add".to_string(), serde_json::json!(add_values));
    }
    if !remove_values.is_empty() {
        out.insert("remove".to_string(), serde_json::json!(remove_values));
    }
    if out.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(out))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DatTrackDigest {
    size: i64,
    crc: String,
    md5: String,
    sha1: String,
}

fn dat_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn dat_track_map(dat: &str) -> Option<std::collections::BTreeMap<String, DatTrackDigest>> {
    let mut out = std::collections::BTreeMap::new();
    for raw_line in dat.lines() {
        let line = raw_line.trim();
        if !line.starts_with("<rom ") {
            continue;
        }
        let name = dat_attr(line, "name")?;
        let track = extract_track_from_filename(&name)?;
        let size = dat_attr(line, "size")?.parse::<i64>().ok()?;
        let crc = dat_attr(line, "crc")?;
        let md5 = dat_attr(line, "md5")?;
        let sha1 = dat_attr(line, "sha1")?;
        if out.contains_key(&track) {
            return None;
        }
        out.insert(
            track,
            DatTrackDigest {
                size,
                crc,
                md5,
                sha1,
            },
        );
    }
    Some(out)
}

fn dat_tracks_differ(old: &str, new: &str) -> bool {
    match (dat_track_map(old), dat_track_map(new)) {
        (Some(a), Some(b)) => a != b,
        _ => normalize_multiline(Some(old)) != normalize_multiline(Some(new)),
    }
}

fn parse_csv_items(value: &str) -> Vec<String> {
    let mut items: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    items.sort_by_key(|s| s.to_lowercase());
    items.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    items
}

fn normalize_csv_field(value: &str) -> String {
    parse_csv_items(value).join(", ")
}

fn ring_layer_change(
    old_layer: Option<&serde_json::Value>,
    new_layer: Option<&serde_json::Value>,
    layer_index: usize,
    _allow_removal: bool,
) -> Option<serde_json::Value> {
    let old_default = serde_json::json!({
        "mastering_code": "",
        "mastering_sid": "",
        "toolstamps": "",
        "mould_sids": "",
        "additional_moulds": ""
    });
    let new_default = old_default.clone();
    let old_layer = old_layer.unwrap_or(&old_default);
    let new_layer = new_layer.unwrap_or(&new_default);

    let mut out = serde_json::Map::new();
    out.insert("index".to_string(), serde_json::json!(layer_index));

    for field in ["mastering_code", "mastering_sid"] {
        if let Some(change) = scalar_operation_change(&old_layer[field], &new_layer[field]) {
            out.insert(field.to_string(), change);
        }
    }

    for field in ["toolstamps", "mould_sids", "additional_moulds"] {
        let old_csv = normalize_csv_field(old_layer[field].as_str().unwrap_or(""));
        let new_csv = normalize_csv_field(new_layer[field].as_str().unwrap_or(""));
        if let Some(change) =
            scalar_operation_change(&serde_json::json!(old_csv), &serde_json::json!(new_csv))
        {
            out.insert(field.to_string(), change);
        }
    }

    if out.len() > 1 {
        Some(serde_json::Value::Object(out))
    } else {
        None
    }
}

fn ring_entry_change(
    old_entry: Option<&serde_json::Value>,
    new_entry: Option<&serde_json::Value>,
    allow_removal: bool,
    entry_id: Option<i32>,
) -> Option<serde_json::Value> {
    let is_removal = old_entry.is_some() && new_entry.is_none();
    let old_default = serde_json::json!({
        "offset_value": "",
        "offset_extra_value": "",
        "sample_start": "",
        "comment": "",
        "layers": []
    });
    let new_default = old_default.clone();
    let old_entry = old_entry.unwrap_or(&old_default);
    let new_entry = new_entry.unwrap_or(&new_default);

    let mut out = serde_json::Map::new();
    if let Some(id) = entry_id {
        out.insert("id".to_string(), serde_json::json!(id));
    }
    if is_removal {
        out.insert("remove".to_string(), serde_json::json!(true));
        return Some(serde_json::Value::Object(out));
    }

    for (history_field, old_key, new_key) in [
        ("offset_value", "offset_value", "offset_value"),
        (
            "offset_extra_value",
            "offset_extra_value",
            "offset_extra_value",
        ),
        ("sample_data_start", "sample_start", "sample_start"),
        ("comment", "comment", "comment"),
    ] {
        if let Some(change) = scalar_operation_change(&old_entry[old_key], &new_entry[new_key]) {
            out.insert(history_field.to_string(), change);
        }
    }

    let old_layers = old_entry["layers"].as_array().cloned().unwrap_or_default();
    let new_layers = new_entry["layers"].as_array().cloned().unwrap_or_default();
    let max_len = old_layers.len().max(new_layers.len());
    let mut layer_changes: Vec<serde_json::Value> = Vec::new();
    for idx in 0..max_len {
        let old_layer = old_layers.get(idx);
        let new_layer = new_layers.get(idx);
        if let Some(layer_change) = ring_layer_change(old_layer, new_layer, idx, allow_removal) {
            layer_changes.push(layer_change);
        }
    }
    if !layer_changes.is_empty() {
        out.insert("layers".to_string(), serde_json::json!(layer_changes));
    }

    if out.is_empty() || (out.len() == 1 && out.contains_key("id")) {
        None
    } else {
        let _ = allow_removal;
        Some(serde_json::Value::Object(out))
    }
}

fn ring_codes_history_changes(
    old_ring_codes: &serde_json::Value,
    new_ring_codes: &serde_json::Value,
    allow_removal: bool,
) -> serde_json::Value {
    let mut old_arr = old_ring_codes.as_array().cloned().unwrap_or_default();
    let mut new_arr = new_ring_codes.as_array().cloned().unwrap_or_default();
    let max_layers = old_arr
        .iter()
        .chain(new_arr.iter())
        .map(|e| e["layers"].as_array().map(|a| a.len()).unwrap_or(0))
        .max()
        .unwrap_or(0);
    disc_service::sort_ring_codes_json(&mut old_arr, max_layers);
    disc_service::sort_ring_codes_json(&mut new_arr, max_layers);
    let mut changes: Vec<serde_json::Value> = Vec::new();
    let mut old_by_id: std::collections::HashMap<i32, &serde_json::Value> =
        std::collections::HashMap::new();
    for old_entry in &old_arr {
        if let Some(id) = old_entry
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
        {
            old_by_id.insert(id, old_entry);
        }
    }
    let mut seen_old_ids = std::collections::HashSet::new();

    for new_entry in &new_arr {
        let maybe_id = new_entry
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32);
        if let Some(id) = maybe_id {
            if let Some(old_entry) = old_by_id.get(&id) {
                seen_old_ids.insert(id);
                if let Some(change) =
                    ring_entry_change(Some(old_entry), Some(new_entry), allow_removal, Some(id))
                {
                    changes.push(change);
                }
            } else if let Some(change) =
                ring_entry_change(None, Some(new_entry), allow_removal, None)
            {
                changes.push(change);
            }
        } else if let Some(change) = ring_entry_change(None, Some(new_entry), allow_removal, None) {
            changes.push(change);
        }
    }

    if allow_removal {
        for old_entry in &old_arr {
            if let Some(id) = old_entry
                .get("id")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
            {
                if !seen_old_ids.contains(&id) {
                    if let Some(change) = ring_entry_change(Some(old_entry), None, true, Some(id)) {
                        changes.push(change);
                    }
                }
            }
        }
    }

    serde_json::json!(changes)
}

fn build_history_changes(
    form: &DiscEditForm,
    detail: Option<&DiscDetail>,
    all_media_types: &[EditMediaTypeRow],
) -> serde_json::Value {
    let db_snapshot = detail
        .map(disc_service::build_snapshot_from_disc)
        .unwrap_or_else(|| serde_json::json!({}));
    let form_snapshot = build_flat_changes(form, all_media_types);

    let Some(db_obj) = db_snapshot.as_object() else {
        return serde_json::json!({});
    };
    let Some(form_obj) = form_snapshot.as_object() else {
        return serde_json::json!({});
    };

    let mut changes = serde_json::Map::new();
    let allow_removal = detail.is_some();

    for key in [
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
        "pvd",
        "header",
        "bca",
        "pic",
        "cuesheet",
        "status",
        "layerbreaks",
    ] {
        let old = db_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let new = form_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let Some(change) = scalar_operation_change(old, new) else {
            continue;
        };
        changes.insert(key.to_string(), change);
    }

    {
        let key = "dat";
        let old = db_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let new = form_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let different = match (old.as_str(), new.as_str()) {
            (Some(o), Some(n)) => dat_tracks_differ(o, n),
            _ => old != new,
        };
        if different {
            if let Some(change) = scalar_operation_change(old, new) {
                changes.insert(key.to_string(), change);
            }
        }
    }

    for key in ["regions", "languages", "serial", "edition", "barcode"] {
        let old = db_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let new = form_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let old_values = json_str_vec(old);
        let new_values = json_str_vec(new);
        if let Some(change) = set_operation_change(&old_values, &new_values, allow_removal) {
            changes.insert(key.to_string(), change);
        }
    }

    {
        let old_ring_codes_ui = detail
            .map(build_ring_codes_json_from_detail)
            .unwrap_or_else(|| serde_json::json!([]));
        let new_ring_codes = form_obj
            .get("ring_codes")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        let list = ring_codes_history_changes(&old_ring_codes_ui, &new_ring_codes, allow_removal);
        if list.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            changes.insert("ring_codes".to_string(), list);
        }
    }

    serde_json::Value::Object(changes)
}

pub(crate) fn build_sparse_edit_changes(
    form: &DiscEditForm,
    detail: &DiscDetail,
    all_media_types: &[EditMediaTypeRow],
) -> serde_json::Value {
    build_history_changes(form, Some(detail), all_media_types)
}

pub(crate) fn build_new_disc_changes(
    form: &DiscEditForm,
    all_media_types: &[EditMediaTypeRow],
) -> serde_json::Value {
    build_history_changes(form, None, all_media_types)
}

#[cfg(test)]
mod operation_delta_tests {
    use super::*;

    fn media_rows() -> Vec<EditMediaTypeRow> {
        vec![EditMediaTypeRow {
            code: "DVD".to_string(),
            name: "DVD-ROM".to_string(),
            layer_count: 2,
            pic: false,
            rom_extension: "iso".to_string(),
        }]
    }

    fn media_type() -> MediaType {
        MediaTypeRow {
            code: "DVD".to_string(),
            name: "DVD-ROM".to_string(),
            layer_count: 2,
            pic: false,
            rom_extension: "iso".to_string(),
        }
        .into()
    }

    fn test_system() -> System {
        System {
            code: "SYS".to_string(),
            system_type: "Console".to_string(),
            manufacturer: "VG".to_string(),
            name: "Index".to_string(),
            short_name: "VGI".to_string(),
            media_types: vec!["DVD".to_string()],
            has_exe_date: true,
            has_sbi: true,
            has_pvd: true,
            has_edc: true,
            has_disc_id: true,
            has_key: true,
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
        }
    }

    fn region(code: &str) -> Region {
        Region {
            code: code.to_string(),
            name: code.to_string(),
            flag_code: code.to_string(),
            sort_order: 0,
        }
    }

    fn language(code: &str) -> Language {
        Language {
            code: code.to_string(),
            name: code.to_string(),
            flag_code: code.to_string(),
            sort_order: 0,
        }
    }

    fn ring_layer(
        entry_id: i32,
        layer: i32,
        mastering: &str,
        toolstamps: &str,
    ) -> DiscRingCodeLayer {
        DiscRingCodeLayer {
            id: entry_id * 10 + layer,
            entry_id,
            layer,
            mastering_code: Some(mastering.to_string()),
            mastering_sid: Some(format!("SID-{mastering}")),
            mould_sids: String::new(),
            toolstamps: toolstamps.to_string(),
            additional_moulds: String::new(),
        }
    }

    fn ring_entry(id: i32, mastering: &str, toolstamps: &str, comment: &str) -> RingEntryView {
        RingEntryView {
            id,
            offset_value: None,
            offset_extra_value: None,
            sample_data_start: None,
            comment: Some(comment.to_string()),
            layers: vec![ring_layer(id, 0, mastering, toolstamps)],
        }
    }

    fn base_detail() -> DiscDetail {
        DiscDetail {
            disc: Disc {
                id: 1,
                system_code: "SYS".to_string(),
                media_type: media_type(),
                title: "Old Game".to_string(),
                title_foreign: Some("Old Foreign".to_string()),
                disc_title: Some("Old Disc Title".to_string()),
                disc_number: Some("1".to_string()),
                serial: vec!["OLD-001".to_string(), "KEEP-002".to_string()],
                category: Category::Games,
                version: Some("1.0".to_string()),
                edition: vec!["Original".to_string()],
                barcode: vec!["111111111111".to_string()],
                comments: Some("old comment".to_string()),
                contents: Some("old contents".to_string()),
                filename_suffix: Some("Old Suffix".to_string()),
                error_count: Some(1),
                exe_date: Some("2020-01-01".to_string()),
                edc: true,
                layerbreaks: Some(vec![10, 20]),
                protection: Some("old protection".to_string()),
                sbi: Some("old sbi".to_string()),
                disc_id: Some("old-disc-id".to_string()),
                disc_key: Some(vec![0x12, 0x34]),
                cue: None,
                pvd: None,
                pic: None,
                header: None,
                bca: None,
                status: DiscStatus::Unverified,
            },
            system: test_system(),
            regions: vec![region("Europe"), region("Americas")],
            languages: vec![language("en")],
            ring_entries: vec![
                ring_entry(10, "MASTER-A", "T1", "old ring"),
                ring_entry(20, "MASTER-B", "T9", "remove ring"),
            ],
            files: vec![],
            dumpers: vec![],
            disc_submission_count: 0,
            sector_ranges: vec![],
            added_at: None,
            modified_at: None,
        }
    }

    fn form_from_detail(detail: &DiscDetail) -> DiscEditForm {
        DiscEditForm {
            system_code: detail.disc.system_code.clone(),
            media_type: detail.disc.media_type.code().to_string(),
            title: detail.disc.title.clone(),
            title_foreign: detail.disc.title_foreign.clone(),
            disc_number: detail.disc.disc_number.clone(),
            disc_title: detail.disc.disc_title.clone(),
            filename_suffix: detail.disc.filename_suffix.clone(),
            category: detail.disc.category.to_string(),
            regions: detail.regions.iter().map(|r| r.code.clone()).collect(),
            languages: detail.languages.iter().map(|l| l.code.clone()).collect(),
            serial: detail.disc.serial.clone(),
            version: detail.disc.version.clone(),
            edition: detail.disc.edition.clone(),
            barcode: detail.disc.barcode.clone(),
            ring_codes_json: Some(build_ring_codes_json_from_detail(detail).to_string()),
            comments: detail.disc.comments.clone(),
            contents: detail.disc.contents.clone(),
            error_count: detail.disc.error_count.map(|v| v.to_string()),
            exe_date: detail.disc.exe_date.clone(),
            edc: if detail.disc.edc {
                vec!["true".to_string()]
            } else {
                vec!["false".to_string()]
            },
            layerbreak: detail
                .disc
                .layerbreaks
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|v| v.to_string())
                .collect(),
            pvd: None,
            pic: None,
            bca: None,
            header: None,
            protection: detail.disc.protection.clone(),
            sector_ranges: None,
            sbi: detail.disc.sbi.clone(),
            protection_key_disc_key: detail.disc.disc_key.as_ref().map(|bytes| {
                bytes
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>()
            }),
            protection_key_disc_id: detail.disc.disc_id.clone(),
            cue: detail.disc.cue.clone(),
            files_xml: None,
            status: detail.disc.status.to_string(),
            submission_comment: None,
            submit_as: None,
            dump_log: None,
            extra_upload_url: None,
        }
    }

    fn new_disc_form() -> DiscEditForm {
        DiscEditForm {
            system_code: "SYS".to_string(),
            media_type: "DVD".to_string(),
            title: "New Game".to_string(),
            title_foreign: Some("New Foreign".to_string()),
            disc_number: Some("2".to_string()),
            disc_title: Some("Install Disc".to_string()),
            filename_suffix: None,
            category: "Games".to_string(),
            regions: vec![
                "Europe".to_string(),
                "Asia".to_string(),
                "Europe".to_string(),
            ],
            languages: vec!["en".to_string(), "ja".to_string(), "en".to_string()],
            serial: vec!["ABC-001".to_string(), "DEF-002".to_string(), "ABC-001".to_string()],
            version: Some("1.0".to_string()),
            edition: vec!["Original".to_string()],
            barcode: vec!["1234567890123".to_string()],
            ring_codes_json: Some(
                serde_json::json!([{
                    "offset_value": "0",
                    "offset_extra_value": "",
                    "sample_start": "123",
                    "comment": "new ring",
                    "layers": [{
                        "mastering_code": "MASTER-N",
                        "mastering_sid": "SID-N",
                        "toolstamps": "T2, T1, T2",
                        "mould_sids": "M2, M1",
                        "additional_moulds": ""
                    }]
                }])
                .to_string(),
            ),
            comments: Some("new comment".to_string()),
            contents: Some("new contents".to_string()),
            error_count: Some("7".to_string()),
            exe_date: Some("2024-01-01".to_string()),
            edc: vec!["true".to_string()],
            layerbreak: vec!["12345".to_string(), "67890".to_string()],
            pvd: None,
            pic: None,
            bca: None,
            header: None,
            protection: Some("new protection".to_string()),
            sector_ranges: None,
            sbi: Some("new sbi".to_string()),
            protection_key_disc_key: Some("aabbccdd".to_string()),
            protection_key_disc_id: Some("new-disc-id".to_string()),
            cue: None,
            files_xml: Some(
                r#"<rom name="New Game.iso" size="1" crc="11111111" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" />"#.to_string(),
            ),
            status: "Verified".to_string(),
            submission_comment: None,
            submit_as: None,
            dump_log: None,
            extra_upload_url: None,
        }
    }

    fn systems_with_edc(has_edc: bool) -> Vec<System> {
        let mut system = test_system();
        system.has_edc = has_edc;
        vec![system]
    }

    #[test]
    fn ring_layers_adds_label_side_layer() {
        assert_eq!(ring_layers(1), 2);
        assert_eq!(ring_layers(2), 3);
        assert_eq!(ring_layers(3), 4);
    }

    #[test]
    fn build_ring_codes_json_includes_label_side_slot() {
        let detail = base_detail();

        let ring_codes = build_ring_codes_json_from_detail(&detail);
        let first_entry = &ring_codes.as_array().unwrap()[0];
        let layers = first_entry["layers"].as_array().unwrap();

        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0]["mastering_code"], "MASTER-A");
        assert_eq!(layers[2]["mastering_code"], "");
    }

    #[test]
    fn validate_form_reports_missing_required_fields_in_red_box_errors() {
        let mut form = new_disc_form();
        form.system_code = String::new();
        form.title = String::new();
        form.regions = vec![];

        let errors = validate_form(&form, &media_rows(), &systems_with_edc(true));

        assert!(errors.contains(&"System: must be selected".to_string()));
        assert!(errors.contains(&"Title: cannot be empty".to_string()));
        assert!(errors.contains(&"Regions: at least one region must be selected".to_string()));
    }

    #[test]
    fn validate_form_requires_edc_choice_for_edc_systems() {
        let mut form = new_disc_form();
        form.edc = vec![];

        let errors = validate_form(&form, &media_rows(), &systems_with_edc(true));

        assert!(errors.contains(&"EDC: select Yes or No".to_string()));
    }

    #[test]
    fn validate_form_accepts_yes_or_no_edc_choices() {
        let mut form = new_disc_form();

        form.edc = vec!["true".to_string()];
        assert!(
            !validate_form(&form, &media_rows(), &systems_with_edc(true))
                .contains(&"EDC: select Yes or No".to_string())
        );

        form.edc = vec!["false".to_string()];
        assert!(
            !validate_form(&form, &media_rows(), &systems_with_edc(true))
                .contains(&"EDC: select Yes or No".to_string())
        );
    }

    #[test]
    fn validate_form_does_not_require_edc_for_non_edc_systems() {
        let mut form = new_disc_form();
        form.edc = vec![];

        let errors = validate_form(&form, &media_rows(), &systems_with_edc(false));

        assert!(!errors.contains(&"EDC: select Yes or No".to_string()));
    }

    #[test]
    fn add_validation_result_message_reflects_match_state() {
        let matched = add_validation_result_for_match(Some((
            123,
            "Game Title (Disc 2) (Bonus Disc)".to_string(),
        )));
        assert_eq!(matched.text, "This is a verification submission for");
        assert_eq!(matched.disc_id, 123);
        assert_eq!(matched.disc_title, "Game Title (Disc 2) (Bonus Disc)");

        let unmatched = add_validation_result_for_match(None);
        assert_eq!(unmatched.text, "This is a new disc submission.");
        assert_eq!(unmatched.disc_id, 0);
        assert_eq!(unmatched.disc_title, "");
    }

    #[test]
    fn add_validation_result_skips_invalid_or_missing_dat() {
        let mut form = new_disc_form();
        form.files_xml = None;
        assert!(valid_dat_for_matching(&form).is_none());

        form.files_xml = Some("not dat".to_string());
        assert!(valid_dat_for_matching(&form).is_none());

        form.files_xml = Some(
            r#"<rom name="New Game.iso" size="1" crc="11111111" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" />"#.to_string(),
        );
        assert!(valid_dat_for_matching(&form).is_some());
    }

    #[test]
    fn generated_disc_name_uses_canonical_rom_name_parts() {
        let region_names = vec!["Japan".to_string(), "USA".to_string()];
        let language_codes = vec!["en".to_string(), "ja".to_string()];

        assert_eq!(
            generated_disc_name(
                "  Sample Game  ",
                &region_names,
                &language_codes,
                Some(" 2 "),
                Some(" Bonus Disc "),
                Some(" Rev 1 "),
            ),
            "Sample Game (Japan, USA) (En,Ja) (Disc 2) (Bonus Disc) (Rev 1)"
        );

        assert_eq!(
            generated_disc_name(
                "Sample Game",
                &region_names,
                &["en".to_string()],
                None,
                None,
                None,
            ),
            "Sample Game (Japan, USA)"
        );
    }

    #[test]
    fn generated_name_key_is_case_insensitive() {
        assert_eq!(
            generated_name_key("Sample Game (USA)"),
            generated_name_key("sample game (usa)")
        );
    }

    #[test]
    fn scalar_operation_change_writes_add_modify_and_remove() {
        assert_eq!(
            scalar_operation_change(&serde_json::Value::Null, &serde_json::json!("New")),
            Some(serde_json::json!({ "add": { "new": "New" } }))
        );
        assert_eq!(
            scalar_operation_change(&serde_json::json!("Old"), &serde_json::json!("New")),
            Some(serde_json::json!({ "modify": { "old": "Old", "new": "New" } }))
        );
        assert_eq!(
            scalar_operation_change(&serde_json::json!("Old"), &serde_json::Value::Null),
            Some(serde_json::json!({ "remove": { "old": "Old" } }))
        );
    }

    #[test]
    fn set_operation_change_is_case_sensitive_and_uses_add_remove() {
        let old = vec!["Original".to_string(), "Europe".to_string()];
        let new = vec!["original".to_string(), "Asia".to_string()];

        assert_eq!(
            set_operation_change(&old, &new, true),
            Some(serde_json::json!({
                "add": ["original", "Asia"],
                "remove": ["Original", "Europe"]
            }))
        );
    }

    #[test]
    fn build_new_disc_changes_writes_explicit_add_delta_without_db_dedup() {
        let form = new_disc_form();

        let changes = build_new_disc_changes(&form, &media_rows());

        assert_eq!(
            changes["title"],
            serde_json::json!({ "add": { "new": "New Game" } })
        );
        assert_eq!(
            changes["error_count"],
            serde_json::json!({ "add": { "new": 7 } })
        );
        assert_eq!(
            changes["edc"],
            serde_json::json!({ "add": { "new": true } })
        );
        assert_eq!(
            changes["layerbreaks"],
            serde_json::json!({ "add": { "new": [12345, 67890] } })
        );
        assert_eq!(
            changes["regions"],
            serde_json::json!({ "add": ["Europe", "Asia", "Europe"] })
        );
        assert_eq!(
            changes["languages"],
            serde_json::json!({ "add": ["en", "ja", "en"] })
        );
        assert_eq!(
            changes["serial"],
            serde_json::json!({ "add": ["ABC-001", "DEF-002", "ABC-001"] })
        );
        assert!(changes["regions"].get("remove").is_none());
        assert_eq!(
            changes["ring_codes"][0]["layers"][0]["toolstamps"],
            serde_json::json!({ "add": { "new": "T1, T2" } })
        );
        assert_eq!(
            changes["status"],
            serde_json::json!({ "add": { "new": "Verified" } })
        );
    }

    #[test]
    fn generated_add_disc_delta_resolves_against_empty_or_existing_snapshot() {
        let changes = build_new_disc_changes(&new_disc_form(), &media_rows());

        let new_disc =
            queue_service::resolve_submission_snapshot(&serde_json::json!({}), &changes).unwrap();
        assert_eq!(new_disc["regions"], serde_json::json!(["Europe", "Asia"]));
        assert_eq!(new_disc["languages"], serde_json::json!(["en", "ja"]));
        assert_eq!(
            new_disc["serial"],
            serde_json::json!(["ABC-001", "DEF-002"])
        );

        let existing = serde_json::json!({
            "title": "Existing Game",
            "regions": ["Europe"],
            "languages": ["en"],
            "serial": ["ABC-001"],
            "edition": ["Original"],
            "barcode": ["1234567890123"],
            "ring_codes": []
        });
        let verification = queue_service::resolve_submission_snapshot(&existing, &changes).unwrap();

        assert_eq!(
            verification["regions"],
            serde_json::json!(["Europe", "Asia"])
        );
        assert_eq!(verification["languages"], serde_json::json!(["en", "ja"]));
        assert_eq!(
            verification["serial"],
            serde_json::json!(["ABC-001", "DEF-002"])
        );
        assert_eq!(verification["edition"], serde_json::json!(["Original"]));
        assert_eq!(
            verification["barcode"],
            serde_json::json!(["1234567890123"])
        );
    }

    #[test]
    fn build_sparse_edit_changes_returns_empty_delta_for_identical_form() {
        let detail = base_detail();
        let form = form_from_detail(&detail);

        let changes = build_sparse_edit_changes(&form, &detail, &media_rows());

        assert_eq!(changes, serde_json::json!({}));
    }

    #[test]
    fn build_sparse_edit_changes_writes_modify_remove_and_set_deltas() {
        let detail = base_detail();
        let mut form = form_from_detail(&detail);
        form.title = "Edited Game".to_string();
        form.version = Some("2.0".to_string());
        form.comments = None;
        form.error_count = None;
        form.edc = vec![];
        form.layerbreak = vec!["10".to_string(), "30".to_string()];
        form.regions = vec!["Europe".to_string(), "Asia".to_string()];
        form.languages = vec!["en".to_string(), "ja".to_string()];
        form.serial = vec!["KEEP-002".to_string(), "NEW-003".to_string()];
        form.edition = vec!["Original".to_string(), "original".to_string()];
        form.barcode = vec![];

        let changes = build_sparse_edit_changes(&form, &detail, &media_rows());

        assert_eq!(
            changes["title"],
            serde_json::json!({ "modify": { "old": "Old Game", "new": "Edited Game" } })
        );
        assert_eq!(
            changes["version"],
            serde_json::json!({ "modify": { "old": "1.0", "new": "2.0" } })
        );
        assert_eq!(
            changes["comments"],
            serde_json::json!({ "remove": { "old": "old comment" } })
        );
        assert_eq!(
            changes["error_count"],
            serde_json::json!({ "remove": { "old": 1 } })
        );
        assert_eq!(
            changes["edc"],
            serde_json::json!({ "modify": { "old": true, "new": false } })
        );
        assert_eq!(
            changes["layerbreaks"],
            serde_json::json!({ "modify": { "old": [10, 20], "new": [10, 30] } })
        );
        assert_eq!(
            changes["regions"],
            serde_json::json!({ "add": ["Asia"], "remove": ["Americas"] })
        );
        assert_eq!(changes["languages"], serde_json::json!({ "add": ["ja"] }));
        assert_eq!(
            changes["serial"],
            serde_json::json!({ "add": ["NEW-003"], "remove": ["OLD-001"] })
        );
        assert_eq!(
            changes["edition"],
            serde_json::json!({ "add": ["original"] })
        );
        assert_eq!(
            changes["barcode"],
            serde_json::json!({ "remove": ["111111111111"] })
        );
    }

    #[test]
    fn edition_inputs_remain_text_fields_with_native_selector() {
        let template = include_str!("../../templates/disc_edit.html");
        let script = include_str!("../../static/js/disc_edit.js");

        assert!(template.contains("const EDITION_SUGGESTIONS ="));
        assert!(!template.contains("<datalist"));
        assert!(!template.contains("list=\"edition-suggestions\""));
        assert!(template.contains(r#"name="edition" autocomplete="off""#));
        assert!(script.contains("function attachEditionSelector(input)"));
        assert!(script.contains("input.removeAttribute('list')"));
        assert!(script.contains("input.setAttribute('autocomplete', 'off')"));
        assert!(script.contains("function ensureEditionSelectorGroup(input)"));
        assert!(script.contains("group.className = 'edition-value-picker'"));
        assert!(script.contains("select.className = 'edition-suggestion-select'"));
        assert!(script.contains("function populateEditionSelect(select)"));
        assert!(script.contains("function refreshEditionSelectors()"));
        assert!(script.contains("var selectedEdition = select.value"));
        assert!(script.contains("input.value = selectedEdition"));
        assert!(script.contains("select.value = ''"));
        assert!(!script.contains("select.name"));
        assert!(!script.contains("select.setAttribute('name'"));
        assert!(script.contains("var INDEPENDENT_INLINE_FIELDS"));
        assert!(script.contains("'serial': true"));
        assert!(script.contains("'edition': true"));
        assert!(script.contains("'barcode': true"));
        assert!(script.contains("function attachIndependentInlineResize(input)"));
        assert!(script.contains("function fitInlineInput(input)"));
        assert!(script.contains("function fitInlineInputSoon(input)"));
        assert!(script.contains("window.requestAnimationFrame"));
        assert!(script.contains("isIndependentlySizedInlineField(input.name)"));
        assert!(!script.contains("function updateEditionDatalist"));
        assert!(!script.contains("prepareEditionPicker"));
        assert!(script.contains("function fitInlineGroupForInput(input)"));
    }

    #[test]
    fn edition_and_barcode_plus_buttons_are_hidden_only_on_add_disc() {
        let template = include_str!("../../templates/disc_edit.html");

        assert!(template.contains(
            r#"<button type="button" class="outline secondary array-add-btn" onclick="addInlineEntry('serial-list','serial')">+</button>"#
        ));
        assert!(template.contains(
            "{% if !is_add_mode %}\n        <button type=\"button\" class=\"outline secondary array-add-btn\" onclick=\"addInlineEntry('edition-list','edition')\">+</button>\n        {% endif %}"
        ));
        assert!(template.contains(
            "{% if !is_add_mode %}\n        <button type=\"button\" class=\"outline secondary array-add-btn\" onclick=\"addInlineEntry('barcode-list','barcode')\">+</button>\n        {% endif %}"
        ));
    }

    #[test]
    fn disc_meta_layout_is_template_native_without_legacy_normalizer() {
        let template = include_str!("../../templates/disc_edit.html");
        let script = include_str!("../../static/js/disc_edit.js");

        assert!(template.contains(r#"<div class="disc-meta-fields">"#));
        assert!(template.contains(r#"<label class="disc-meta-row{% if"#));
        assert!(template.contains(r#"<span class="disc-meta-label">System:</span>"#));
        assert!(!script.contains("normalizeLegacyDiscMetaLayout"));
        assert!(!script.contains("splitDiscMetaLabel"));
        assert!(!script.contains("form.insertBefore(metaFields"));
        assert!(script.contains("function fitDiscMetaFields()"));
    }

    #[test]
    fn submit_as_uses_text_field_with_native_selector() {
        let template = include_str!("../../templates/disc_edit.html");
        let script = include_str!("../../static/js/disc_edit.js");
        let css = include_str!("../../static/css/app.css");

        assert!(template.contains("const SUBMIT_AS_USERNAMES ="));
        assert!(template.contains("{% if show_submit_as %}"));
        assert!(template.contains(r#"<label>Submit As"#));
        assert!(template.contains(
            r#"<input type="text" name="submit_as" id="submit-as-input" autocomplete="off""#
        ));
        assert!(!template.contains("<datalist"));
        assert!(!template.contains("list=\"submit-as"));
        assert!(!template.contains("select name=\"submit_as\""));

        let comment_pos = template.find(r#"<label>Submission Comment"#).unwrap();
        let submit_as_pos = template.find(r#"<label>Submit As"#).unwrap();
        let validate_pos = template.find(r#"name="action" value="validate""#).unwrap();
        assert!(comment_pos < submit_as_pos);
        assert!(submit_as_pos < validate_pos);

        assert!(script.contains("function attachSubmitAsSelector(input)"));
        assert!(script.contains("input.removeAttribute('list')"));
        assert!(script.contains("input.setAttribute('autocomplete', 'off')"));
        assert!(script.contains("function ensureSubmitAsSelectorGroup(input)"));
        assert!(script.contains("group.className = 'submit-as-picker'"));
        assert!(script.contains("select.className = 'submit-as-user-select'"));
        assert!(script.contains("function populateSubmitAsSelect(select)"));
        assert!(script.contains("var selectedUser = select.value"));
        assert!(script.contains("input.value = selectedUser"));
        assert!(!script.contains("select.name"));
        assert!(!script.contains("select.setAttribute('name'"));

        assert!(css.contains(".disc-edit .submit-as-picker"));
        assert!(css.contains(".disc-edit .submit-as-picker select.submit-as-user-select"));
    }

    #[test]
    fn submit_as_validation_is_moderator_only_and_preserves_default() {
        let mut form = new_disc_form();

        assert_eq!(
            submit_as_username_for_form("CurrentUser", &form, true),
            "CurrentUser"
        );
        assert_eq!(submit_as_username_for_form("CurrentUser", &form, false), "");

        form.submit_as = Some("  ".to_string());
        assert_eq!(
            validate_submit_as_for_add(&form, true),
            vec!["Submit As: cannot be empty".to_string()]
        );
        assert!(validate_submit_as_for_add(&form, false).is_empty());

        form.submit_as = Some("OtherUser".to_string());
        assert!(validate_submit_as_for_add(&form, true).is_empty());
        assert_eq!(
            submit_as_username_for_form("CurrentUser", &form, true),
            "OtherUser"
        );

        form.submit_as = Some("a".repeat(65));
        assert_eq!(
            validate_submit_as_for_add(&form, true),
            vec!["Submit As: must be 64 characters or fewer".to_string()]
        );
    }

    #[test]
    fn add_submission_logs_are_required_unless_moderator_or_admin() {
        let mut form = new_disc_form();

        assert_eq!(
            validate_add_submission_logs(&form, false),
            vec![
                "Dump Log: cannot be empty".to_string(),
                "Logs Archive URL: cannot be empty".to_string()
            ]
        );
        assert!(validate_add_submission_logs(&form, true).is_empty());

        form.dump_log = Some("  ".to_string());
        form.extra_upload_url = Some("\n\t".to_string());
        assert_eq!(
            validate_add_submission_logs(&form, false),
            vec![
                "Dump Log: cannot be empty".to_string(),
                "Logs Archive URL: cannot be empty".to_string()
            ]
        );

        form.dump_log = Some("  redumper log  ".to_string());
        form.extra_upload_url = Some("  https://example.test/logs  ".to_string());
        assert!(validate_add_submission_logs(&form, false).is_empty());

        form.extra_upload_url = Some("not a url".to_string());
        assert_eq!(
            validate_add_submission_logs(&form, false),
            vec!["Logs Archive URL: must be a valid URL".to_string()]
        );
        assert_eq!(
            validate_add_submission_logs(&form, true),
            vec!["Logs Archive URL: must be a valid URL".to_string()]
        );

        form.extra_upload_url = Some("example.test/logs".to_string());
        assert_eq!(
            validate_add_submission_logs(&form, false),
            vec!["Logs Archive URL: must be a valid URL".to_string()]
        );

        form.extra_upload_url = Some("ftp://example.test/logs".to_string());
        assert_eq!(
            validate_add_submission_logs(&form, false),
            vec!["Logs Archive URL: must be a valid URL".to_string()]
        );

        form.extra_upload_url = Some("http://example.test/logs".to_string());
        assert!(validate_add_submission_logs(&form, false).is_empty());

        assert!(!UserRole::UserPlus.can_moderate());
        assert!(UserRole::Moderator.can_moderate());
        assert!(UserRole::Admin.can_moderate());
    }

    #[test]
    fn submit_as_user_resolution_matches_oidc_exact_case_behavior() {
        assert!(FIND_OR_CREATE_USER_SQL.contains("ON CONFLICT (username)"));
        assert!(FIND_OR_CREATE_USER_SQL.contains("RETURNING id"));
        assert!(!FIND_OR_CREATE_USER_SQL
            .to_ascii_lowercase()
            .contains("lower(username)"));

        assert_eq!(
            normalize_submit_as_username(Some("  Alice  ")).unwrap(),
            "Alice"
        );
        assert_eq!(
            normalize_submit_as_username(Some("alice")).unwrap(),
            "alice"
        );
    }

    #[test]
    fn multiline_metadata_textareas_are_not_collapsible() {
        let template = include_str!("../../templates/disc_edit.html");
        let css = include_str!("../../static/css/app.css");

        assert!(template.contains(r#"<label>Contents"#));
        assert!(template.contains(r#"<label>Protection"#));
        assert!(template.contains(r#"<label>SBI"#));
        assert!(!template.contains(
            r#"<details class="collapsible-review-field{% if !self.highlight_class("contents")"#
        ));
        assert!(!template.contains(
            r#"<details class="collapsible-review-field{% if !self.highlight_class("protection")"#
        ));
        assert!(!template.contains(
            r#"<details class="collapsible-review-field{% if !self.highlight_class("sbi")"#
        ));
        assert!(
            !css.contains(".disc-edit .disc-form-section-multiline > .collapsible-review-field")
        );
    }

    #[test]
    fn dump_log_uses_submission_comment_textarea_style() {
        let template = include_str!("../../templates/disc_edit.html");
        let css = include_str!("../../static/css/app.css");
        let script = include_str!("../../static/js/disc_edit.js");

        assert!(template.contains(
            r#"<label>Dump Log
        <textarea name="dump_log" rows="16" class="auto-expand full-width-textarea" placeholder="Paste the contents of the redumper .log file here, or an equivalent if redumper was not used.  If there is no .log file generated by dumping software for this system, please indicate the dumping software that was used">"#
        ));
        assert!(template.contains(
            r#"<textarea name="submission_comment" rows="2" class="auto-expand fixed-80""#
        ));
        assert!(template.contains(
            r#"<li>{{ err.text }} <a href="/disc/{{ err.disc_id }}/" target="_blank" rel="noopener noreferrer">{{ err.disc_title }}</a></li>"#
        ));
        assert!(!template.contains(
            r#"<textarea name="dump_log" rows="5" class="hex-dump-input auto-expand fixed-80">"#
        ));
        assert!(!template.contains(concat!(
            r#"<details class=""#,
            "dump",
            "-log",
            "-collapsible"
        )));
        assert!(!css.contains(concat!(".dump", "-log", "-collapsible")));
        assert!(!script.contains(concat!("init", "CollapsibleReviewFields")));
        assert!(css.contains("textarea.full-width-textarea {\n    width: 100%;\n}"));
    }

    #[test]
    fn build_sparse_edit_changes_can_add_modify_and_remove_ring_codes() {
        let detail = base_detail();
        let mut form = form_from_detail(&detail);
        form.ring_codes_json = Some(
            serde_json::json!([
                {
                    "id": 10,
                    "offset_value": "",
                    "offset_extra_value": "",
                    "sample_start": "",
                    "comment": "updated ring",
                    "layers": [{
                        "mastering_code": "MASTER-A",
                        "mastering_sid": "SID-MASTER-A",
                        "toolstamps": "T1, T3",
                        "mould_sids": "",
                        "additional_moulds": ""
                    }]
                },
                {
                    "offset_value": "88",
                    "offset_extra_value": "",
                    "sample_start": "",
                    "comment": "new ring",
                    "layers": [{
                        "mastering_code": "MASTER-C",
                        "mastering_sid": "SID-C",
                        "toolstamps": "T5",
                        "mould_sids": "",
                        "additional_moulds": ""
                    }]
                }
            ])
            .to_string(),
        );

        let changes = build_sparse_edit_changes(&form, &detail, &media_rows());
        let rings = changes["ring_codes"].as_array().unwrap();

        let modified = rings
            .iter()
            .find(|entry| entry["id"].as_i64() == Some(10))
            .unwrap();
        assert_eq!(
            modified["comment"],
            serde_json::json!({ "modify": { "old": "old ring", "new": "updated ring" } })
        );
        assert_eq!(
            modified["layers"][0]["toolstamps"],
            serde_json::json!({ "modify": { "old": "T1", "new": "T1, T3" } })
        );

        let removed = rings
            .iter()
            .find(|entry| entry["id"].as_i64() == Some(20))
            .unwrap();
        assert_eq!(removed["remove"], true);

        let added = rings
            .iter()
            .find(|entry| entry.get("id").is_none())
            .unwrap();
        assert_eq!(
            added["offset_value"],
            serde_json::json!({ "add": { "new": "88" } })
        );
        assert_eq!(
            added["layers"][0]["mastering_code"],
            serde_json::json!({ "add": { "new": "MASTER-C" } })
        );
    }
}

fn format_hex_dump_edit(data: &[u8], base_addr: usize) -> String {
    let mut out = String::new();
    let total_chunks = data.chunks(16).len();
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = base_addr + i * 16;
        out.push_str(&format!("{:04X} : ", offset));
        for (j, byte) in chunk.iter().enumerate() {
            out.push_str(&format!("{:02X} ", byte));
            if j == 7 {
                out.push(' ');
            }
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push_str("  ");
        for byte in chunk {
            if byte.is_ascii_graphic() || *byte == b' ' {
                out.push(*byte as char);
            } else {
                out.push(' ');
            }
        }
        if i + 1 < total_chunks {
            out.push('\n');
        }
    }
    out
}

pub(crate) fn normalize_pvd_hex_dump(text: &str) -> String {
    let bytes = disc_service::parse_hex_dump(text);
    format_pvd_hex_dump(&bytes)
}

pub(crate) fn format_pvd_hex_dump(data: &[u8]) -> String {
    const PVD_FULL_SIZE: usize = 96;
    const PVD_STORED_SIZE: usize = 82;
    let mut buf = [0u8; PVD_FULL_SIZE];
    let copy_len = data.len().min(PVD_STORED_SIZE);
    buf[..copy_len].copy_from_slice(&data[..copy_len]);
    format_hex_dump_edit(&buf, 0x0320)
}

pub(crate) fn format_header_hex_dump(data: &[u8]) -> String {
    format_hex_dump_edit(data, 0x0000)
}
