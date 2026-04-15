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
    pub media_rom_extensions_json: String,

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
    pub removed_serials: Vec<String>,
    pub removed_editions: Vec<String>,
    pub removed_barcodes: Vec<String>,

    pub ring_codes_json: String,
    pub ring_highlights_json: String,

    pub comments: String,
    pub contents: String,

    pub show_error_count: bool,
    pub error_count: String,
    pub show_exe_date: bool,
    pub exe_date: String,
    pub show_edc: bool,
    pub edc_value: bool,

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

    pub show_keys: bool,
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

    pub questionable: bool,
    pub enabled: bool,

    pub is_add_mode: bool,
    pub dump_log: String,
    pub dump_log_required: bool,
    pub extra_upload_url: String,

    pub submit_button_text: String,
    pub validation_errors: Vec<String>,

    pub is_review_mode: bool,
    pub changed_fields: Vec<String>,
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
    pub created_at_display: String,
    pub reviewed_at_display: String,
    pub changes_json: String,
}
impl SiteConfig for DiscEditTemplate {}

impl DiscEditTemplate {
    pub fn highlight_class(&self, field: &str) -> &str {
        for f in &self.changed_fields {
            if f.len() > field.len()
                && f.as_bytes()[field.len()] == b':'
                && f.starts_with(field)
            {
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
}

pub(crate) struct HighlightedValue {
    pub value: String,
    pub highlight: String,
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
    let all_categories: Vec<CategoryRow> = sqlx::query_as(
        "SELECT id, name FROM categories ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    let all_regions: Vec<Region> =
        sqlx::query_as("SELECT * FROM regions ORDER BY sort_order")
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

pub(crate) fn build_systems_json(
    all_systems: &[System],
) -> (String, String) {
    let mut systems_media_map = serde_json::Map::new();
    let mut systems_has_flags_map = serde_json::Map::new();
    for s in all_systems {
        systems_media_map.insert(s.code.clone(), serde_json::json!(s.media_types));
        systems_has_flags_map.insert(s.code.clone(), serde_json::json!({
            "has_title_foreign": s.has_title_foreign,
            "has_disc_number": s.has_disc_number,
            "has_disc_title": s.has_disc_title,
            "has_serial": s.has_serial,
            "has_version": s.has_version,
            "has_edition": s.has_edition,
            "has_barcode": s.has_barcode,
            "has_exe_date": s.has_exe_date,
            "has_edc": s.has_edc,
            "has_keys": s.has_keys,
            "has_pvd": s.has_pvd,
            "has_bca": s.has_bca,
            "has_header": s.has_header,
            "has_protection": s.has_protection,
            "has_sector_ranges": s.has_sector_ranges,
            "has_sbi": s.has_sbi,
            "has_sample_start": s.has_sample_start,
            "has_offset_extra": s.has_offset_extra,
        }));
    }
    let systems_media_json =
        serde_json::to_string(&systems_media_map).unwrap_or_else(|_| "{}".into());
    let systems_has_flags_json =
        serde_json::to_string(&systems_has_flags_map).unwrap_or_else(|_| "{}".into());
    (systems_media_json, systems_has_flags_json)
}

pub(crate) fn build_media_rom_extensions_json(all_media_types: &[EditMediaTypeRow]) -> String {
    let mut map = serde_json::Map::new();
    for m in all_media_types {
        map.insert(m.code.clone(), serde_json::json!(m.rom_extension));
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
        .map_or(false, |m| m.rom_extension != "iso")
}

pub(crate) fn build_system_options(all_systems: &[System], selected: &str) -> Vec<SystemOption> {
    let mut systems: Vec<SystemOption> = all_systems
        .iter()
        .map(|s| SystemOption {
            code: s.code.clone(),
            name: s.name.clone(),
            selected: s.code == selected,
        })
        .collect();
    systems.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    systems
}

pub(crate) fn build_media_options(all_media_types: &[EditMediaTypeRow], selected: &str) -> Vec<MediaTypeOption> {
    all_media_types
        .iter()
        .map(|m| MediaTypeOption {
            code: m.code.clone(),
            name: m.name.clone(),
            selected: m.code == selected,
        })
        .collect()
}

pub(crate) fn build_category_options(all_categories: &[CategoryRow], selected: &str) -> Vec<SelectOption> {
    all_categories
        .iter()
        .map(|c| SelectOption {
            value: c.name.clone(),
            name: c.name.clone(),
            selected: selected == c.name,
        })
        .collect()
}

pub(crate) fn build_check_options(
    all: &[Region],
    selected_codes: &[String],
) -> Vec<CheckOption> {
    let mut options: Vec<CheckOption> = all
        .iter()
        .map(|r| CheckOption {
            value: r.code.trim().to_string(),
            name: r.name.clone(),
            code: r.flag_code.trim().to_lowercase(),
            selected: selected_codes.iter().any(|c| c.trim() == r.code.trim()),
            highlight: String::new(),
        })
        .collect();
    options.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    options
}

pub(crate) fn build_lang_check_options(
    all: &[Language],
    selected_codes: &[String],
) -> Vec<CheckOption> {
    let mut options: Vec<CheckOption> = all
        .iter()
        .map(|l| CheckOption {
            value: l.code.trim().to_string(),
            name: l.name.clone(),
            code: l.flag_code.trim().to_lowercase(),
            selected: selected_codes.iter().any(|c| c.trim() == l.code.trim()),
            highlight: String::new(),
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
    media_layers.max(2)
}

async fn edit_page(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    let ref_data = fetch_ref_data(&state.pool).await?;

    let disc_region_codes: Vec<String> = sqlx::query_scalar(
        "SELECT region_code FROM disc_regions WHERE disc_id = $1",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let disc_lang_codes: Vec<String> = sqlx::query_scalar(
        "SELECT language_code FROM disc_languages WHERE disc_id = $1",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let (systems_media_json, systems_has_flags_json) =
        build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
    let max_layers = detail.disc.media_type.max_layers();
    let ring_layer_count = ring_layers(max_layers);

    let mut sorted_ring_entries = detail.ring_entries.clone();
    disc_service::sort_ring_entry_views(
        &mut sorted_ring_entries,
        ring_layer_count as usize,
    );
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
            let name = build_simple_track_name(
                f.track_number.as_deref(),
                total_tracks,
                rom_extension,
            );
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
            media_types_all: build_media_options(&ref_data.all_media_types, detail.disc.media_type.code()),
            categories: build_category_options(&ref_data.all_categories, &detail.disc.category.to_string()),
            regions: build_check_options(&ref_data.all_regions, &disc_region_codes),
            languages: build_lang_check_options(&ref_data.all_languages, &disc_lang_codes),

            system_code: detail.disc.system_code.clone(),
            media_type_code: detail.disc.media_type.code().to_string(),
            max_layers,
            media_layers_json,
            systems_media_json,
            systems_has_flags_json,
            media_rom_extensions_json,

            title: detail.disc.title.clone(),
            show_title_foreign: detail.system.has_title_foreign,
            title_foreign: detail.disc.title_foreign.clone().unwrap_or_default(),
            show_disc_number: detail.system.has_disc_number,
            disc_number: detail.disc.disc_number.clone().unwrap_or_default(),
            show_disc_title: detail.system.has_disc_title,
            disc_title: detail.disc.disc_title.clone().unwrap_or_default(),
            filename_suffix: detail.disc.filename_suffix.clone().unwrap_or_default(),

            show_serial: detail.system.has_serial,
            serials: detail.disc.serial
                .iter()
                .cloned()
                .map(|s| HighlightedValue { value: s, highlight: String::new() })
                .collect(),
            show_version: detail.system.has_version,
            version: detail.disc.version.clone().unwrap_or_default(),
            show_edition: detail.system.has_edition,
            editions: detail.disc.edition
                .iter()
                .cloned()
                .map(|s| HighlightedValue { value: s, highlight: String::new() })
                .collect(),
            show_barcode: detail.system.has_barcode,
            barcodes: detail.disc.barcode
                .iter()
                .cloned()
                .map(|s| HighlightedValue { value: s, highlight: String::new() })
                .collect(),
            removed_serials: vec![],
            removed_editions: vec![],
            removed_barcodes: vec![],

            ring_codes_json,
            ring_highlights_json: "[]".to_string(),

            comments: detail.disc.comments.clone().unwrap_or_default(),
            contents: detail.disc.contents.clone().unwrap_or_default(),

            show_error_count: rom_extension != "iso",
            error_count: detail.disc.error_count.map(|e| e.to_string()).unwrap_or_default(),
            show_exe_date: detail.system.has_exe_date,
            exe_date: detail.disc.exe_date.clone().unwrap_or_default(),
            show_edc: detail.system.has_edc,
            edc_value: detail.disc.edc,

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

            show_keys: detail.system.has_keys,
            show_protection: detail.system.has_protection,
            protection: detail.disc.protection.clone().unwrap_or_default(),
            show_sector_ranges: detail.system.has_sector_ranges,
            sector_ranges_text,
            show_sbi: detail.system.has_sbi,
            sbi: detail.disc.sbi.clone().unwrap_or_default(),
            has_sample_start: detail.system.has_sample_start,
            protection_key_disc_key: detail
                .disc
                .keys
                .as_deref()
                .unwrap_or_default()
                .first()
                .cloned()
                .unwrap_or_default(),
            protection_key_disc_id: detail
                .disc
                .keys
                .as_deref()
                .unwrap_or_default()
                .get(1)
                .cloned()
                .unwrap_or_default(),

            cue: detail.disc.cue.as_deref()
                .filter(|s| !s.is_empty())
                .map(|c| simplify_cue(c, rom_extension))
                .unwrap_or_default(),
            files_xml,

            questionable: detail.disc.questionable,
            enabled: detail.disc.enabled,

            is_add_mode: false,
            dump_log: String::new(),
            dump_log_required: false,
            extra_upload_url: String::new(),

            submit_button_text: if user.role.can_edit_directly() { "Save".into() } else { "Submit".into() },
            validation_errors: vec![],

            is_review_mode: false,
            changed_fields: vec![],
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
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub questionable: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many_strings")]
    pub enabled: Vec<String>,
    pub submission_comment: Option<String>,
    pub dump_log: Option<String>,
    pub extra_upload_url: Option<String>,
}

pub(crate) fn validate_form(form: &DiscEditForm, all_media_types: &[EditMediaTypeRow]) -> Vec<String> {
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
                errors.push(format!("Layerbreak {}: must be a non-negative integer", i + 1));
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

    let rom_ext = all_media_types.iter()
        .find(|m| m.code == form.media_type)
        .map(|m| m.rom_extension.as_str())
        .unwrap_or("");

    if rom_ext == "bin" {
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
    pool: &sqlx::PgPool,
    id: i32,
    username: &str,
    form: &DiscEditForm,
    errors: Vec<String>,
    is_add_mode: bool,
    can_edit_directly: bool,
) -> AppResult<Response> {
    let ref_data = fetch_ref_data(pool).await?;
    let system = disc_service::get_system(pool, &form.system_code).await.ok();

    let (systems_media_json, systems_has_flags_json) =
        build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);
    let max_layers = max_layers_for_media(&ref_data.all_media_types, &form.media_type);

    let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

    let media_pic = ref_data.all_media_types.iter()
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
        media_rom_extensions_json,

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
            .map(|s| HighlightedValue { value: s, highlight: String::new() })
            .collect(),
        show_version: has_sys(|s| s.has_version),
        version: form.version.clone().unwrap_or_default(),
        show_edition: has_sys(|s| s.has_edition),
        editions: form
            .edition
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| HighlightedValue { value: s, highlight: String::new() })
            .collect(),
        show_barcode: has_sys(|s| s.has_barcode),
        barcodes: form
            .barcode
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| HighlightedValue { value: s, highlight: String::new() })
            .collect(),
        removed_serials: vec![],
        removed_editions: vec![],
        removed_barcodes: vec![],

        ring_codes_json: form.ring_codes_json.clone().unwrap_or_else(|| "[]".into()),
        ring_highlights_json: "[]".to_string(),

        comments: form.comments.clone().unwrap_or_default(),
        contents: form.contents.clone().unwrap_or_default(),

        show_error_count,
        error_count: form.error_count.clone().unwrap_or_default(),
        show_exe_date: has_sys(|s| s.has_exe_date),
        exe_date: form.exe_date.clone().unwrap_or_default(),
        show_edc: has_sys(|s| s.has_edc),
        edc_value: form_edc_bool(form),

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

        show_keys: has_sys(|s| s.has_keys),
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

        questionable: form.questionable.iter().any(|v| v == "true"),
        enabled: form.enabled.iter().any(|v| v == "true"),

        is_add_mode,
        dump_log: form.dump_log.clone().unwrap_or_default(),
        dump_log_required: is_add_mode && !can_edit_directly,
        extra_upload_url: form.extra_upload_url.clone().unwrap_or_default(),

        submit_button_text: if can_edit_directly { "Save".into() } else { "Submit".into() },
        validation_errors: errors,

        is_review_mode: false,
        changed_fields: vec![],
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
        created_at_display: String::new(),
        reviewed_at_display: String::new(),
        changes_json: String::new(),
    };

    let html = template.render().unwrap();
    Ok((StatusCode::BAD_REQUEST, Html(html)).into_response())
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

fn form_edc_bool(form: &DiscEditForm) -> bool {
    form.edc.iter().any(|v| v == "true")
}

fn diff_str(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: &str, new: &str) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

fn diff_opt_str(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: Option<&str>, new: Option<&str>) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

fn diff_bool(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: bool, new: bool) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

fn diff_opt_bool(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: Option<bool>, new: Option<bool>) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

fn diff_opt_i32(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: Option<i32>, new: Option<i32>) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

fn diff_str_vec(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: &[String], new: &[String]) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

fn diff_i32_vec(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: &[i32], new: &[i32]) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

fn diff_json(changes: &mut serde_json::Map<String, serde_json::Value>, key: &str, old: &serde_json::Value, new: &serde_json::Value) {
    if old != new {
        changes.insert(key.to_string(), serde_json::json!({"old": old, "new": new}));
    }
}

/// Union-merge two string vecs, preserving all unique entries (case-insensitive dedup).
fn merge_str_vecs(old: &[String], new: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = old.to_vec();
    for s in new {
        if !merged.iter().any(|existing| existing.eq_ignore_ascii_case(s)) {
            merged.push(s.clone());
        }
    }
    merged
}

/// Normalize multiline text for comparison: strip \r, trim each line's trailing whitespace,
/// trim leading/trailing blank lines.
fn normalize_multiline(s: Option<&str>) -> Option<String> {
    s.map(normalize_newlines).map(|text| {
        text.lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    })
    .filter(|v| !v.is_empty())
}

/// Merge ring code entries: if an old entry matches a new entry on offset + mastering_code +
/// mastering_sid (across all layers), merge toolstamps, mould_sids, additional_moulds (union)
/// and comments (comma-delimited append). Non-matching new entries are added.
fn merge_ring_codes(old: &serde_json::Value, new: &serde_json::Value) -> serde_json::Value {
    let old_arr = match old.as_array() {
        Some(a) => a,
        None => return new.clone(),
    };
    let new_arr = match new.as_array() {
        Some(a) => a,
        None => return old.clone(),
    };

    let mut result: Vec<serde_json::Value> = old_arr.clone();
    let mut matched_old: Vec<bool> = vec![false; old_arr.len()];

    for new_entry in new_arr {
        let match_idx = old_arr.iter().enumerate().position(|(idx, old_entry)| {
            if matched_old[idx] {
                return false;
            }
            ring_entry_key_matches(old_entry, new_entry)
        });

        if let Some(idx) = match_idx {
            matched_old[idx] = true;
            result[idx] = merge_single_ring_entry(&result[idx], new_entry);
        } else {
            result.push(new_entry.clone());
        }
    }

    serde_json::json!(result)
}

pub(crate) fn ring_entry_key_matches(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    let str_field = |entry: &serde_json::Value, key: &str| {
        entry[key].as_str().unwrap_or("").to_string()
    };
    if str_field(a, "offset_value") != str_field(b, "offset_value") {
        return false;
    }
    if str_field(a, "offset_extra_value") != str_field(b, "offset_extra_value") {
        return false;
    }
    if str_field(a, "sample_start") != str_field(b, "sample_start") {
        return false;
    }

    let layers_a = a["layers"].as_array();
    let layers_b = b["layers"].as_array();
    match (layers_a, layers_b) {
        (Some(la), Some(lb)) => {
            if la.len() != lb.len() {
                return false;
            }
            la.iter().zip(lb.iter()).all(|(la_layer, lb_layer)| {
                let mc_a = la_layer["mastering_code"].as_str().unwrap_or("");
                let mc_b = lb_layer["mastering_code"].as_str().unwrap_or("");
                let ms_a = la_layer["mastering_sid"].as_str().unwrap_or("");
                let ms_b = lb_layer["mastering_sid"].as_str().unwrap_or("");
                mc_a == mc_b && ms_a == ms_b
            })
        }
        _ => false,
    }
}

fn merge_single_ring_entry(old: &serde_json::Value, new: &serde_json::Value) -> serde_json::Value {
    let mut merged = old.clone();

    let old_comment = old["comment"].as_str().unwrap_or("").trim().to_string();
    let new_comment = new["comment"].as_str().unwrap_or("").trim().to_string();
    if !new_comment.is_empty() && new_comment != old_comment {
        let combined = if old_comment.is_empty() {
            new_comment
        } else {
            format!("{}, {}", old_comment, new_comment)
        };
        merged["comment"] = serde_json::json!(combined);
    }

    if let (Some(old_layers), Some(new_layers)) =
        (old["layers"].as_array(), new["layers"].as_array())
    {
        let merged_layers: Vec<serde_json::Value> = old_layers
            .iter()
            .zip(new_layers.iter())
            .map(|(ol, nl)| {
                let mut ml = ol.clone();
                ml["toolstamps"] = serde_json::json!(
                    merge_csv_field(
                        ol["toolstamps"].as_str().unwrap_or(""),
                        nl["toolstamps"].as_str().unwrap_or(""),
                    )
                );
                ml["mould_sids"] = serde_json::json!(
                    merge_csv_field(
                        ol["mould_sids"].as_str().unwrap_or(""),
                        nl["mould_sids"].as_str().unwrap_or(""),
                    )
                );
                ml["additional_moulds"] = serde_json::json!(
                    merge_csv_field(
                        ol["additional_moulds"].as_str().unwrap_or(""),
                        nl["additional_moulds"].as_str().unwrap_or(""),
                    )
                );
                ml
            })
            .collect();
        merged["layers"] = serde_json::json!(merged_layers);
    }

    merged
}

/// Merge two comma-delimited strings, keeping unique values.
fn merge_csv_field(old: &str, new: &str) -> String {
    let mut items: Vec<String> = old
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for val in new.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !items.iter().any(|existing| existing.eq_ignore_ascii_case(val)) {
            items.push(val.to_string());
        }
    }
    items.join(", ")
}

fn build_ring_codes_json_from_detail(detail: &DiscDetail) -> serde_json::Value {
    let ring_layer_count = ring_layers(detail.disc.media_type.max_layers());
    let mut sorted_ring_entries = detail.ring_entries.clone();
    disc_service::sort_ring_entry_views(
        &mut sorted_ring_entries,
        ring_layer_count as usize,
    );
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

fn build_sector_ranges_json(ranges: &[ProtectionRange]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = ranges
        .iter()
        .map(|r| serde_json::json!({"start": r.range_start, "end": r.range_end}))
        .collect();
    serde_json::json!(arr)
}

fn build_files_xml_from_detail(detail: &DiscDetail) -> String {
    let rom_extension = detail.disc.media_type.rom_extension();
    let total_tracks = detail.files.iter().filter(|f| f.track_number.is_some()).count();
    detail
        .files
        .iter()
        .filter(|f| f.track_number.is_some())
        .map(|f| {
            let name = build_simple_track_name(
                f.track_number.as_deref(),
                total_tracks,
                rom_extension,
            );
            format!(
                r#"<rom name="{}" size="{}" crc="{}" md5="{}" sha1="{}" />"#,
                name, f.size, f.crc32, f.md5, f.sha1
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn edit_submit(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
    Form(form): Form<DiscEditForm>,
) -> AppResult<Response> {
    let ref_data = fetch_ref_data(&state.pool).await?;
    let errors = validate_form(&form, &ref_data.all_media_types);
    if !errors.is_empty() {
        return render_form_with_errors(
            &state.pool,
            id,
            &user.username,
            &form,
            errors,
            false,
            user.role.can_edit_directly(),
        )
        .await;
    }

    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    let changes = build_sparse_edit_changes(&form, &detail, &ref_data.all_media_types);

    let submission_comment = form.submission_comment.as_deref()
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
            true,
            user.id,
            None,
            &state.archive_tx,
        )
        .await?
        .ok_or(AppError::Internal("submission was already processed".into()))?;
        Ok(Redirect::to(&format!("/disc/{disc_id}/")).into_response())
    } else {
        Ok(Redirect::to("/queue/").into_response())
    }
}

async fn add_page(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
) -> AppResult<Html<String>> {
    let ref_data = fetch_ref_data(&state.pool).await?;

    let (systems_media_json, systems_has_flags_json) =
        build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let media_rom_extensions_json = build_media_rom_extensions_json(&ref_data.all_media_types);
    let media_has_pic_json = build_media_has_pic_json(&ref_data.all_media_types);

    let default_system = ref_data.all_systems.iter().find(|s| s.code == "PC");
    let has_sys = |f: fn(&System) -> bool| default_system.map_or(true, f);
    let show_error_count = false;

    Ok(Html(
        DiscEditTemplate {
            current_user: Some(user.username),
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
            media_rom_extensions_json,

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
            removed_serials: vec![],
            removed_editions: vec![],
            removed_barcodes: vec![],

            ring_codes_json: "[]".to_string(),
            ring_highlights_json: "[]".to_string(),

            comments: String::new(),
            contents: String::new(),

            show_error_count,
            error_count: String::new(),
            show_exe_date: has_sys(|s| s.has_exe_date),
            exe_date: String::new(),
            show_edc: has_sys(|s| s.has_edc),
            edc_value: false,

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

            show_keys: has_sys(|s| s.has_keys),
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

            questionable: false,
            enabled: true,

            is_add_mode: true,
            dump_log: String::new(),
            dump_log_required: !user.role.can_edit_directly(),
            extra_upload_url: String::new(),

            submit_button_text: if user.role.can_edit_directly() { "Save".into() } else { "Submit".into() },
            validation_errors: vec![],

            is_review_mode: false,
            changed_fields: vec![],
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
    Form(form): Form<DiscEditForm>,
) -> AppResult<Response> {
    let ref_data = fetch_ref_data(&state.pool).await?;
    let mut errors = validate_form(&form, &ref_data.all_media_types);

    let dump_log_text = form.dump_log.as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    if dump_log_text.is_none() && !user.role.can_edit_directly() {
        errors.push("Dump Log: cannot be empty".into());
    }

    if !errors.is_empty() {
        return render_form_with_errors(
            &state.pool,
            0,
            &user.username,
            &form,
            errors,
            true,
            user.role.can_edit_directly(),
        )
        .await;
    }

    let files_xml_str = form.files_xml.as_deref().unwrap_or("");
    let target_disc_id = queue_service::find_matching_disc(&state.pool, files_xml_str).await;
    let changes = build_new_disc_changes(&form, &ref_data.all_media_types);

    let submission_comment = form.submission_comment.as_deref()
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let sub = queue_service::create_submission(
        &state.pool,
        SubmissionType::Disc,
        user.id,
        target_disc_id,
        changes,
        submission_comment.as_deref(),
        dump_log_text,
        form.extra_upload_url.as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty()),
    )
    .await?;

    if user.role.can_edit_directly() {
        let disc_id = queue_service::approve_submission(
            &state.pool,
            &sub,
            &sub.changes,
            true,
            user.id,
            None,
            &state.archive_tx,
        )
        .await?
        .ok_or(AppError::Internal("submission was already processed".into()))?;
        Ok(Redirect::to(&format!("/disc/{disc_id}/")).into_response())
    } else {
        Ok(Redirect::to("/queue/").into_response())
    }
}

fn compute_verification_diff(
    form: &DiscEditForm,
    detail: &DiscDetail,
) -> serde_json::Map<String, serde_json::Value> {
    let mut changes = serde_json::Map::new();

    if !form.system_code.trim().is_empty() && form.system_code.trim() != detail.disc.system_code {
        diff_str(&mut changes, "system_code", &detail.disc.system_code, form.system_code.trim());
    }
    if !form.media_type.trim().is_empty() && form.media_type.trim() != detail.disc.media_type.code() {
        diff_str(&mut changes, "media_type", detail.disc.media_type.code(), form.media_type.trim());
    }
    if !form.title.trim().is_empty() && form.title.trim() != detail.disc.title {
        diff_str(&mut changes, "title", &detail.disc.title, form.title.trim());
    }
    if !form.category.trim().is_empty() && form.category.trim() != detail.disc.category.to_string() {
        diff_str(&mut changes, "category", &detail.disc.category.to_string(), form.category.trim());
    }

    let new_val = norm_opt_str(form.title_foreign.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.title_foreign.as_deref() {
        diff_opt_str(&mut changes, "title_foreign", detail.disc.title_foreign.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.disc_number.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.disc_number.as_deref() {
        diff_opt_str(&mut changes, "disc_number", detail.disc.disc_number.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.disc_title.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.disc_title.as_deref() {
        diff_opt_str(&mut changes, "disc_title", detail.disc.disc_title.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.filename_suffix.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.filename_suffix.as_deref() {
        diff_opt_str(&mut changes, "filename_suffix", detail.disc.filename_suffix.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.version.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.version.as_deref() {
        diff_opt_str(&mut changes, "version", detail.disc.version.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.exe_date.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.exe_date.as_deref() {
        diff_opt_str(&mut changes, "exe_date", detail.disc.exe_date.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.protection.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.protection.as_deref() {
        diff_opt_str(&mut changes, "protection", detail.disc.protection.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.sbi.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.sbi.as_deref() {
        diff_opt_str(&mut changes, "sbi", detail.disc.sbi.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.comments.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.comments.as_deref() {
        diff_opt_str(&mut changes, "comments", detail.disc.comments.as_deref(), new_val.as_deref());
    }
    let new_val = norm_opt_str(form.contents.as_deref());
    if new_val.is_some() && new_val.as_deref() != detail.disc.contents.as_deref() {
        diff_opt_str(&mut changes, "contents", detail.disc.contents.as_deref(), new_val.as_deref());
    }

    let new_edc = form_edc_bool(form);
    if new_edc != detail.disc.edc {
        diff_bool(&mut changes, "edc", detail.disc.edc, new_edc);
    }

    let new_error_count = form.error_count.as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| s.trim().parse::<i32>().ok());
    if new_error_count.is_some() && new_error_count != detail.disc.error_count {
        diff_opt_i32(&mut changes, "error_count", detail.disc.error_count, new_error_count);
    }

    let new_serials = norm_str_vec(form.serial.clone());
    if !new_serials.is_empty() {
        let old_serials = { let mut v = detail.disc.serial.clone(); v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase())); v };
        let merged = merge_str_vecs(&old_serials, &new_serials);
        if merged != old_serials {
            diff_str_vec(&mut changes, "serial", &old_serials, &merged);
        }
    }
    let new_editions = norm_str_vec(form.edition.clone());
    if !new_editions.is_empty() {
        let old_editions = { let mut v = detail.disc.edition.clone(); v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase())); v };
        let merged = merge_str_vecs(&old_editions, &new_editions);
        if merged != old_editions {
            diff_str_vec(&mut changes, "edition", &old_editions, &merged);
        }
    }
    let new_barcodes = norm_str_vec(form.barcode.clone());
    if !new_barcodes.is_empty() {
        let old_barcodes = { let mut v = detail.disc.barcode.clone(); v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase())); v };
        let merged = merge_str_vecs(&old_barcodes, &new_barcodes);
        if merged != old_barcodes {
            diff_str_vec(&mut changes, "barcode", &old_barcodes, &merged);
        }
    }
    let new_regions = norm_str_vec(form.regions.clone());
    if !new_regions.is_empty() {
        let old_region_codes: Vec<String> = {
            let mut v: Vec<String> = detail.regions.iter().map(|r| r.code.trim().to_string()).collect();
            v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            v
        };
        if new_regions != old_region_codes {
            diff_str_vec(&mut changes, "regions", &old_region_codes, &new_regions);
        }
    }
    let new_languages = norm_str_vec(form.languages.clone());
    if !new_languages.is_empty() {
        let old_lang_codes: Vec<String> = {
            let mut v: Vec<String> = detail.languages.iter().map(|l| l.code.trim().to_string()).collect();
            v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            v
        };
        if new_languages != old_lang_codes {
            diff_str_vec(&mut changes, "languages", &old_lang_codes, &new_languages);
        }
    }

    let new_keys: Vec<String> = [
        form.protection_key_disc_key.as_deref().unwrap_or("").trim(),
        form.protection_key_disc_id.as_deref().unwrap_or("").trim(),
    ].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    if !new_keys.is_empty() {
        let old_keys: Vec<String> = detail.disc.keys.clone().unwrap_or_default();
        if new_keys != old_keys {
            diff_str_vec(&mut changes, "keys", &old_keys, &new_keys);
        }
    }

    let new_layerbreaks: Vec<i32> = form.layerbreak.iter()
        .filter_map(|s| { let s = s.trim(); if s.is_empty() { None } else { s.parse::<i32>().ok() } })
        .collect();
    if !new_layerbreaks.is_empty() {
        let old_layerbreaks: Vec<i32> = detail.disc.layerbreaks.clone().unwrap_or_default();
        if new_layerbreaks != old_layerbreaks {
            diff_i32_vec(&mut changes, "layerbreaks", &old_layerbreaks, &new_layerbreaks);
        }
    }

    let new_ring_codes = form.ring_codes_json.as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .unwrap_or(serde_json::json!([]));
    if new_ring_codes.as_array().map_or(false, |a| !a.is_empty()) {
        let old_ring_codes = build_ring_codes_json_from_detail(detail);
        let merged_ring_codes = merge_ring_codes(&old_ring_codes, &new_ring_codes);
        if merged_ring_codes != old_ring_codes {
            diff_json(&mut changes, "ring_codes", &old_ring_codes, &merged_ring_codes);
        }
    }

    let new_sector_ranges: Vec<serde_json::Value> =
        validation::parse_sector_range_pairs(form.sector_ranges.as_deref().unwrap_or(""))
            .into_iter()
            .map(|(start, end)| serde_json::json!({"start": start, "end": end}))
            .collect();
    if !new_sector_ranges.is_empty() {
        let old_sector_ranges_json = build_sector_ranges_json(&detail.sector_ranges);
        let new_sector_ranges_json = serde_json::json!(new_sector_ranges);
        if new_sector_ranges_json != old_sector_ranges_json {
            diff_json(&mut changes, "sector_ranges", &old_sector_ranges_json, &new_sector_ranges_json);
        }
    }

    let rom_ext = detail.disc.media_type.rom_extension();
    let new_cue = norm_opt_str(form.cue.as_deref())
        .map(|c| simplify_cue(&c, rom_ext));
    if new_cue.is_some() {
        let old_cue = detail.disc.cue.as_deref()
            .filter(|s| !s.is_empty())
            .map(|c| simplify_cue(c, rom_ext));
        if normalize_multiline(new_cue.as_deref()) != normalize_multiline(old_cue.as_deref()) {
            diff_opt_str(&mut changes, "cuesheet", old_cue.as_deref(), new_cue.as_deref());
        }
    }

    let new_pvd = norm_opt_str(form.pvd.as_deref());
    if new_pvd.is_some() {
        let old_pvd = detail.disc.pvd.as_ref().map(|data| format_pvd_hex_dump(data));
        if normalize_multiline(new_pvd.as_deref()) != normalize_multiline(old_pvd.as_deref()) {
            diff_opt_str(&mut changes, "pvd", old_pvd.as_deref(), new_pvd.as_deref());
        }
    }
    let new_pic = norm_opt_str(form.pic.as_deref());
    if new_pic.is_some() {
        let old_pic = detail.disc.pic.as_ref().map(|data| format_header_hex_dump(data));
        if normalize_multiline(new_pic.as_deref()) != normalize_multiline(old_pic.as_deref()) {
            diff_opt_str(&mut changes, "pic", old_pic.as_deref(), new_pic.as_deref());
        }
    }
    let new_bca = norm_opt_str(form.bca.as_deref());
    if new_bca.is_some() {
        let old_bca = detail.disc.bca.as_ref().map(|data| format_header_hex_dump(data));
        if normalize_multiline(new_bca.as_deref()) != normalize_multiline(old_bca.as_deref()) {
            diff_opt_str(&mut changes, "bca", old_bca.as_deref(), new_bca.as_deref());
        }
    }
    let new_header = norm_opt_str(form.header.as_deref());
    if new_header.is_some() {
        let old_header = detail.disc.header.as_ref().map(|data| format_header_hex_dump(data));
        if normalize_multiline(new_header.as_deref()) != normalize_multiline(old_header.as_deref()) {
            diff_opt_str(&mut changes, "header", old_header.as_deref(), new_header.as_deref());
        }
    }

    changes
}

pub(crate) fn build_flat_changes(form: &DiscEditForm, all_media_types: &[EditMediaTypeRow]) -> serde_json::Value {
    let new_edc = form_edc_bool(form);
    let new_error_count: serde_json::Value = form.error_count.as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| s.trim().parse::<i32>().ok())
        .map(|v| serde_json::json!(v))
        .unwrap_or(serde_json::Value::Null);
    let new_layerbreaks: Vec<i32> = form.layerbreak.iter()
        .filter_map(|s| { let s = s.trim(); if s.is_empty() { None } else { s.parse::<i32>().ok() } })
        .collect();
    let new_keys: Vec<String> = [
        form.protection_key_disc_key.as_deref().unwrap_or("").trim(),
        form.protection_key_disc_id.as_deref().unwrap_or("").trim(),
    ].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    let new_ring_codes = form.ring_codes_json.as_deref()
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

    let rom_ext = all_media_types.iter()
        .find(|m| m.code == form.media_type)
        .map(|m| m.rom_extension.as_str())
        .unwrap_or("");
    let new_cue = norm_opt_multiline_str(form.cue.as_deref())
        .map(|c| simplify_cue(&c, rom_ext));

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
        "keys": new_keys,
        "cuesheet": new_cue,
        "dat": norm_opt_multiline_str(form.files_xml.as_deref()).map(|s| simplify_files_xml(&s, rom_ext)),
        "regions": new_regions,
        "languages": new_languages,
        "ring_codes": new_ring_codes,
        "sector_ranges": new_sector_ranges,
        "questionable": form.questionable.iter().any(|v| v == "true"),
        "enabled": form.enabled.iter().any(|v| v == "true"),
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

fn scalar_change(old: &serde_json::Value, new: &serde_json::Value) -> Option<serde_json::Value> {
    if old == new {
        return None;
    }
    let mut out = serde_json::Map::new();
    if !is_empty_json(old) {
        out.insert("old".to_string(), old.clone());
    }
    out.insert("new".to_string(), new.clone());
    Some(serde_json::Value::Object(out))
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
        out.insert(track, DatTrackDigest { size, crc, md5, sha1 });
    }
    Some(out)
}

fn dat_tracks_differ(old: &str, new: &str) -> bool {
    match (dat_track_map(old), dat_track_map(new)) {
        (Some(a), Some(b)) => a != b,
        _ => normalize_multiline(Some(old)) != normalize_multiline(Some(new)),
    }
}

fn csv_change(old: &serde_json::Value, new: &serde_json::Value) -> Option<serde_json::Value> {
    let to_items = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    let canonical = |items: &[String]| -> Vec<String> {
        let mut out = items
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>();
        out.sort_unstable();
        out.dedup();
        out
    };

    let old_items = to_items(old);
    let new_items = to_items(new);
    if canonical(&old_items) == canonical(&new_items) {
        return None;
    }

    let old_csv = old_items.join(",");
    let new_csv = new_items.join(",");
    let mut out = serde_json::Map::new();
    if !old_csv.is_empty() {
        out.insert("old".to_string(), serde_json::json!(old_csv));
    }
    out.insert("new".to_string(), serde_json::json!(new_csv));
    Some(serde_json::Value::Object(out))
}

fn string_list_changes(
    old_values: &[String],
    new_values: &[String],
    additions_without_index: bool,
    allow_removal: bool,
    moved_as_remove_add: bool,
) -> serde_json::Value {
    if additions_without_index {
        // Sequence-aware matching for extendable arrays (serial/edition/barcode),
        // so reordered/shifted values don't get misrepresented as positional swaps.
        let n = old_values.len();
        let m = new_values.len();
        let mut dp = vec![vec![0usize; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                if old_values[i] == new_values[j] {
                    dp[i][j] = dp[i + 1][j + 1] + 1;
                } else {
                    dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
                }
            }
        }

        let mut matches: Vec<(usize, usize)> = Vec::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < n && j < m {
            if old_values[i] == new_values[j] {
                matches.push((i, j));
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }

        let mut matched_old = vec![false; n];
        let mut matched_new = vec![false; m];
        for (oi, nj) in &matches {
            matched_old[*oi] = true;
            matched_new[*nj] = true;
        }

        let mut unmatched_old: Vec<usize> = (0..n).filter(|idx| !matched_old[*idx]).collect();
        let mut unmatched_new: Vec<usize> = (0..m).filter(|idx| !matched_new[*idx]).collect();

        let mut changes: Vec<serde_json::Value> = Vec::new();

        if allow_removal && moved_as_remove_add {
            for (old_idx, new_idx) in &matches {
                if old_idx != new_idx {
                    let mut rem = serde_json::Map::new();
                    rem.insert("index".to_string(), serde_json::json!(old_idx));
                    rem.insert("old".to_string(), serde_json::json!(old_values[*old_idx]));
                    rem.insert("new".to_string(), serde_json::Value::Null);
                    changes.push(serde_json::Value::Object(rem));

                    if !new_values[*new_idx].trim().is_empty() {
                        let mut add = serde_json::Map::new();
                        add.insert("new".to_string(), serde_json::json!(new_values[*new_idx]));
                        changes.push(serde_json::Value::Object(add));
                    }
                }
            }
        }

        while !unmatched_old.is_empty() && !unmatched_new.is_empty() {
            let old_idx = unmatched_old.remove(0);
            let new_idx = unmatched_new.remove(0);
            let old_val = &old_values[old_idx];
            let new_val = &new_values[new_idx];

            if allow_removal && new_val.trim().is_empty() {
                let mut rem = serde_json::Map::new();
                rem.insert("index".to_string(), serde_json::json!(old_idx));
                rem.insert("old".to_string(), serde_json::json!(old_val));
                rem.insert("new".to_string(), serde_json::Value::Null);
                changes.push(serde_json::Value::Object(rem));
                continue;
            }

            if old_val == new_val && old_idx != new_idx {
                let mut rem = serde_json::Map::new();
                rem.insert("index".to_string(), serde_json::json!(old_idx));
                rem.insert("old".to_string(), serde_json::json!(old_val));
                rem.insert("new".to_string(), serde_json::Value::Null);
                changes.push(serde_json::Value::Object(rem));

                let mut add = serde_json::Map::new();
                add.insert("new".to_string(), serde_json::json!(new_val));
                changes.push(serde_json::Value::Object(add));
            } else if old_val != new_val {
                let mut upd = serde_json::Map::new();
                upd.insert("index".to_string(), serde_json::json!(old_idx));
                upd.insert("old".to_string(), serde_json::json!(old_val));
                upd.insert("new".to_string(), serde_json::json!(new_val));
                changes.push(serde_json::Value::Object(upd));
            }
        }

        if allow_removal {
            for old_idx in unmatched_old {
                let mut rem = serde_json::Map::new();
                rem.insert("index".to_string(), serde_json::json!(old_idx));
                rem.insert("old".to_string(), serde_json::json!(old_values[old_idx]));
                rem.insert("new".to_string(), serde_json::Value::Null);
                changes.push(serde_json::Value::Object(rem));
            }
        }

        for new_idx in unmatched_new {
            if new_values[new_idx].trim().is_empty() {
                continue;
            }
            let mut add = serde_json::Map::new();
            add.insert("new".to_string(), serde_json::json!(new_values[new_idx]));
            changes.push(serde_json::Value::Object(add));
        }

        return serde_json::json!(changes);
    }

    let mut changes: Vec<serde_json::Value> = Vec::new();
    let common = old_values.len().min(new_values.len());

    for idx in 0..common {
        if old_values[idx] != new_values[idx] {
            let mut item = serde_json::Map::new();
            item.insert("index".to_string(), serde_json::json!(idx));
            item.insert("old".to_string(), serde_json::json!(old_values[idx]));
            if allow_removal && new_values[idx].trim().is_empty() {
                item.insert("new".to_string(), serde_json::Value::Null);
            } else {
                item.insert("new".to_string(), serde_json::json!(new_values[idx]));
            }
            changes.push(serde_json::Value::Object(item));
        }
    }

    for idx in common..new_values.len() {
        let mut item = serde_json::Map::new();
        if !additions_without_index {
            item.insert("index".to_string(), serde_json::json!(idx));
        }
        if !new_values[idx].trim().is_empty() {
            item.insert("new".to_string(), serde_json::json!(new_values[idx]));
            changes.push(serde_json::Value::Object(item));
        }
    }

    if allow_removal {
        for idx in common..old_values.len() {
            let mut item = serde_json::Map::new();
            item.insert("index".to_string(), serde_json::json!(idx));
            item.insert("old".to_string(), serde_json::json!(old_values[idx]));
            item.insert("new".to_string(), serde_json::Value::Null);
            changes.push(serde_json::Value::Object(item));
        }
    }

    serde_json::json!(changes)
}

fn i32_list_changes(
    old_values: &[i32],
    new_values: &[i32],
    allow_removal: bool,
) -> serde_json::Value {
    let old_as_str: Vec<String> = old_values.iter().map(|v| v.to_string()).collect();
    let new_as_str: Vec<String> = new_values.iter().map(|v| v.to_string()).collect();
    string_list_changes(&old_as_str, &new_as_str, false, allow_removal, false)
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
        if let Some(change) = scalar_change(&old_layer[field], &new_layer[field]) {
            out.insert(field.to_string(), change);
        }
    }

    for field in ["toolstamps", "mould_sids", "additional_moulds"] {
        let old_csv = normalize_csv_field(old_layer[field].as_str().unwrap_or(""));
        let new_csv = normalize_csv_field(new_layer[field].as_str().unwrap_or(""));
        if let Some(change) = scalar_change(&serde_json::json!(old_csv), &serde_json::json!(new_csv)) {
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
        out.insert("removed".to_string(), serde_json::json!(true));
    }

    for (history_field, old_key, new_key) in [
        ("offset_value", "offset_value", "offset_value"),
        ("offset_extra_value", "offset_extra_value", "offset_extra_value"),
        ("sample_data_start", "sample_start", "sample_start"),
        ("comment", "comment", "comment"),
    ] {
        if let Some(change) = scalar_change(&old_entry[old_key], &new_entry[new_key]) {
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
    let mut old_by_id: std::collections::HashMap<i32, &serde_json::Value> = std::collections::HashMap::new();
    for old_entry in &old_arr {
        if let Some(id) = old_entry.get("id").and_then(|v| v.as_i64()).map(|v| v as i32) {
            old_by_id.insert(id, old_entry);
        }
    }
    let mut seen_old_ids = std::collections::HashSet::new();

    for new_entry in &new_arr {
        let maybe_id = new_entry.get("id").and_then(|v| v.as_i64()).map(|v| v as i32);
        if let Some(id) = maybe_id {
            if let Some(old_entry) = old_by_id.get(&id) {
                seen_old_ids.insert(id);
                if let Some(change) = ring_entry_change(Some(old_entry), Some(new_entry), allow_removal, Some(id)) {
                    changes.push(change);
                }
            } else if let Some(change) = ring_entry_change(None, Some(new_entry), allow_removal, None) {
                changes.push(change);
            }
        } else if let Some(change) = ring_entry_change(None, Some(new_entry), allow_removal, None) {
            changes.push(change);
        }
    }

    if allow_removal {
        for old_entry in &old_arr {
            if let Some(id) = old_entry.get("id").and_then(|v| v.as_i64()).map(|v| v as i32) {
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
    submission_type: SubmissionType,
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
    let allow_removal = submission_type == SubmissionType::Edit;

    for key in [
        "system_code", "media_type", "category", "title", "title_foreign", "disc_number",
        "disc_title", "filename_suffix", "version", "error_count", "exe_date", "edc",
        "comments", "contents", "protection", "sector_ranges", "sbi", "pvd", "header",
        "bca", "pic", "cuesheet", "enabled", "questionable",
    ] {
        let old = db_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let new = form_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let Some(change) = scalar_change(old, new) else {
            continue;
        };
        if submission_type == SubmissionType::Disc {
            let include = match key {
                "questionable" => new.as_bool().unwrap_or(false) && !old.as_bool().unwrap_or(false),
                "enabled" => new.as_bool().unwrap_or(false) && !old.as_bool().unwrap_or(false),
                _ => !is_empty_json(new),
            };
            if !include {
                continue;
            }
        }
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
            if let Some(change) = scalar_change(old, new) {
                if submission_type == SubmissionType::Edit || !is_empty_json(new) {
                    changes.insert(key.to_string(), change);
                }
            }
        }
    }

    for key in ["regions", "languages"] {
        let old = db_obj.get(key).unwrap_or(&serde_json::Value::Null);
        let new = form_obj.get(key).unwrap_or(&serde_json::Value::Null);
        if let Some(change) = csv_change(old, new) {
            if submission_type == SubmissionType::Edit || !change["new"].as_str().unwrap_or("").is_empty() {
                changes.insert(key.to_string(), change);
            }
        }
    }

    for key in ["serial", "edition", "barcode"] {
        let old_values: Vec<String> = db_obj
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let new_values: Vec<String> = match key {
            "serial" => norm_str_vec_keep_order_with_internal_blanks(form.serial.clone()),
            "edition" => norm_str_vec_keep_order_with_internal_blanks(form.edition.clone()),
            "barcode" => norm_str_vec_keep_order_with_internal_blanks(form.barcode.clone()),
            _ => Vec::new(),
        };
        let list = string_list_changes(&old_values, &new_values, true, allow_removal, true);
        if list.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            changes.insert(key.to_string(), list);
        }
    }

    {
        let old_layerbreaks: Vec<i32> = db_obj
            .get("layerbreaks")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64().map(|x| x as i32)).collect())
            .unwrap_or_default();
        let new_layerbreaks: Vec<i32> = form_obj
            .get("layerbreaks")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64().map(|x| x as i32)).collect())
            .unwrap_or_default();
        let list = i32_list_changes(&old_layerbreaks, &new_layerbreaks, allow_removal);
        if list.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            changes.insert("layerbreaks".to_string(), list);
        }
    }

    {
        let old_keys: Vec<String> = db_obj
            .get("keys")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let new_keys: Vec<String> = form_obj
            .get("keys")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let list = string_list_changes(&old_keys, &new_keys, false, allow_removal, false);
        if list.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
            changes.insert("keys".to_string(), list);
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
    build_history_changes(form, Some(detail), all_media_types, SubmissionType::Edit)
}

pub(crate) fn build_new_disc_changes(
    form: &DiscEditForm,
    all_media_types: &[EditMediaTypeRow],
) -> serde_json::Value {
    build_history_changes(form, None, all_media_types, SubmissionType::Disc)
}

fn merge_opt_str(db: &serde_json::Value, user: Option<String>) -> serde_json::Value {
    match user {
        Some(v) if !v.is_empty() => serde_json::json!(v),
        _ => db.clone(),
    }
}

fn merge_opt_json(db: &serde_json::Value, user: &serde_json::Value) -> serde_json::Value {
    if user.is_null() { db.clone() } else { user.clone() }
}

pub(crate) fn build_merged_changes(
    form: &DiscEditForm,
    detail: &DiscDetail,
    all_media_types: &[EditMediaTypeRow],
) -> serde_json::Value {
    let db = disc_service::build_snapshot_from_disc(detail);
    let user = build_flat_changes(form, all_media_types);

    let system_code = {
        let u = user["system_code"].as_str().unwrap_or("");
        if u.is_empty() { db["system_code"].clone() } else { user["system_code"].clone() }
    };
    let media_type = {
        let u = user["media_type"].as_str().unwrap_or("");
        if u.is_empty() { db["media_type"].clone() } else { user["media_type"].clone() }
    };
    let title = {
        let u = user["title"].as_str().unwrap_or("");
        if u.is_empty() { db["title"].clone() } else { user["title"].clone() }
    };
    let category = {
        let u = user["category"].as_str().unwrap_or("");
        if u.is_empty() { db["category"].clone() } else { user["category"].clone() }
    };

    let u_str = |key: &str| user[key].as_str().map(|s| s.to_string());
    let title_foreign = merge_opt_str(&db["title_foreign"], u_str("title_foreign"));
    let disc_number = merge_opt_str(&db["disc_number"], u_str("disc_number"));
    let disc_title = merge_opt_str(&db["disc_title"], u_str("disc_title"));
    let filename_suffix = merge_opt_str(&db["filename_suffix"], u_str("filename_suffix"));
    let version = merge_opt_str(&db["version"], u_str("version"));
    let exe_date = merge_opt_str(&db["exe_date"], u_str("exe_date"));
    let comments = merge_opt_str(&db["comments"], u_str("comments"));
    let contents = merge_opt_str(&db["contents"], u_str("contents"));
    let protection = merge_opt_str(&db["protection"], u_str("protection"));
    let sbi = merge_opt_str(&db["sbi"], u_str("sbi"));
    let pvd = merge_opt_str(&db["pvd"], u_str("pvd"));
    let header = merge_opt_str(&db["header"], u_str("header"));
    let bca = merge_opt_str(&db["bca"], u_str("bca"));
    let pic = merge_opt_str(&db["pic"], u_str("pic"));
    let cue = merge_opt_str(&db["cuesheet"], u_str("cuesheet"));
    let files_xml = merge_opt_str(&db["dat"], u_str("dat"));

    let error_count = merge_opt_json(&db["error_count"], &user["error_count"]);
    let edc = merge_opt_json(&db["edc"], &user["edc"]);

    let layerbreaks = {
        let u = user["layerbreaks"].as_array();
        if u.map_or(true, |a| a.is_empty()) { db["layerbreaks"].clone() } else { user["layerbreaks"].clone() }
    };
    let keys = {
        let u = user["keys"].as_array();
        if u.map_or(true, |a| a.is_empty()) { db["keys"].clone() } else { user["keys"].clone() }
    };
    let sector_ranges = {
        let u = user["sector_ranges"].as_array();
        if u.map_or(true, |a| a.is_empty()) { db["sector_ranges"].clone() } else { user["sector_ranges"].clone() }
    };

    let questionable = user["questionable"].as_bool().unwrap_or(
        db["questionable"].as_bool().unwrap_or(false)
    );
    let enabled = user["enabled"].as_bool().unwrap_or(
        db["enabled"].as_bool().unwrap_or(true)
    );

    let db_serials: Vec<String> = db["serial"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let db_editions: Vec<String> = db["edition"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let db_barcodes: Vec<String> = db["barcode"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let db_regions: Vec<String> = db["regions"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let db_languages: Vec<String> = db["languages"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();

    let serial = merge_str_vecs(&db_serials, &norm_str_vec(form.serial.clone()));
    let edition = merge_str_vecs(&db_editions, &norm_str_vec(form.edition.clone()));
    let barcode = merge_str_vecs(&db_barcodes, &norm_str_vec(form.barcode.clone()));
    let regions = merge_str_vecs(&db_regions, &norm_str_vec(form.regions.clone()));
    let languages = merge_str_vecs(&db_languages, &norm_str_vec(form.languages.clone()));

    let db_ring_codes = &db["ring_codes"];
    let user_ring_codes = &user["ring_codes"];
    let ring_codes = merge_ring_codes(db_ring_codes, user_ring_codes);

    serde_json::json!({
        "system_code": system_code,
        "media_type": media_type,
        "title": title,
        "category": category,
        "title_foreign": title_foreign,
        "disc_number": disc_number,
        "disc_title": disc_title,
        "filename_suffix": filename_suffix,
        "serial": serial,
        "version": version,
        "edition": edition,
        "barcode": barcode,
        "comments": comments,
        "contents": contents,
        "error_count": error_count,
        "exe_date": exe_date,
        "edc": edc,
        "layerbreaks": layerbreaks,
        "pvd": pvd,
        "pic": pic,
        "bca": bca,
        "header": header,
        "protection": protection,
        "sbi": sbi,
        "keys": keys,
        "cuesheet": cue,
        "dat": files_xml,
        "regions": regions,
        "languages": languages,
        "ring_codes": ring_codes,
        "sector_ranges": sector_ranges,
        "questionable": questionable,
        "enabled": enabled,
    })
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
