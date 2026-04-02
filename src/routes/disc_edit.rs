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
use crate::error::AppResult;
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
}

#[derive(Template)]
#[template(path = "disc_edit.html")]
struct DiscEditTemplate {
    current_user: Option<String>,
    disc_id: i32,
    page_title: String,

    systems: Vec<SystemOption>,
    media_types_all: Vec<MediaTypeOption>,
    categories: Vec<SelectOption>,
    regions: Vec<CheckOption>,
    languages: Vec<CheckOption>,

    system_code: String,
    media_type_code: String,
    max_layers: u32,
    media_layers_json: String,
    systems_media_json: String,
    systems_has_offset_extra_json: String,

    title: String,
    show_title_foreign: bool,
    title_foreign: String,
    show_disc_number: bool,
    disc_number: String,
    show_disc_title: bool,
    disc_title: String,
    filename_suffix: String,

    show_serial: bool,
    serials: Vec<String>,
    show_version: bool,
    version: String,
    show_edition: bool,
    editions: Vec<String>,
    show_barcode: bool,
    barcodes: Vec<String>,

    ring_codes_json: String,

    comments: String,
    contents: String,

    show_error_count: bool,
    error_count: String,
    show_exe_date: bool,
    exe_date: String,
    show_edc: bool,
    edc_value: String,

    layerbreaks: Vec<String>,
    show_pvd: bool,
    pvd_hex: String,
    show_pic: bool,
    pic_hex: String,
    show_bca: bool,
    bca_hex: String,
    show_header: bool,
    header_hex: String,

    show_protection: bool,
    protection: String,
    show_sector_ranges: bool,
    sector_ranges_text: String,
    show_sbi: bool,
    sbi: String,
    protection_key_disc_key: String,
    protection_key_disc_id: String,
    has_sample_start: bool,

    cue: String,
    files_xml: String,

    questionable: bool,
    enabled: bool,

    validation_errors: Vec<String>,
}
impl SiteConfig for DiscEditTemplate {}

struct SystemOption {
    code: String,
    name: String,
    selected: bool,
}

struct MediaTypeOption {
    code: String,
    name: String,
    selected: bool,
}

struct SelectOption {
    value: String,
    name: String,
    selected: bool,
}

struct CheckOption {
    value: String,
    name: String,
    code: String,
    selected: bool,
}

#[derive(sqlx::FromRow)]
struct EditMediaTypeRow {
    code: String,
    name: String,
    layer_count: i32,
}

#[derive(sqlx::FromRow)]
struct CategoryRow {
    #[allow(dead_code)]
    id: i32,
    name: String,
}

struct EditRefData {
    all_systems: Vec<System>,
    all_media_types: Vec<EditMediaTypeRow>,
    all_categories: Vec<CategoryRow>,
    all_regions: Vec<Region>,
    all_languages: Vec<Language>,
}

async fn fetch_ref_data(pool: &sqlx::PgPool) -> AppResult<EditRefData> {
    let all_systems = disc_service::get_all_systems(pool).await?;
    let all_media_types: Vec<EditMediaTypeRow> = sqlx::query_as(
        "SELECT code, name, layer_count FROM media_types ORDER BY name",
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

fn build_systems_json(
    all_systems: &[System],
) -> (String, String) {
    let mut systems_media_map = serde_json::Map::new();
    let mut systems_has_offset_extra_map = serde_json::Map::new();
    for s in all_systems {
        systems_media_map.insert(s.code.clone(), serde_json::json!(s.media_types));
        systems_has_offset_extra_map.insert(s.code.clone(), serde_json::json!(s.has_offset_extra));
    }
    let systems_media_json =
        serde_json::to_string(&systems_media_map).unwrap_or_else(|_| "{}".into());
    let systems_has_offset_extra_json =
        serde_json::to_string(&systems_has_offset_extra_map).unwrap_or_else(|_| "{}".into());
    (systems_media_json, systems_has_offset_extra_json)
}

fn build_media_layers_json(all_media_types: &[EditMediaTypeRow]) -> String {
    let mut media_layers_map = serde_json::Map::new();
    for m in all_media_types {
        media_layers_map.insert(m.code.clone(), serde_json::json!(m.layer_count));
    }
    serde_json::to_string(&media_layers_map).unwrap_or_else(|_| "{}".into())
}

fn build_system_options(all_systems: &[System], selected: &str) -> Vec<SystemOption> {
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

fn build_media_options(all_media_types: &[EditMediaTypeRow], selected: &str) -> Vec<MediaTypeOption> {
    all_media_types
        .iter()
        .map(|m| MediaTypeOption {
            code: m.code.clone(),
            name: m.name.clone(),
            selected: m.code == selected,
        })
        .collect()
}

fn build_category_options(all_categories: &[CategoryRow], selected: &str) -> Vec<SelectOption> {
    all_categories
        .iter()
        .map(|c| SelectOption {
            value: c.name.clone(),
            name: c.name.clone(),
            selected: selected == c.name,
        })
        .collect()
}

fn build_check_options(
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
        })
        .collect();
    options.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    options
}

fn build_lang_check_options(
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
        })
        .collect();
    options.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    options
}

