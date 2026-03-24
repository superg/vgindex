use askama::Template;
use axum::{
    extract::{Path, State},
    response::{Html, Redirect},
    routing::get,
    Form, Router,
};
use serde::Deserialize;

use crate::auth::middleware::RequireAuth;
use crate::db::models::*;
use crate::error::AppResult;
use crate::services::{disc_service, submission_service};
use crate::AppState;

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
    disc_title: String,
    categories: Vec<SelectOption>,
    regions: Vec<CheckOption>,
    languages: Vec<CheckOption>,
    show_barcode: bool,
    barcode: String,
    comments: String,
    show_version: bool,
    version: String,
    show_edition: bool,
    edition: String,
    exe_date: String,
    show_date_field: bool,
    edc_value: String,
    show_edc_field: bool,
    show_protection: bool,
    protection: String,
    show_error_count: bool,
    error_count: String,
    show_pvd: bool,
    pvd_hex: String,
    show_header: bool,
    header_hex: String,
}

struct SelectOption {
    id: i32,
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

async fn edit_page(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;

    let all_regions: Vec<Region> =
        sqlx::query_as("SELECT * FROM regions ORDER BY sort_order")
            .fetch_all(&state.pool).await?;
    let langs: Vec<Language> =
        sqlx::query_as("SELECT * FROM languages ORDER BY sort_order")
            .fetch_all(&state.pool).await?;

    let disc_region_codes: Vec<String> = sqlx::query_scalar(
        "SELECT region_code FROM disc_regions WHERE disc_id = $1"
    ).bind(id).fetch_all(&state.pool).await?;

    let disc_lang_codes: Vec<String> = sqlx::query_scalar(
        "SELECT language_code FROM disc_languages WHERE disc_id = $1"
    ).bind(id).fetch_all(&state.pool).await?;

    let categories: Vec<SelectOption> = Category::ALL.iter().map(|c| SelectOption {
        id: 0,
        value: c.to_string(),
        name: c.to_string(),
        selected: detail.disc.category == *c,
    }).collect();

    let regions: Vec<CheckOption> = all_regions.iter().map(|r| CheckOption {
        value: r.code.trim().to_string(),
        name: r.name.clone(),
        code: r.flag_code.trim().to_lowercase(),
        selected: disc_region_codes.iter().any(|c| c.trim() == r.code.trim()),
    }).collect();

    let languages: Vec<CheckOption> = langs.iter().map(|l| CheckOption {
        value: l.code.trim().to_string(),
        name: l.name.clone(),
        code: l.flag_code.trim().to_lowercase(),
        selected: disc_lang_codes.iter().any(|c| c.trim() == l.code.trim()),
    }).collect();

    Ok(Html(
        DiscEditTemplate {
            current_user: Some(user.username),
            disc_id: id,
            disc_title: detail.disc.title.clone(),
            categories,
            regions,
            languages,
            show_barcode: detail.system.has_barcode,
            barcode: detail.disc.barcode.join(", "),
            comments: detail.disc.comments.unwrap_or_default(),
            show_version: detail.system.has_version,
            version: detail.disc.version.unwrap_or_default(),
            show_edition: detail.system.has_edition,
            edition: detail.disc.edition.join(", "),
            exe_date: detail.disc.exe_date.map(|d| d.to_string()).unwrap_or_default(),
            show_date_field: detail.system.has_exe_date,
            edc_value: detail.disc.m2f2_edc.map(|e| e.to_string()).unwrap_or_default(),
            show_edc_field: detail.system.has_m2f2_edc,
            show_protection: detail.system.has_protection,
            protection: detail.disc.protection.unwrap_or_default(),
            show_error_count: detail.system.has_error_count,
            error_count: detail.disc.error_count.map(|e| e.to_string()).unwrap_or("0".to_string()),
            show_pvd: detail.system.has_pvd,
            pvd_hex: detail.disc.pvd.as_ref()
                .map(|data| format_pvd_hex_dump(data))
                .unwrap_or_default(),
            show_header: detail.system.has_header,
            header_hex: detail.disc.header.as_ref()
                .map(|data| format_header_hex_dump(data))
                .unwrap_or_default(),
        }
        .render()
        .unwrap(),
    ))
}

#[derive(Deserialize)]
pub struct DiscEditForm {
    pub title: String,
    pub category: String,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub barcode: Option<String>,
    pub comments: Option<String>,
    pub exe_date: Option<String>,
    pub edc: Option<String>,
    pub protection: Option<String>,
    pub error_count: Option<i32>,
    pub pvd: Option<String>,
    pub header: Option<String>,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
}

async fn edit_submit(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
    Form(form): Form<DiscEditForm>,
) -> AppResult<Redirect> {
    let data = serde_json::json!({
        "title": form.title,
        "category": form.category,
        "version": form.version,
        "edition": form.edition,
        "barcode": form.barcode,
        "comments": form.comments,
        "exe_date": form.exe_date,
        "edc": form.edc,
        "protection": form.protection,
        "error_count": form.error_count,
        "pvd": form.pvd,
        "header": form.header,
        "regions": form.regions,
        "languages": form.languages,
    });

    if user.role.can_edit_directly() {
        disc_service::update_disc(&state.pool, id, &data).await?;
    } else {
        submission_service::create_submission(
            &state.pool,
            SubmissionType::Edit,
            user.id,
            Some(id),
            data,
            None,
            None,
        ).await?;
    }

    Ok(Redirect::to(&format!("/disc/{id}/")))
}

fn format_hex_dump_edit(data: &[u8], base_addr: usize) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = base_addr + i * 16;
        out.push_str(&format!("{:04X} : ", offset));
        for (j, byte) in chunk.iter().enumerate() {
            out.push_str(&format!("{:02X} ", byte));
            if j == 7 { out.push(' '); }
        }
        for _ in chunk.len()..16 { out.push_str("   "); }
        out.push_str("  ");
        for byte in chunk {
            if byte.is_ascii_graphic() || *byte == b' ' {
                out.push(*byte as char);
            } else {
                out.push(' ');
            }
        }
        if i < data.len() / 16 {
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
