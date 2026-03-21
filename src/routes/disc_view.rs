use askama::Template;
use axum::{
    extract::{Path, State},
    response::Html,
    routing::get,
    Router,
};

use crate::auth::middleware::CurrentUser;
use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::disc_service;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/disc/{id}/", get(disc_view))
}

#[derive(Template)]
#[template(path = "disc_view.html")]
struct DiscViewTemplate {
    current_user: Option<String>,
    can_edit: bool,
    disc_id: i32,
    title: String,
    system_name: String,
    media_type: String,
    is_cd: bool,
    category: String,
    regions: Vec<ViewFlag>,
    lang_flags: Vec<ViewFlag>,
    alt_titles: Vec<ViewKV>,
    serials: Vec<ViewKV>,
    exe_date: String,
    version: String,
    edition: String,
    barcode: String,
    comments: String,
    edc_display: String,
    protection: String,
    error_count: String,
    file_count: usize,
    status_class: String,
    status_display: String,
    created_at: String,
    updated_at: String,
    dumpers: Vec<DumperInfo>,
    ring_entries: Vec<ViewRingEntry>,
    files: Vec<ViewFile>,
    sbi_rows: Vec<SbiRow>,
    hex_pvd: String,
}

struct ViewFlag {
    flag_code_lower: String,
    name: String,
}

struct ViewKV {
    type_name: String,
    value: String,
}

struct ViewRingEntry {
    layers: Vec<ViewRingLayer>,
}

struct ViewRingLayer {
    layer: i32,
    mastering_code: String,
    mastering_sid: String,
    mould_sids: String,
    toolstamps: String,
    additional_moulds: String,
    offset_value: String,
    sample_data_start: String,
    comment: String,
}

struct ViewFile {
    track_display: String,
    size: i64,
    crc32: String,
    md5: String,
    sha1: String,
}

struct SbiRow {
    sector: u32,
    msf: String,
    contents: String,
}

async fn disc_view(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;

    if detail.disc.status == DiscStatus::Bad && !user.user().map_or(false, |u| u.role.can_edit_directly()) {
        return Err(AppError::NotFound);
    }

    let can_edit = user.user().map_or(false, |u| u.role.can_submit());
    let is_cd = detail.disc.media_type.is_cd();

    let ring_entries: Vec<ViewRingEntry> = detail.ring_entries.iter().map(|e| ViewRingEntry {
        layers: e.layers.iter().map(|l| ViewRingLayer {
            layer: l.layer,
            mastering_code: l.mastering_code.clone().unwrap_or_default(),
            mastering_sid: l.mastering_sid.clone().unwrap_or_default(),
            mould_sids: l.mould_sids.join(", "),
            toolstamps: l.toolstamps.join(", "),
            additional_moulds: l.additional_moulds.join(", "),
            offset_value: l.offset_value.clone().unwrap_or_default(),
            sample_data_start: l.sample_data_start.clone().unwrap_or_default(),
            comment: l.comment.clone().unwrap_or_default(),
        }).collect(),
    }).collect();

    let files: Vec<ViewFile> = detail.files.iter().map(|f| ViewFile {
        track_display: f.track_number.clone().unwrap_or_default(),
        size: f.size,
        crc32: f.crc32.clone(),
        md5: f.md5.clone(),
        sha1: f.sha1.clone(),
    }).collect();

    let sbi_rows = detail.disc.sbi_data.as_ref()
        .map(|data| parse_sbi_display(data))
        .unwrap_or_default();

    let hex_pvd = detail.disc.pvd_data.as_ref()
        .map(|data| format_hex_dump(data))
        .unwrap_or_default();

    Ok(Html(
        DiscViewTemplate {
            current_user: user.user().map(|u| u.username.clone()),
            can_edit,
            disc_id: id,
            title: detail.disc.title.clone(),
            system_name: detail.system.full_name.clone(),
            media_type: detail.disc.media_type.to_string(),
            is_cd,
            category: detail.disc.category.to_string(),
            regions: detail.regions.iter().map(|r| ViewFlag {
                flag_code_lower: r.flag_code.to_lowercase(),
                name: r.name.clone(),
            }).collect(),
            lang_flags: detail.languages.iter().map(|l| ViewFlag {
                flag_code_lower: l.flag_code.to_lowercase(),
                name: l.name.clone(),
            }).collect(),
            alt_titles: detail.alt_titles.iter().map(|(t, v)| ViewKV {
                type_name: t.clone(),
                value: v.clone(),
            }).collect(),
            serials: detail.serials.iter().map(|(t, v)| ViewKV {
                type_name: t.clone(),
                value: v.clone(),
            }).collect(),
            exe_date: detail.disc.exe_date.map(|d| d.to_string()).unwrap_or_default(),
            version: detail.disc.version.clone().unwrap_or_default(),
            edition: detail.disc.edition.clone().unwrap_or_default(),
            barcode: detail.disc.barcode.clone().unwrap_or_default(),
            comments: detail.disc.comments.clone().unwrap_or_default(),
            edc_display: detail.disc.edc.map(|e| if e { "Yes" } else { "No" }.to_string()).unwrap_or_default(),
            protection: detail.disc.protection.clone().unwrap_or_default(),
            error_count: detail.disc.error_count.map(|e| e.to_string()).unwrap_or_default(),
            file_count: detail.files.len(),
            status_class: detail.disc.status.css_class().to_string(),
            status_display: detail.disc.status.to_string(),
            created_at: detail.disc.created_at.format("%Y-%m-%d %H:%M").to_string(),
            updated_at: detail.disc.updated_at.format("%Y-%m-%d %H:%M").to_string(),
            dumpers: detail.dumpers,
            ring_entries,
            files,
            sbi_rows,
            hex_pvd,
        }
        .render()
        .unwrap(),
    ))
}

fn format_hex_dump(data: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = i * 16;
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
        out.push('\n');
    }
    out
}

fn parse_sbi_display(data: &[u8]) -> Vec<SbiRow> {
    let mut rows = Vec::new();
    for chunk in data.chunks_exact(15) {
        let m = chunk[0] as u32;
        let s = chunk[1] as u32;
        let f = chunk[2] as u32;
        let sector = (m * 60 + s) * 75 + f - 150;
        let msf = format!("{:02X}:{:02X}:{:02X}", chunk[0], chunk[1], chunk[2]);
        let contents: Vec<String> = chunk[3..15].iter().map(|b| format!("{:02X}", b)).collect();
        rows.push(SbiRow { sector, msf, contents: contents.join(" ") });
    }
    rows
}