fn max_layers_for_media(all_media_types: &[EditMediaTypeRow], code: &str) -> u32 {
    all_media_types
        .iter()
        .find(|m| m.code == code)
        .map(|m| m.layer_count as u32)
        .unwrap_or(1)
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

    let (systems_media_json, systems_has_offset_extra_json) =
        build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let max_layers = detail.disc.media_type.max_layers();

    let ring_data: Vec<serde_json::Value> = detail
        .ring_entries
        .iter()
        .map(|e| {
            let layers: Vec<serde_json::Value> = (0..max_layers)
                .map(|li| {
                    let layer = e.layers.iter().find(|l| l.layer == li as i32);
                    serde_json::json!({
                        "mastering_code": layer.and_then(|l| l.mastering_code.as_deref()).unwrap_or(""),
                        "mastering_sid": layer.and_then(|l| l.mastering_sid.as_deref()).unwrap_or(""),
                        "mould_sids": layer.map(|l| l.mould_sids.join(", ")).unwrap_or_default(),
                        "toolstamps": layer.map(|l| l.toolstamps.join(", ")).unwrap_or_default(),
                        "additional_moulds": layer.map(|l| l.additional_moulds.join(", ")).unwrap_or_default(),
                    })
                })
                .collect();
            serde_json::json!({
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
            systems_has_offset_extra_json,

            title: detail.disc.title.clone(),
            show_title_foreign: detail.system.has_title_foreign,
            title_foreign: detail.disc.title_foreign.clone().unwrap_or_default(),
            show_disc_number: detail.system.has_disc_number,
            disc_number: detail.disc.disc_number.clone().unwrap_or_default(),
            show_disc_title: detail.system.has_disc_title,
            disc_title: detail.disc.disc_title.clone().unwrap_or_default(),
            filename_suffix: detail.disc.filename_suffix.clone().unwrap_or_default(),

            show_serial: detail.system.has_serial,
            serials: {
                let mut v = detail.disc.serial.clone();
                v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
                v
            },
            show_version: detail.system.has_version,
            version: detail.disc.version.clone().unwrap_or_default(),
            show_edition: detail.system.has_edition,
            editions: {
                let mut v = detail.disc.edition.clone();
                v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
                v
            },
            show_barcode: detail.system.has_barcode,
            barcodes: {
                let mut v = detail.disc.barcode.clone();
                v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
                v
            },

            ring_codes_json,

            comments: detail.disc.comments.clone().unwrap_or_default(),
            contents: detail.disc.contents.clone().unwrap_or_default(),

            show_error_count: detail.system.has_error_count,
            error_count: detail.disc.error_count.map(|e| e.to_string()).unwrap_or_default(),
            show_exe_date: detail.system.has_exe_date,
            exe_date: detail.disc.exe_date.clone().unwrap_or_default(),
            show_edc: detail.system.has_edc,
            edc_value: detail.disc.edc.map(|e| e.to_string()).unwrap_or_default(),

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
            show_pic: detail.system.has_pic,
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

            validation_errors: vec![],
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
    pub edc: Option<String>,
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
    pub cue: Option<String>,
    pub files_xml: Option<String>,
    #[serde(default)]
    pub questionable: Option<String>,
    #[serde(default)]
    pub enabled: Option<String>,
    pub submission_comment: Option<String>,
}

fn validate_form(form: &DiscEditForm) -> Vec<String> {
    let mut errors = Vec::new();

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

    if let Some(ref text) = form.cue {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_cuesheet(text) {
                errors.push(format!("Cuesheet: {}", e));
            }
        }
    }

    if let Some(ref text) = form.files_xml {
        let text = text.trim();
        if !text.is_empty() {
            if let Err(e) = validation::validate_dat(text) {
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
) -> AppResult<Response> {
    let ref_data = fetch_ref_data(pool).await?;
    let system = disc_service::get_system(pool, &form.system_code).await.ok();

    let (systems_media_json, systems_has_offset_extra_json) =
        build_systems_json(&ref_data.all_systems);
    let media_layers_json = build_media_layers_json(&ref_data.all_media_types);
    let max_layers = max_layers_for_media(&ref_data.all_media_types, &form.media_type);

    let has_sys = |f: fn(&System) -> bool| system.as_ref().map_or(true, f);

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
        systems_has_offset_extra_json,

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
            .collect(),
        show_version: has_sys(|s| s.has_version),
        version: form.version.clone().unwrap_or_default(),
        show_edition: has_sys(|s| s.has_edition),
        editions: form
            .edition
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        show_barcode: has_sys(|s| s.has_barcode),
        barcodes: form
            .barcode
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),

        ring_codes_json: form.ring_codes_json.clone().unwrap_or_else(|| "[]".into()),

        comments: form.comments.clone().unwrap_or_default(),
        contents: form.contents.clone().unwrap_or_default(),

        show_error_count: has_sys(|s| s.has_error_count),
        error_count: form.error_count.clone().unwrap_or_default(),
        show_exe_date: has_sys(|s| s.has_exe_date),
        exe_date: form.exe_date.clone().unwrap_or_default(),
        show_edc: has_sys(|s| s.has_edc),
        edc_value: form.edc.clone().unwrap_or_default(),

        layerbreaks: form.layerbreak.clone(),
        show_pvd: has_sys(|s| s.has_pvd),
        pvd_hex: form.pvd.clone().unwrap_or_default(),
        show_pic: has_sys(|s| s.has_pic),
        pic_hex: form.pic.clone().unwrap_or_default(),
        show_bca: has_sys(|s| s.has_bca),
        bca_hex: form.bca.clone().unwrap_or_default(),
        show_header: has_sys(|s| s.has_header),
        header_hex: form.header.clone().unwrap_or_default(),

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

        questionable: form.questionable.as_deref() == Some("true"),
        enabled: form.enabled.as_deref() == Some("true"),

        validation_errors: errors,
    };

    let html = template.render().unwrap();
    Ok((StatusCode::BAD_REQUEST, Html(html)).into_response())
}

fn norm_opt_str(s: Option<&str>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn norm_str_vec(v: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = v.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    out
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

fn build_ring_codes_json_from_detail(detail: &DiscDetail) -> serde_json::Value {
    let max_layers = detail.disc.media_type.max_layers();
    let entries: Vec<serde_json::Value> = detail
        .ring_entries
        .iter()
        .map(|e| {
            let layers: Vec<serde_json::Value> = (0..max_layers)
                .map(|li| {
                    let layer = e.layers.iter().find(|l| l.layer == li as i32);
                    serde_json::json!({
                        "mastering_code": layer.and_then(|l| l.mastering_code.as_deref()).unwrap_or(""),
                        "mastering_sid": layer.and_then(|l| l.mastering_sid.as_deref()).unwrap_or(""),
                        "mould_sids": layer.map(|l| l.mould_sids.join(", ")).unwrap_or_default(),
                        "toolstamps": layer.map(|l| l.toolstamps.join(", ")).unwrap_or_default(),
                        "additional_moulds": layer.map(|l| l.additional_moulds.join(", ")).unwrap_or_default(),
                    })
                })
                .collect();
            serde_json::json!({
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
    let errors = validate_form(&form);
    if !errors.is_empty() {
        return render_form_with_errors(
            &state.pool,
            id,
            &user.username,
            &form,
            errors,
        )
        .await;
    }

    let detail = disc_service::get_disc_detail(&state.pool, id).await?;

    let old_region_codes: Vec<String> = {
        let mut v: Vec<String> = detail.regions.iter().map(|r| r.code.trim().to_string()).collect();
        v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        v
    };
    let old_lang_codes: Vec<String> = {
        let mut v: Vec<String> = detail.languages.iter().map(|l| l.code.trim().to_string()).collect();
        v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        v
    };

    let new_regions = norm_str_vec(form.regions.clone());
    let new_languages = norm_str_vec(form.languages.clone());
    let new_serials = norm_str_vec(form.serial.clone());
    let new_editions = norm_str_vec(form.edition.clone());
    let new_barcodes = norm_str_vec(form.barcode.clone());

    let old_serials = { let mut v = detail.disc.serial.clone(); v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase())); v };
    let old_editions = { let mut v = detail.disc.edition.clone(); v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase())); v };
    let old_barcodes = { let mut v = detail.disc.barcode.clone(); v.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase())); v };

    let new_error_count = form.error_count.as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| s.trim().parse::<i32>().ok());
    let new_edc = match form.edc.as_deref() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let new_layerbreaks: Vec<i32> = form.layerbreak.iter()
        .filter_map(|s| { let s = s.trim(); if s.is_empty() { None } else { s.parse::<i32>().ok() } })
        .collect();
    let old_layerbreaks: Vec<i32> = detail.disc.layerbreaks.clone().unwrap_or_default();

    let new_keys: Vec<String> = [
        form.protection_key_disc_key.as_deref().unwrap_or("").trim(),
        form.protection_key_disc_id.as_deref().unwrap_or("").trim(),
    ].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    let old_keys: Vec<String> = detail.disc.keys.clone().unwrap_or_default();

    let new_sector_ranges: Vec<serde_json::Value> =
        validation::parse_sector_range_pairs(form.sector_ranges.as_deref().unwrap_or(""))
            .into_iter()
            .map(|(start, end)| serde_json::json!({"start": start, "end": end}))
            .collect();
    let old_sector_ranges_json = build_sector_ranges_json(&detail.sector_ranges);
    let new_sector_ranges_json = serde_json::json!(new_sector_ranges);

    let new_ring_codes = form.ring_codes_json.as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .unwrap_or(serde_json::json!([]));
    let old_ring_codes = build_ring_codes_json_from_detail(&detail);

    let new_cue = norm_opt_str(form.cue.as_deref());
    let old_cue = detail.disc.cue.as_deref()
        .filter(|s| !s.is_empty())
        .map(|c| simplify_cue(c, detail.disc.media_type.rom_extension()));

    let new_files_xml = norm_opt_str(form.files_xml.as_deref());
    let old_files_xml = {
        let s = build_files_xml_from_detail(&detail);
        if s.is_empty() { None } else { Some(s) }
    };

    let new_pvd = norm_opt_str(form.pvd.as_deref());
    let old_pvd = detail.disc.pvd.as_ref().map(|data| format_pvd_hex_dump(data));
    let new_pic = norm_opt_str(form.pic.as_deref());
    let old_pic = detail.disc.pic.as_ref().map(|data| format_header_hex_dump(data));
    let new_bca = norm_opt_str(form.bca.as_deref());
    let old_bca = detail.disc.bca.as_ref().map(|data| format_header_hex_dump(data));
    let new_header = norm_opt_str(form.header.as_deref());
    let old_header = detail.disc.header.as_ref().map(|data| format_header_hex_dump(data));

    let new_questionable = form.questionable.as_deref() == Some("true");
    let new_enabled = form.enabled.as_deref() == Some("true");

    let mut changes = serde_json::Map::new();

    diff_str(&mut changes, "system_code", &detail.disc.system_code, &form.system_code);
    diff_str(&mut changes, "media_type", detail.disc.media_type.code(), &form.media_type);
    diff_str(&mut changes, "title", &detail.disc.title, form.title.trim());
    diff_str(&mut changes, "category", &detail.disc.category.to_string(), &form.category);

    diff_opt_str(&mut changes, "title_foreign",
        detail.disc.title_foreign.as_deref(),
        norm_opt_str(form.title_foreign.as_deref()).as_deref());
    diff_opt_str(&mut changes, "disc_number",
        detail.disc.disc_number.as_deref(),
        norm_opt_str(form.disc_number.as_deref()).as_deref());
    diff_opt_str(&mut changes, "disc_title",
        detail.disc.disc_title.as_deref(),
        norm_opt_str(form.disc_title.as_deref()).as_deref());
    diff_opt_str(&mut changes, "filename_suffix",
        detail.disc.filename_suffix.as_deref(),
        norm_opt_str(form.filename_suffix.as_deref()).as_deref());
    diff_opt_str(&mut changes, "version",
        detail.disc.version.as_deref(),
        norm_opt_str(form.version.as_deref()).as_deref());
    diff_opt_str(&mut changes, "exe_date",
        detail.disc.exe_date.as_deref(),
        norm_opt_str(form.exe_date.as_deref()).as_deref());
    diff_opt_str(&mut changes, "protection",
        detail.disc.protection.as_deref(),
        norm_opt_str(form.protection.as_deref()).as_deref());
    diff_opt_str(&mut changes, "sbi",
        detail.disc.sbi.as_deref(),
        norm_opt_str(form.sbi.as_deref()).as_deref());
    diff_opt_str(&mut changes, "comments",
        detail.disc.comments.as_deref(),
        norm_opt_str(form.comments.as_deref()).as_deref());
    diff_opt_str(&mut changes, "contents",
        detail.disc.contents.as_deref(),
        norm_opt_str(form.contents.as_deref()).as_deref());

    diff_opt_bool(&mut changes, "edc", detail.disc.edc, new_edc);
    diff_bool(&mut changes, "questionable", detail.disc.questionable, new_questionable);
    diff_bool(&mut changes, "enabled", detail.disc.enabled, new_enabled);
    diff_opt_i32(&mut changes, "error_count", detail.disc.error_count, new_error_count);

    diff_str_vec(&mut changes, "serial", &old_serials, &new_serials);
    diff_str_vec(&mut changes, "edition", &old_editions, &new_editions);
    diff_str_vec(&mut changes, "barcode", &old_barcodes, &new_barcodes);
    diff_str_vec(&mut changes, "regions", &old_region_codes, &new_regions);
    diff_str_vec(&mut changes, "languages", &old_lang_codes, &new_languages);
    diff_str_vec(&mut changes, "keys", &old_keys, &new_keys);
    diff_i32_vec(&mut changes, "layerbreaks", &old_layerbreaks, &new_layerbreaks);

    diff_json(&mut changes, "ring_codes", &old_ring_codes, &new_ring_codes);
    diff_json(&mut changes, "sector_ranges", &old_sector_ranges_json, &new_sector_ranges_json);

    diff_opt_str(&mut changes, "cue", old_cue.as_deref(), new_cue.as_deref());
    diff_opt_str(&mut changes, "files_xml", old_files_xml.as_deref(), new_files_xml.as_deref());
    diff_opt_str(&mut changes, "pvd", old_pvd.as_deref(), new_pvd.as_deref());
    diff_opt_str(&mut changes, "pic", old_pic.as_deref(), new_pic.as_deref());
    diff_opt_str(&mut changes, "bca", old_bca.as_deref(), new_bca.as_deref());
    diff_opt_str(&mut changes, "header", old_header.as_deref(), new_header.as_deref());

    if changes.is_empty() {
        return render_form_with_errors(
            &state.pool,
            id,
            &user.username,
            &form,
            vec!["No changes detected".into()],
        )
        .await;
    }

    let submitter_comment = form.submission_comment.as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    queue_service::create_submission(
        &state.pool,
        SubmissionType::Edit,
        user.id,
        Some(id),
        serde_json::Value::Object(changes),
        submitter_comment,
        None,
        None,
    )
    .await?;

    Ok(Redirect::to(&format!("/disc/{id}/")).into_response())
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

fn format_pvd_hex_dump(data: &[u8]) -> String {
    const PVD_FULL_SIZE: usize = 96;
    const PVD_STORED_SIZE: usize = 82;
    let mut buf = [0u8; PVD_FULL_SIZE];
    let copy_len = data.len().min(PVD_STORED_SIZE);
    buf[..copy_len].copy_from_slice(&data[..copy_len]);
    format_hex_dump_edit(&buf, 0x0320)
}

fn format_header_hex_dump(data: &[u8]) -> String {
    format_hex_dump_edit(data, 0x0000)
}
