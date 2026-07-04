use askama::Template;
use axum::{
    extract::{rejection::PathRejection, Path, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};

use crate::auth::middleware::{AuthenticatedUser, CurrentUser};
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::disc_service;
use crate::AppState;

/// Sentinel `created_at` for `disc_submissions` rows that mark discs which had
/// no `added` timestamp on redump.org. The import script
/// (`scripts/generate_import_sql.py`, constant `NO_ADDED_SENTINEL_TS`) emits
/// one such row per affected disc; `MIN(created_at)` then surfaces this exact
/// value as the disc's `added_at`, which we recognize here to suppress the
/// "Added" row in the disc view.
const NO_ADDED_SENTINEL: DateTime<Utc> = DateTime::<Utc>::UNIX_EPOCH;

fn ring_tab_replace(s: &str) -> String {
    let escaped = s
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    escaped.replace(
        '\t',
        "<span class=\"ring-tab-marker\" title=\"Tab\"></span>",
    )
}

fn join_sorted_identifier_values(values: &[String]) -> String {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| {
        a.trim()
            .to_lowercase()
            .cmp(&b.trim().to_lowercase())
            .then_with(|| a.cmp(b))
    });
    sorted
        .iter()
        .map(|v| {
            v.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        })
        .collect::<Vec<String>>()
        .join("<br>")
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/disc/{id}", get(disc_view))
        .route("/disc/{id}/cue", get(disc_cue_download))
        .route("/disc/{id}/sbi", get(disc_sbi_download))
}

#[derive(Template)]
#[template(path = "disc_view.html")]
struct DiscViewTemplate {
    current_user: Option<AuthenticatedUser>,
    can_edit: bool,
    can_view_history: bool,
    disc_id: i32,
    title: String,
    system_name: String,
    system_code: String,
    media_type: String,
    category: String,
    regions: Vec<ViewFlag>,
    lang_flags: Vec<ViewFlag>,
    show_title_foreign: bool,
    title_foreign: String,
    disc_title: String,
    disc_number: String,
    serial: String,
    serial_count: usize,
    show_serial: bool,
    exe_date: String,
    show_exe_date: bool,
    version: String,
    show_version: bool,
    edition: String,
    edition_count: usize,
    show_edition: bool,
    barcode: String,
    barcode_count: usize,
    show_barcode: bool,
    layerbreaks: String,
    comments: String,
    contents: String,
    edc_display: String,
    show_edc: bool,
    protection: String,
    show_protection: bool,
    error_count: String,
    file_count: usize,
    status_class: String,
    status_display: String,
    dumper_count: usize,
    created_at: String,
    updated_at: String,
    dumpers_display: String,
    ring_rows: Vec<ViewRingRow>,
    ring_vis: RingColVis,
    show_tracks_table: bool,
    track_rows: Vec<ViewTrackTableRow>,
    track_col_vis: TrackColVis,
    files: Vec<ViewFile>,
    sbi_rows: Vec<SbiRow>,
    show_sbi: bool,
    pvd_rows: Vec<PvdRow>,
    show_pvd: bool,
    pic_rows: Vec<HeaderRow>,
    show_disc_id: bool,
    show_key: bool,
    disc_key: String,
    show_universal_hash: bool,
    universal_hash: String,
    disc_id_text: String,
    sector_ranges: Vec<ProtectionRangeRow>,
    show_sector_ranges: bool,
    header_rows: Vec<HeaderRow>,
    show_header: bool,
    bca_rows: Vec<HeaderRow>,
    show_bca: bool,
}
impl SiteConfig for DiscViewTemplate {}

struct ProtectionRangeRow {
    num: usize,
    start: i32,
    end: i32,
}

struct ViewFlag {
    code: String,
    region_code: String,
    name: String,
}

struct ViewRingRow {
    entry_num: usize,
    layer: String,
    mastering_code: String,
    mastering_sid: String,
    mould_sids: String,
    additional_moulds: String,
    toolstamps: String,
    offset: String,
    offset_extra: String,
    sample_data_start: String,
    comment: String,
    first_in_entry: bool,
    entry_even: bool,
    entry_rowspan: usize,
}

struct RingColVis {
    layer: bool,
    mastering_code: bool,
    mastering_sid: bool,
    mould_sids: bool,
    additional_moulds: bool,
    toolstamps: bool,
    offset: bool,
    offset_extra: bool,
    sample_data_start: bool,
    comment: bool,
}

fn ring_layer_label(layer_index: usize, layer_count: usize) -> String {
    if layer_index + 1 == layer_count {
        "LS".to_string()
    } else {
        format!("L{}", layer_index)
    }
}

fn can_show_disc_view_protection(
    system_code: &str,
    has_protection: bool,
    is_logged_in: bool,
) -> bool {
    has_protection && (is_logged_in || !matches!(system_code.trim(), "BD-VIDEO" | "HDDVD-VIDEO"))
}

impl RingColVis {
    fn from_rows(
        rows: &[ViewRingRow],
        is_cd: bool,
        has_sample_start: bool,
        has_offset_extra: bool,
    ) -> Self {
        Self {
            layer: rows.iter().any(|r| !r.layer.is_empty()),
            mastering_code: rows.iter().any(|r| !r.mastering_code.is_empty()),
            mastering_sid: rows.iter().any(|r| !r.mastering_sid.is_empty()),
            mould_sids: rows.iter().any(|r| !r.mould_sids.is_empty()),
            additional_moulds: rows.iter().any(|r| !r.additional_moulds.is_empty()),
            toolstamps: rows.iter().any(|r| !r.toolstamps.is_empty()),
            offset: is_cd && rows.iter().any(|r| !r.offset.is_empty()),
            offset_extra: is_cd && has_offset_extra,
            sample_data_start: is_cd
                && has_sample_start
                && rows.iter().any(|r| !r.sample_data_start.is_empty()),
            comment: rows.iter().any(|r| !r.comment.is_empty()),
        }
    }
}

fn has_ring_text(value: &str) -> bool {
    !value.trim().is_empty()
}

fn has_optional_ring_text(value: Option<&str>) -> bool {
    value.map(has_ring_text).unwrap_or(false)
}

fn ring_layer_has_data(layer: Option<&DiscRingCodeLayer>) -> bool {
    layer
        .map(|l| {
            has_optional_ring_text(l.mastering_code.as_deref())
                || has_optional_ring_text(l.mastering_sid.as_deref())
                || has_ring_text(&l.mould_sids)
                || has_ring_text(&l.additional_moulds)
                || has_ring_text(&l.toolstamps)
        })
        .unwrap_or(false)
}

fn build_ring_rows(entries: &[RingEntryView], ring_display_layers: usize) -> Vec<ViewRingRow> {
    entries
        .iter()
        .enumerate()
        .flat_map(|(i, e)| {
            let offset = format_signed_offset(e.offset_value);
            let offset_extra = format_signed_offset(e.offset_extra_value);
            let sample_data_start = format_signed_offset(e.sample_data_start);
            let comment = e.comment.clone().unwrap_or_default();
            let entry_num = i + 1;
            let entry_even = entry_num % 2 == 0;
            let visible_layers: Vec<_> = (0..ring_display_layers)
                .filter_map(|li| {
                    let layer = e.layers.iter().find(|l| l.layer == li as i32);
                    if ring_layer_has_data(layer) {
                        Some((li, layer))
                    } else {
                        None
                    }
                })
                .collect();

            if visible_layers.is_empty() {
                let has_data = !offset.is_empty()
                    || !offset_extra.is_empty()
                    || !sample_data_start.is_empty()
                    || !comment.is_empty();
                if has_data {
                    return vec![ViewRingRow {
                        entry_num,
                        layer: String::new(),
                        mastering_code: String::new(),
                        mastering_sid: String::new(),
                        mould_sids: String::new(),
                        additional_moulds: String::new(),
                        toolstamps: String::new(),
                        offset,
                        offset_extra,
                        sample_data_start,
                        comment,
                        first_in_entry: true,
                        entry_even,
                        entry_rowspan: 1,
                    }];
                }
                return vec![];
            }

            let entry_rowspan = visible_layers.len();
            visible_layers
                .into_iter()
                .enumerate()
                .map(|(row_index, (li, layer))| ViewRingRow {
                    entry_num,
                    layer: ring_layer_label(li, ring_display_layers),
                    mastering_code: ring_tab_replace(
                        &layer
                            .and_then(|l| l.mastering_code.clone())
                            .unwrap_or_default(),
                    ),
                    mastering_sid: ring_tab_replace(
                        &layer
                            .and_then(|l| l.mastering_sid.clone())
                            .unwrap_or_default(),
                    ),
                    mould_sids: ring_tab_replace(
                        &layer.map(|l| l.mould_sids.clone()).unwrap_or_default(),
                    ),
                    additional_moulds: ring_tab_replace(
                        &layer
                            .map(|l| l.additional_moulds.clone())
                            .unwrap_or_default(),
                    ),
                    toolstamps: ring_tab_replace(
                        &layer.map(|l| l.toolstamps.clone()).unwrap_or_default(),
                    ),
                    offset: offset.clone(),
                    offset_extra: offset_extra.clone(),
                    sample_data_start: sample_data_start.clone(),
                    comment: comment.clone(),
                    first_in_entry: row_index == 0,
                    entry_even,
                    entry_rowspan: if row_index == 0 { entry_rowspan } else { 0 },
                })
                .collect()
        })
        .collect()
}

struct ViewFile {
    name: String,
    file_suffix: String,
    is_cue: bool,
    is_synthetic: bool,
    track_sort_num: u32,
    size: i64,
    crc32: String,
    md5: String,
    sha1: String,
}

fn cue_name_to_img_name(cue_name: &str) -> String {
    if cue_name.len() >= 4 && cue_name[cue_name.len() - 4..].eq_ignore_ascii_case(".cue") {
        format!("{}.img", &cue_name[..cue_name.len() - 4])
    } else {
        format!("{cue_name}.img")
    }
}

fn is_numbered_bin_track_file(file: &ViewFile) -> bool {
    !file.is_cue && file.track_sort_num > 0 && file.file_suffix.eq_ignore_ascii_case(".bin")
}

fn combined_bin_track_crc32(files: &[ViewFile]) -> Option<(i64, String)> {
    let mut track_files: Vec<&ViewFile> = files
        .iter()
        .filter(|file| is_numbered_bin_track_file(file))
        .collect();
    track_files.sort_by(|a, b| {
        a.track_sort_num
            .cmp(&b.track_sort_num)
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut total_size = 0i64;
    let mut combined = crc32fast::Hasher::new();
    for file in track_files {
        let crc = u32::from_str_radix(&file.crc32, 16).ok()?;
        let size = u64::try_from(file.size).ok()?;
        total_size = total_size.checked_add(file.size)?;
        let track = crc32fast::Hasher::new_with_initial_len(crc, size);
        combined.combine(&track);
    }

    Some((total_size, format!("{:08x}", combined.finalize())))
}

fn synthetic_img_file(files: &[ViewFile]) -> Option<ViewFile> {
    let cue_file = files.iter().find(|file| file.is_cue)?;
    let (size, crc32) = combined_bin_track_crc32(files)?;

    Some(ViewFile {
        name: cue_name_to_img_name(&cue_file.name),
        file_suffix: ".img".to_string(),
        is_cue: false,
        is_synthetic: true,
        track_sort_num: u32::MAX,
        size,
        crc32,
        md5: String::new(),
        sha1: String::new(),
    })
}

fn append_synthetic_img_file(files: &mut Vec<ViewFile>) {
    if let Some(file) = synthetic_img_file(files) {
        files.push(file);
    }
}

#[derive(Default)]
struct CueTrackMeta {
    extension: Option<String>,
    track_type: String,
    pregap_frames: Option<u32>,
}

struct SbiRow {
    sector: u32,
    msf: String,
    contents: String,
    xor: String,
}

struct PvdRow {
    label: &'static str,
    hex_contents: String,
    date: String,
    time: String,
    gmt: String,
}

struct HeaderRow {
    offset: String,
    hex_contents: String,
    ascii: String,
}

struct ParsedCueTrack {
    session_num: u32,
    track_num: u32,
    track_display: String,
    track_type: String,
    flags: Vec<String>,
    index01_frames: Option<u32>,
}

struct ViewTrackTableRow {
    is_session_header: bool,
    is_total_row: bool,
    session_label: String,
    track_num: String,
    type_display: String,
    flags_display: String,
    pregap: String,
    length: String,
    sectors: String,
}

struct TrackColVis {
    track_num: bool,
    type_display: bool,
    flags: bool,
    pregap: bool,
    length: bool,
    sectors: bool,
    visible_count: usize,
}

async fn disc_view(
    State(state): State<AppState>,
    user: CurrentUser,
    id: Result<Path<i32>, PathRejection>,
) -> AppResult<Html<String>> {
    let id = crate::routes::path_i32(id)?;
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    disc_service::ensure_disc_status_visible(detail.disc.status, user.can_view_disabled_discs())?;

    let can_edit = user.user().map_or(false, |u| u.role.can_submit());
    let can_view_key_material = user.is_logged_in();

    let ring_display_layers = detail.disc.media_type.max_layers() as usize + 1;

    let mut sorted_entries = detail.ring_entries.clone();
    disc_service::sort_ring_entry_views(&mut sorted_entries, ring_display_layers);

    let ring_rows = build_ring_rows(&sorted_entries, ring_display_layers);

    let region_names: Vec<String> = detail.regions.iter().map(|r| r.name.clone()).collect();
    let language_codes: Vec<String> = detail.languages.iter().map(|l| l.code.clone()).collect();
    let rom_extension = detail.disc.media_type.rom_extension();
    let rom_base_name = build_rom_base_name(
        &detail.disc.title,
        &region_names,
        &language_codes,
        if detail.system.has_disc_number {
            detail.disc.disc_number.as_deref()
        } else {
            None
        },
        if detail.system.has_disc_title {
            detail.disc.disc_title.as_deref()
        } else {
            None
        },
        detail.disc.filename_suffix.as_deref(),
    );

    let total_tracks = detail
        .files
        .iter()
        .filter(|f| f.track_number.is_some())
        .count();
    let cue_active = detail
        .system
        .has_cue_for_media_type(&detail.disc.media_type);
    let cue_track_meta = if cue_active {
        detail
            .disc
            .cue
            .as_deref()
            .map(parse_cue_track_meta)
            .unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };

    let mut files: Vec<ViewFile> = detail
        .files
        .iter()
        .filter(|f| f.track_number.is_some() || cue_active)
        .map(|f| {
            let is_cue = f.track_number.is_none();
            let track = f.track_number.as_deref();
            let track_num = track.and_then(|t| t.parse::<u32>().ok()).unwrap_or(0);
            let cue_meta = track.and_then(|t| cue_track_meta.get(t));
            let ext = if is_cue {
                "cue"
            } else {
                cue_meta
                    .and_then(|m| m.extension.as_deref())
                    .unwrap_or(rom_extension)
            };
            ViewFile {
                name: build_rom_name(&rom_base_name, track, total_tracks, ext),
                file_suffix: if is_cue {
                    ".cue".to_string()
                } else {
                    format!(".{ext}")
                },
                is_cue,
                is_synthetic: false,
                track_sort_num: track_num,
                size: f.size,
                crc32: f.crc32.to_ascii_lowercase(),
                md5: f.md5.to_ascii_lowercase(),
                sha1: f.sha1.to_ascii_lowercase(),
            }
        })
        .collect();

    files.sort_by(|a, b| {
        a.track_sort_num
            .cmp(&b.track_sort_num)
            .then_with(|| a.file_suffix.cmp(&b.file_suffix))
            .then_with(|| a.name.cmp(&b.name))
    });
    append_synthetic_img_file(&mut files);

    let track_rows = if cue_active {
        detail
            .disc
            .cue
            .as_deref()
            .map(|cue| build_track_table_rows(cue, &detail.files))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let track_col_vis = compute_track_col_vis(&track_rows);

    let show_sbi = detail
        .system
        .has_sbi_for_media_type(&detail.disc.media_type);
    let sbi_rows = if show_sbi {
        detail
            .disc
            .sbi
            .as_deref()
            .map(|text| parse_sbi_display(text))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let pvd_rows = detail
        .disc
        .pvd
        .as_ref()
        .map(|data| parse_pvd_rows(data))
        .unwrap_or_default();

    let pic_rows = detail
        .disc
        .pic
        .as_ref()
        .map(|data| parse_header_rows(data))
        .unwrap_or_default();

    let disc_key = if can_view_key_material {
        detail
            .disc
            .disc_key
            .as_ref()
            .map(|bytes| {
                bytes
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>()
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let disc_id_text = if can_view_key_material {
        detail
            .disc
            .disc_id
            .as_deref()
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let universal_hash = detail
        .disc
        .universal_hash
        .as_ref()
        .map(|bytes| {
            bytes
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>()
        })
        .unwrap_or_default();

    let sector_ranges: Vec<ProtectionRangeRow> = detail
        .sector_ranges
        .iter()
        .enumerate()
        .map(|(i, r)| ProtectionRangeRow {
            num: i + 1,
            start: r.range_start,
            end: r.range_end,
        })
        .collect();

    let header_rows = detail
        .disc
        .header
        .as_ref()
        .map(|data| parse_header_rows(data))
        .unwrap_or_default();

    let bca_rows = detail
        .disc
        .bca
        .as_ref()
        .map(|data| parse_header_rows(data))
        .unwrap_or_default();
    let show_protection = can_show_disc_view_protection(
        &detail.system.code,
        detail.system.has_protection,
        user.is_logged_in(),
    );

    Ok(Html(
        DiscViewTemplate {
            current_user: user.user().cloned(),
            can_edit,
            can_view_history: user.is_logged_in(),
            disc_id: id,
            title: format_display_title(
                &detail.disc.title,
                if detail.system.has_disc_number {
                    detail.disc.disc_number.as_deref()
                } else {
                    None
                },
                if detail.system.has_disc_title {
                    detail.disc.disc_title.as_deref()
                } else {
                    None
                },
                detail.disc.filename_suffix.as_deref(),
            ),
            system_name: detail.system.system_name(),
            system_code: detail.system.code.clone(),
            media_type: detail.disc.media_type.to_string(),
            category: detail.disc.category.to_string(),
            regions: detail
                .regions
                .iter()
                .map(|r| ViewFlag {
                    code: r.flag_code.trim().to_lowercase(),
                    region_code: r.code.trim().to_string(),
                    name: r.name.clone(),
                })
                .collect(),
            lang_flags: detail
                .languages
                .iter()
                .map(|l| ViewFlag {
                    code: l.flag_code.trim().to_lowercase(),
                    region_code: String::new(),
                    name: l.name.clone(),
                })
                .collect(),
            show_title_foreign: detail.system.has_title_foreign,
            title_foreign: detail.disc.title_foreign.clone().unwrap_or_default(),
            disc_title: detail.disc.disc_title.clone().unwrap_or_default(),
            disc_number: detail.disc.disc_number.clone().unwrap_or_default(),
            show_serial: detail.system.has_serial,
            serial_count: detail.disc.serial.len(),
            serial: join_sorted_identifier_values(&detail.disc.serial),
            show_exe_date: detail.system.has_exe_date,
            exe_date: detail.disc.exe_date.clone().unwrap_or_default(),
            show_version: detail.system.has_version,
            version: detail.disc.version.clone().unwrap_or_default(),
            show_edition: detail.system.has_edition,
            edition_count: detail.disc.edition.len(),
            edition: join_sorted_identifier_values(&detail.disc.edition),
            show_barcode: detail.system.has_barcode,
            barcode_count: detail.disc.barcode.len(),
            barcode: join_sorted_identifier_values(&detail.disc.barcode),
            layerbreaks: detail
                .disc
                .layerbreaks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            comments: format_comments(&detail.disc.comments.clone().unwrap_or_default()),
            contents: format_comments(&detail.disc.contents.clone().unwrap_or_default()),
            edc_display: if detail.disc.edc { "Yes" } else { "No" }.to_string(),
            show_edc: detail.system.has_edc,
            protection: if show_protection {
                detail.disc.protection.clone().unwrap_or_default()
            } else {
                String::new()
            },
            show_protection,
            error_count: detail
                .disc
                .error_count
                .map(|e| e.to_string())
                .unwrap_or_default(),
            file_count: detail.files.len(),
            status_class: detail.disc.status.css_class().to_string(),
            status_display: detail.disc.status.to_string(),
            created_at: detail
                .added_at
                .filter(|d| *d != NO_ADDED_SENTINEL)
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default(),
            updated_at: detail
                .modified_at
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default(),
            dumper_count: detail.dumpers.len(),
            dumpers_display: if detail.dumpers.is_empty() {
                "Unknown".to_string()
            } else {
                detail
                    .dumpers
                    .iter()
                    .map(|d| {
                        format!(
                            "<a href=\"/discs?dumper={}\">{}</a>",
                            urlencoding::encode(&d.username),
                            d.username
                                .replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;"),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            ring_vis: RingColVis::from_rows(
                &ring_rows,
                detail.disc.media_type.is_cd(),
                detail.system.has_sample_start,
                detail.system.has_offset_extra,
            ),
            ring_rows,
            show_tracks_table: !track_rows.is_empty() && track_col_vis.visible_count > 0,
            track_rows,
            track_col_vis,
            files,
            sbi_rows,
            show_sbi,
            pvd_rows,
            show_pvd: detail.system.has_pvd,
            pic_rows,
            show_disc_id: can_view_key_material && detail.system.has_disc_id,
            show_key: can_view_key_material && detail.system.has_key,
            disc_key,
            show_universal_hash: detail.system.has_universal_hash,
            universal_hash,
            disc_id_text,
            sector_ranges,
            show_sector_ranges: detail.system.has_sector_ranges,
            header_rows,
            show_header: detail.system.has_header,
            bca_rows,
            show_bca: detail.system.has_bca,
        }
        .render()
        .unwrap(),
    ))
}

fn format_signed_offset(offset: Option<i32>) -> String {
    match offset {
        Some(v) if v > 0 => format!("+{v}"),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

fn parse_cue_track_meta(cue: &str) -> std::collections::HashMap<String, CueTrackMeta> {
    let mut tracks = std::collections::HashMap::new();
    let mut current_file_ext: Option<String> = None;
    let mut current_track: Option<String> = None;

    for raw_line in cue.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let upper = line.to_ascii_uppercase();
        if upper.starts_with("FILE \"") {
            current_file_ext = extract_file_extension_from_cue_file(line);
            continue;
        }

        if let Some((track_num, mode)) = parse_cue_track_line_local(&upper) {
            let track_key = track_num.to_string();
            let entry = tracks
                .entry(track_key.clone())
                .or_insert_with(CueTrackMeta::default);
            entry.track_type = mode.to_string();
            if entry.extension.is_none() {
                entry.extension = current_file_ext.clone();
            }
            current_track = Some(track_key);
            continue;
        }

        if let Some((idx_num, mm, ss, ff)) = parse_cue_index_line_local(&upper) {
            if idx_num == 1 {
                if let Some(track_key) = current_track.as_ref() {
                    let entry = tracks
                        .entry(track_key.clone())
                        .or_insert_with(CueTrackMeta::default);
                    let frames = mm * 60 * 75 + ss * 75 + ff;
                    if frames > 0 {
                        entry.pregap_frames = Some(frames);
                    }
                }
            }
        }
    }

    tracks
}

fn extract_file_extension_from_cue_file(line: &str) -> Option<String> {
    let rest = line.strip_prefix("FILE \"")?;
    let end_quote = rest.find('"')?;
    let filename = &rest[..end_quote];
    let (_, ext) = filename.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

fn parse_cue_track_line_local(line: &str) -> Option<(u32, &str)> {
    if !line.starts_with("TRACK ") {
        return None;
    }
    let rest = &line[6..];
    if rest.len() < 3 {
        return None;
    }
    if !rest.as_bytes()[0].is_ascii_digit()
        || !rest.as_bytes()[1].is_ascii_digit()
        || rest.as_bytes()[2] != b' '
    {
        return None;
    }
    let num: u32 = rest[..2].parse().ok()?;
    Some((num, &rest[3..]))
}

fn parse_cue_index_line_local(line: &str) -> Option<(u32, u32, u32, u32)> {
    if !line.starts_with("INDEX ") {
        return None;
    }
    let rest = &line[6..];
    if rest.len() < 3 {
        return None;
    }
    if !rest.as_bytes()[0].is_ascii_digit()
        || !rest.as_bytes()[1].is_ascii_digit()
        || rest.as_bytes()[2] != b' '
    {
        return None;
    }
    let idx_num: u32 = rest[..2].parse().ok()?;
    let time = &rest[3..];
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let mm = parts[0].parse::<u32>().ok()?;
    let ss = parts[1].parse::<u32>().ok()?;
    let ff = parts[2].parse::<u32>().ok()?;
    Some((idx_num, mm, ss, ff))
}

fn format_frames_as_msf(frames: u32) -> String {
    let minutes = frames / (75 * 60);
    let seconds = (frames % (75 * 60)) / 75;
    let frame = frames % 75;
    format!("{minutes:02}:{seconds:02}:{frame:02}")
}

fn parse_session_from_rem(line: &str) -> Option<u32> {
    let upper = line.to_ascii_uppercase();
    if upper.starts_with("REM SINGLE-DENSITY AREA") {
        return Some(1);
    }
    if upper.starts_with("REM HIGH-DENSITY AREA") {
        return Some(2);
    }
    if !upper.starts_with("REM SESSION ") {
        return None;
    }
    line[12..].trim().parse::<u32>().ok()
}

fn parse_flags_line(line: &str) -> Option<Vec<String>> {
    if !line.to_ascii_uppercase().starts_with("FLAGS ") {
        return None;
    }
    let parts: Vec<String> = line[6..]
        .split_whitespace()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    Some(parts)
}

fn parse_cue_tracks_with_sessions(cue: &str) -> Vec<ParsedCueTrack> {
    let mut tracks = Vec::new();
    let mut current_session = 1u32;
    let mut current_track_idx: Option<usize> = None;

    for raw_line in cue.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(session_num) = parse_session_from_rem(line) {
            current_session = session_num;
            continue;
        }

        let upper = line.to_ascii_uppercase();
        if let Some((track_num, mode)) = parse_cue_track_line_local(&upper) {
            tracks.push(ParsedCueTrack {
                session_num: current_session,
                track_num,
                track_display: format!("{track_num:02}"),
                track_type: mode.to_string(),
                flags: Vec::new(),
                index01_frames: None,
            });
            current_track_idx = Some(tracks.len() - 1);
            continue;
        }

        if let Some(flags) = parse_flags_line(line) {
            if let Some(idx) = current_track_idx {
                tracks[idx].flags = flags;
            }
            continue;
        }

        if let Some((idx_num, mm, ss, ff)) = parse_cue_index_line_local(&upper) {
            if idx_num == 1 {
                if let Some(idx) = current_track_idx {
                    tracks[idx].index01_frames = Some(mm * 60 * 75 + ss * 75 + ff);
                }
            }
        }
    }

    tracks
}

fn display_track_type(raw: &str) -> String {
    match raw {
        "AUDIO" => "Audio".to_string(),
        "MODE1/2352" => "Data/Mode 1".to_string(),
        "MODE2/2352" => "Data/Mode 2".to_string(),
        _ => raw.to_string(),
    }
}

fn build_track_table_rows(cue: &str, files: &[File]) -> Vec<ViewTrackTableRow> {
    let parsed_tracks = parse_cue_tracks_with_sessions(cue);
    if parsed_tracks.is_empty() {
        return Vec::new();
    }

    let mut sectors_by_track = std::collections::HashMap::<u32, u32>::new();
    for f in files {
        if let Some(track_str) = f.track_number.as_deref() {
            if let Ok(track_num) = track_str.parse::<u32>() {
                let sectors = (f.size / 2352).max(0) as u32;
                let entry = sectors_by_track.entry(track_num).or_insert(0);
                *entry += sectors;
            }
        }
    }

    let mut rows = Vec::new();
    let mut total_sectors: u64 = 0;
    let session_count = parsed_tracks
        .iter()
        .map(|t| t.session_num)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let show_sessions = session_count > 1;
    let mut last_session: Option<u32> = None;

    for track in parsed_tracks {
        if show_sessions && last_session != Some(track.session_num) {
            rows.push(ViewTrackTableRow {
                is_session_header: true,
                is_total_row: false,
                session_label: format!("Session {}", track.session_num),
                track_num: String::new(),
                type_display: String::new(),
                flags_display: String::new(),
                pregap: String::new(),
                length: String::new(),
                sectors: String::new(),
            });
            last_session = Some(track.session_num);
        }

        let sectors = sectors_by_track.get(&track.track_num).copied().unwrap_or(0);
        total_sectors += sectors as u64;
        rows.push(ViewTrackTableRow {
            is_session_header: false,
            is_total_row: false,
            session_label: String::new(),
            track_num: track.track_display,
            type_display: display_track_type(&track.track_type),
            flags_display: track.flags.join(", "),
            pregap: track
                .index01_frames
                .map(format_frames_as_msf)
                .unwrap_or_default(),
            length: format_frames_as_msf(sectors),
            sectors: sectors.to_string(),
        });
    }

    rows.push(ViewTrackTableRow {
        is_session_header: false,
        is_total_row: true,
        session_label: String::new(),
        track_num: String::new(),
        type_display: "Total".to_string(),
        flags_display: String::new(),
        pregap: String::new(),
        length: format_frames_as_msf(total_sectors as u32),
        sectors: total_sectors.to_string(),
    });

    rows
}

fn compute_track_col_vis(rows: &[ViewTrackTableRow]) -> TrackColVis {
    let non_session_rows: Vec<&ViewTrackTableRow> =
        rows.iter().filter(|r| !r.is_session_header).collect();
    let track_num = non_session_rows.iter().any(|r| !r.track_num.is_empty());
    let type_display = non_session_rows.iter().any(|r| !r.type_display.is_empty());
    let flags = non_session_rows.iter().any(|r| !r.flags_display.is_empty());
    let pregap = non_session_rows.iter().any(|r| !r.pregap.is_empty());
    let length = non_session_rows.iter().any(|r| !r.length.is_empty());
    let sectors = non_session_rows.iter().any(|r| !r.sectors.is_empty());
    let visible_count = [track_num, type_display, flags, pregap, length, sectors]
        .into_iter()
        .filter(|v| *v)
        .count();
    TrackColVis {
        track_num,
        type_display,
        flags,
        pregap,
        length,
        sectors,
        visible_count,
    }
}

fn format_comments(raw: &str) -> String {
    const TAG_MAP: &[(&str, &str)] = &[
        ("[T:VOL]", "<b>Volume Label</b>:"),
        ("[T:ISN]", "<b>Internal Serial</b>:"),
        ("[T:ISBN]", "<b>ISBN</b>:"),
        ("[T:ALT]", "<b>Alternative Title</b>:"),
        ("[T:EAID]", "<b>Electronic Arts ID</b>:"),
        ("[T:ALTF]", "<b>Alternative Foreign Title</b>:"),
        ("[T:SID]", "<b>Sega ID</b>:"),
        ("[T:JID]", "<b>JASRAC ID</b>:"),
        ("[T:ACT]", "<b>Activision ID</b>:"),
        ("[T:KID]", "<b>Konami ID</b>:"),
        ("[T:G]", "<b>Genre</b>:"),
        ("[T:BBFC]", "<b>BBFC Reg. No.</b>:"),
        ("[T:UID]", "<b>Ubisoft ID</b>:"),
        ("[T:ISSN]", "<b>ISSN</b>:"),
        ("[T:BID]", "<b>Bandai ID</b>:"),
        ("[T:DNAS]", "<b>DNAS Disc ID</b>:"),
        ("[T:S]", "<b>Series</b>:"),
        ("[T:TID]", "<b>Taito ID</b>:"),
        ("[T:KOEI]", "<b>Koei ID</b>:"),
        ("[T:LAID]", "<b>Lucas Arts ID</b>:"),
        ("[T:PT2]", "<b>Postgap type</b>:"),
        ("[T:ACC]", "<b>Acclaim ID</b>:"),
        ("[T:VFC]", "<b>VFC code</b>:"),
        ("[T:GTID]", "<b>GT Interactive ID</b>:"),
        ("[T:KIRZ]", "<b>King Records ID</b>:"),
        ("[T:PCID]", "<b>Pony Canyon ID</b>:"),
        ("[T:FIID]", "<b>Fox Interactive ID</b>:"),
        ("[T:NID]", "<b>Namco ID</b>:"),
        ("[T:VID]", "<b>Valve ID</b>:"),
        ("[T:NPS]", "<b>Nippon Ichi Software ID</b>:"),
        ("[T:OID]", "<b>Origin ID</b>:"),
        ("[T:SNID]", "<b>Selen ID</b>:"),
        ("[T:X]", "<b>Extras</b>:"),
        ("[T:NGID]", "<b>Nagano ID</b>:"),
        ("[T:PPN]", "<b>PPN</b>:"),
        ("[T:P]", "<b>Patches</b>:"),
        ("[T:PD]", "<b>Playable Demos</b>:"),
        ("[T:V]", "<b>Videos</b>:"),
        ("[T:NYG]", "<b>Net Yaroze Games</b>:"),
        ("[T:TD]", "<b>Techno Demos</b>:"),
        ("[T:UD]", "<b>Unplayable Demos</b>:"),
        ("[T:GF]", "<b>Game Footage</b>:"),
        ("[T:RD]", "<b>Rolling Demos</b>:"),
        ("[T:SG]", "<b>Savegames</b>:"),
        ("[T:VCD]", "<b>V-CD</b>"),
    ];

    let mut s = raw.to_string();
    for &(tag, replacement) in TAG_MAP {
        s = s.replace(tag, replacement);
    }

    let mut result = String::with_capacity(s.len() * 2);
    let mut i = 0;
    let bytes = s.as_bytes();

    while i < bytes.len() {
        match bytes[i] {
            b'<' => {
                if s[i..].starts_with("<xmp>") {
                    result.push_str("<xmp>");
                    i += 5;
                    if let Some(end) = s[i..].find("</xmp>") {
                        result.push_str(&s[i..i + end]);
                        result.push_str("</xmp>");
                        i += end + 6;
                    }
                } else if let Some(tag_len) = allowed_html_tag(&s[i..]) {
                    result.push_str(&s[i..i + tag_len]);
                    i += tag_len;
                } else {
                    result.push_str("&lt;");
                    i += 1;
                }
            }
            b'>' => {
                result.push_str("&gt;");
                i += 1;
            }
            b'&' => {
                result.push_str("&amp;");
                i += 1;
            }
            b'"' => {
                result.push_str("&quot;");
                i += 1;
            }
            _ => {
                let c = s[i..].chars().next().unwrap();
                result.push(c);
                i += c.len_utf8();
            }
        }
    }

    result
}

fn allowed_html_tag(s: &str) -> Option<usize> {
    const SIMPLE_TAGS: &[&str] = &[
        "<b>",
        "</b>",
        "<B>",
        "</B>",
        "<u>",
        "</u>",
        "<U>",
        "</U>",
        "<i>",
        "</i>",
        "<s>",
        "</s>",
        "<code>",
        "</code>",
        "<tt>",
        "</tt>",
        "<xmp>",
        "</xmp>",
        "<li>",
        "</li>",
        "<ul>",
        "</ul>",
        "<center>",
        "</center>",
        "<del>",
        "</del>",
        "<sup>",
        "</sup>",
        "<small>",
        "</small>",
        "<br>",
        "<br />",
        "<BR>",
        "</a>",
    ];

    for tag in SIMPLE_TAGS {
        if s.starts_with(tag) {
            return Some(tag.len());
        }
    }

    if s.starts_with("<a ") || s.starts_with("<A ") || s.starts_with("<img ") {
        return s.find('>').map(|end| end + 1);
    }

    None
}

fn parse_pvd_rows(data: &[u8]) -> Vec<PvdRow> {
    const PVD_DATE_OFFSET: usize = 13;
    const PVD_DATE_SIZE: usize = 17;
    const LABELS: [&str; 4] = ["Creation", "Modification", "Expiration", "Effective"];

    if data.len() < PVD_DATE_OFFSET + 4 * PVD_DATE_SIZE {
        return Vec::new();
    }

    LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let start = PVD_DATE_OFFSET + i * PVD_DATE_SIZE;
            let field = &data[start..start + PVD_DATE_SIZE];

            let hex_contents = field
                .iter()
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(" ");

            let ascii: String = field[..16]
                .iter()
                .map(|&b| if b.is_ascii_digit() { b as char } else { '0' })
                .collect();

            let year = &ascii[0..4];
            let month = &ascii[4..6];
            let day = &ascii[6..8];
            let hour = &ascii[8..10];
            let minute = &ascii[10..12];
            let second = &ascii[12..14];
            let centiseconds = &ascii[14..16];

            let gmt_byte = field[16] as i8;
            let gmt_minutes = gmt_byte as i32 * 15;
            let gmt_sign = if gmt_minutes >= 0 { '+' } else { '-' };
            let gmt_abs = gmt_minutes.unsigned_abs();
            let gmt = format!("{}{:02}:{:02}", gmt_sign, gmt_abs / 60, gmt_abs % 60);

            PvdRow {
                label,
                hex_contents,
                date: format!("{}-{}-{}", year, month, day),
                time: format!("{}:{}:{}.{}", hour, minute, second, centiseconds),
                gmt,
            }
        })
        .collect()
}

fn parse_header_rows(data: &[u8]) -> Vec<HeaderRow> {
    data.chunks(16)
        .enumerate()
        .map(|(i, chunk)| {
            let hex_contents = chunk
                .iter()
                .map(|b| format!("{:02X}", b))
                .collect::<Vec<_>>()
                .join(" ");
            let ascii: String = chunk
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            HeaderRow {
                offset: format!("{:04X}", i * 16),
                hex_contents,
                ascii,
            }
        })
        .collect()
}

fn format_hex_dump(data: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = i * 16;
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
        out.push('\n');
    }
    out
}

fn parse_sbi_display(text: &str) -> Vec<SbiRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msf = line
            .strip_prefix("MSF: ")
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("")
            .to_string();
        let qdata_str = line.find("Q-Data: ").map(|i| &line[i + 8..]).unwrap_or("");
        let qdata_bytes = parse_qdata_bytes(qdata_str);
        let sector = parse_msf_to_sector(&msf).unwrap_or(0);
        let contents = format_sbi_contents(sector, &qdata_bytes);
        let xor = if qdata_bytes.len() >= 12 {
            compute_sbi_xor(sector, &qdata_bytes)
        } else {
            String::new()
        };
        rows.push(SbiRow {
            sector,
            msf,
            contents,
            xor,
        });
    }
    rows
}

// parse_qdata_bytes is in crate::db::models (imported via *)

fn format_sbi_contents(sector: u32, qdata: &[u8]) -> String {
    let expected = qsector(sector);
    let mut parts = Vec::new();
    for (i, &b) in qdata.iter().enumerate() {
        let hex = format!("{:02X}", b);
        let expected_byte = if i < 10 { expected[i] } else { 0 };
        if b == expected_byte {
            parts.push(hex);
        } else {
            parts.push(format!(r#"<span style="color: #ff0000;">{}</span>"#, hex));
        }
    }
    parts.join(" ")
}

fn bcd_to_int(b: u8) -> u8 {
    (b >> 4) * 10 + (b & 0x0f)
}

fn int_to_bcd(i: u8) -> u8 {
    (i / 10) * 16 + (i % 10)
}

/// PHP-compatible itob: `(floor(i/10)*16) + (i%10)` with truncated remainder.
fn int_to_bcd_signed(i: i64) -> u8 {
    let tens = i.div_euclid(10);
    let ones = i % 10;
    (tens * 16 + ones) as u8
}

fn parse_msf_to_sector(msf: &str) -> Option<u32> {
    let parts: Vec<&str> = msf.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let m = u8::from_str_radix(parts[0], 16).ok()?;
    let s = u8::from_str_radix(parts[1], 16).ok()?;
    let f = u8::from_str_radix(parts[2], 16).ok()?;
    Some(bcd_to_int(m) as u32 * 4500 + bcd_to_int(s) as u32 * 75 + bcd_to_int(f) as u32)
}

fn qsector(sector2: u32) -> [u8; 10] {
    let sector = sector2 as i64 - 150;
    let mut arr = [0u8; 10];
    arr[0] = 0x41;
    arr[1] = 0x01;
    arr[2] = 0x01;

    let m = sector.div_euclid(4500);
    let rem = sector - m * 4500;
    let s = rem.div_euclid(75);
    let f = rem - s * 75;
    arr[3] = int_to_bcd_signed(m);
    arr[4] = int_to_bcd_signed(s);
    arr[5] = int_to_bcd_signed(f);

    arr[6] = 0x00;

    let m2 = (sector2 / 4500) as u8;
    let s2 = ((sector2 % 4500) / 75) as u8;
    let f2 = (sector2 % 75) as u8;
    arr[7] = int_to_bcd(m2);
    arr[8] = int_to_bcd(s2);
    arr[9] = int_to_bcd(f2);

    arr
}

const CRC16_GSM: crc::Crc<u16> = crc::Crc::<u16>::new(&crc::CRC_16_GSM);

fn compute_sbi_xor(sector: u32, qdata: &[u8]) -> String {
    let expected = qsector(sector);
    let crc1 = (qdata[10] as u16) << 8 | qdata[11] as u16;
    let crc2 = CRC16_GSM.checksum(&expected);
    let crc3 = CRC16_GSM.checksum(&qdata[..10]);
    format!("{:04X} {:04X}", crc1 ^ crc2, crc1 ^ crc3)
}

async fn disc_cue_download(
    State(state): State<AppState>,
    user: CurrentUser,
    id: Result<Path<i32>, PathRejection>,
) -> AppResult<impl IntoResponse> {
    let id = crate::routes::path_i32(id)?;
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    disc_service::ensure_disc_status_visible(detail.disc.status, user.can_view_disabled_discs())?;

    if !detail
        .system
        .has_cue_for_media_type(&detail.disc.media_type)
    {
        return Err(AppError::NotFound);
    }

    let cue = detail
        .disc
        .cue
        .filter(|c| !c.is_empty())
        .ok_or(AppError::NotFound)?;

    let region_names: Vec<String> = detail.regions.iter().map(|r| r.name.clone()).collect();
    let language_codes: Vec<String> = detail.languages.iter().map(|l| l.code.clone()).collect();
    let rom_base_name = build_rom_base_name(
        &detail.disc.title,
        &region_names,
        &language_codes,
        detail.disc.disc_number.as_deref(),
        detail.disc.disc_title.as_deref(),
        detail.disc.filename_suffix.as_deref(),
    );
    let filename = format!("{rom_base_name}.cue");
    let disposition = format!("attachment; filename=\"{filename}\"");

    Ok((
        [
            (
                http::header::CONTENT_TYPE,
                "application/x-cuesheet".to_string(),
            ),
            (http::header::CONTENT_DISPOSITION, disposition),
        ],
        cue,
    ))
}

async fn disc_sbi_download(
    State(state): State<AppState>,
    user: CurrentUser,
    id: Result<Path<i32>, PathRejection>,
) -> AppResult<impl IntoResponse> {
    let id = crate::routes::path_i32(id)?;
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    disc_service::ensure_disc_status_visible(detail.disc.status, user.can_view_disabled_discs())?;

    if !detail
        .system
        .has_sbi_for_media_type(&detail.disc.media_type)
    {
        return Err(AppError::NotFound);
    }

    let sbi_text = detail
        .disc
        .sbi
        .filter(|s| !s.is_empty())
        .ok_or(AppError::NotFound)?;

    let buf = build_sbi_binary(&sbi_text);

    let region_names: Vec<String> = detail.regions.iter().map(|r| r.name.clone()).collect();
    let language_codes: Vec<String> = detail.languages.iter().map(|l| l.code.clone()).collect();
    let rom_base_name = build_rom_base_name(
        &detail.disc.title,
        &region_names,
        &language_codes,
        detail.disc.disc_number.as_deref(),
        detail.disc.disc_title.as_deref(),
        detail.disc.filename_suffix.as_deref(),
    );
    let filename = format!("{rom_base_name}.sbi");
    let disposition = format!("attachment; filename=\"{filename}\"");

    Ok((
        [
            (
                http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
            (http::header::CONTENT_DISPOSITION, disposition),
        ],
        buf,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

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

    fn view_file(
        name: &str,
        file_suffix: &str,
        is_cue: bool,
        track_sort_num: u32,
        data: &[u8],
    ) -> ViewFile {
        ViewFile {
            name: name.to_string(),
            file_suffix: file_suffix.to_string(),
            is_cue,
            is_synthetic: false,
            track_sort_num,
            size: data.len() as i64,
            crc32: format!("{:08x}", crc32fast::hash(data)),
            md5: "md5".to_string(),
            sha1: "sha1".to_string(),
        }
    }

    fn ring_layer(layer: i32, mastering_code: &str) -> DiscRingCodeLayer {
        DiscRingCodeLayer {
            id: layer + 1,
            entry_id: 1,
            layer,
            mastering_code: if mastering_code.is_empty() {
                None
            } else {
                Some(mastering_code.to_string())
            },
            mastering_sid: None,
            mould_sids: String::new(),
            toolstamps: String::new(),
            additional_moulds: String::new(),
        }
    }

    fn ring_entry(
        offset_value: Option<i32>,
        comment: Option<&str>,
        layers: Vec<DiscRingCodeLayer>,
    ) -> RingEntryView {
        RingEntryView {
            id: 1,
            offset_value,
            offset_extra_value: None,
            sample_data_start: None,
            comment: comment.map(str::to_string),
            layers,
        }
    }

    #[tokio::test]
    async fn invalid_disc_id_path_returns_not_found() {
        for uri in [
            "/disc/asdf.log",
            "/disc/asdf.log/",
            "/disc/asdf.log/cue",
            "/disc/asdf.log/sbi",
        ] {
            let app = routes().with_state(test_state());
            let response = app
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        }
    }

    fn disc_view_template(
        show_disc_id: bool,
        show_key: bool,
        disc_id_text: &str,
        disc_key: &str,
    ) -> DiscViewTemplate {
        DiscViewTemplate {
            current_user: None,
            can_edit: false,
            can_view_history: false,
            disc_id: 1,
            title: "Example Disc".to_string(),
            system_name: "Sony - PlayStation 3".to_string(),
            system_code: "PS3".to_string(),
            media_type: "Blu-ray Disc".to_string(),
            category: "Games".to_string(),
            regions: Vec::new(),
            lang_flags: Vec::new(),
            show_title_foreign: false,
            title_foreign: String::new(),
            disc_title: String::new(),
            disc_number: String::new(),
            serial: String::new(),
            serial_count: 0,
            show_serial: false,
            exe_date: String::new(),
            show_exe_date: false,
            version: String::new(),
            show_version: false,
            edition: String::new(),
            edition_count: 0,
            show_edition: false,
            barcode: String::new(),
            barcode_count: 0,
            show_barcode: false,
            layerbreaks: String::new(),
            comments: String::new(),
            contents: String::new(),
            edc_display: String::new(),
            show_edc: false,
            protection: String::new(),
            show_protection: false,
            error_count: String::new(),
            file_count: 0,
            status_class: "verified".to_string(),
            status_display: "Verified".to_string(),
            dumper_count: 0,
            created_at: String::new(),
            updated_at: String::new(),
            dumpers_display: "Unknown".to_string(),
            ring_rows: Vec::new(),
            ring_vis: RingColVis {
                layer: false,
                mastering_code: false,
                mastering_sid: false,
                mould_sids: false,
                additional_moulds: false,
                toolstamps: false,
                offset: false,
                offset_extra: false,
                sample_data_start: false,
                comment: false,
            },
            show_tracks_table: false,
            track_rows: Vec::new(),
            track_col_vis: TrackColVis {
                track_num: false,
                type_display: false,
                flags: false,
                pregap: false,
                length: false,
                sectors: false,
                visible_count: 0,
            },
            files: Vec::new(),
            sbi_rows: Vec::new(),
            show_sbi: false,
            pvd_rows: Vec::new(),
            show_pvd: false,
            pic_rows: Vec::new(),
            show_disc_id,
            show_key,
            disc_key: disc_key.to_string(),
            show_universal_hash: false,
            universal_hash: String::new(),
            disc_id_text: disc_id_text.to_string(),
            sector_ranges: Vec::new(),
            show_sector_ranges: false,
            header_rows: Vec::new(),
            show_header: false,
            bca_rows: Vec::new(),
            show_bca: false,
        }
    }

    #[test]
    fn disc_identifier_fields_render_sorted_for_display() {
        let mut template = disc_view_template(false, false, "", "");
        template.show_serial = true;
        template.serial_count = 4;
        template.serial = join_sorted_identifier_values(&[
            "beta-002".to_string(),
            "abc".to_string(),
            "ABC".to_string(),
            "Alpha-001".to_string(),
        ]);
        template.show_edition = true;
        template.edition_count = 3;
        template.edition = join_sorted_identifier_values(&[
            "Limited".to_string(),
            "original".to_string(),
            "Original".to_string(),
        ]);
        template.show_barcode = true;
        template.barcode_count = 3;
        template.barcode = join_sorted_identifier_values(&[
            "9 999999 999999".to_string(),
            "0 123456 789012".to_string(),
            "0 123456 123456".to_string(),
        ]);

        let html = template.render().unwrap();

        assert!(html.contains("<strong>Disc Serials</strong>"));
        assert!(html.contains("ABC<br>abc<br>Alpha-001<br>beta-002"));
        assert!(html.contains("<strong>Editions</strong>"));
        assert!(html.contains("Limited<br>Original<br>original"));
        assert!(html.contains("<strong>Barcodes</strong>"));
        assert!(html.contains("0 123456 123456<br>0 123456 789012<br>9 999999 999999"));
    }

    #[test]
    fn disc_view_hides_history_link_when_user_cannot_view_history() {
        let html = disc_view_template(false, false, "", "").render().unwrap();

        assert!(!html.contains("History"));
        assert!(!html.contains("/queue?disc_id=1"));
    }

    #[test]
    fn disc_view_shows_history_link_when_user_can_view_history() {
        let mut template = disc_view_template(false, false, "", "");
        template.current_user = Some(AuthenticatedUser::template_only("user"));
        template.can_view_history = true;

        let html = template.render().unwrap();

        assert!(html.contains(r#"<a href="/queue?disc_id=1" role="button">History</a>"#));
    }

    #[test]
    fn ring_layer_label_marks_final_layer_as_label_side() {
        assert_eq!(ring_layer_label(0, 2), "L0");
        assert_eq!(ring_layer_label(1, 2), "LS");
        assert_eq!(ring_layer_label(2, 4), "L2");
        assert_eq!(ring_layer_label(3, 4), "LS");
    }

    #[test]
    fn video_protection_is_hidden_from_disc_view_guests() {
        assert!(!can_show_disc_view_protection("BD-VIDEO", true, false));
        assert!(!can_show_disc_view_protection("HDDVD-VIDEO", true, false));
        assert!(can_show_disc_view_protection("BD-VIDEO", true, true));
        assert!(can_show_disc_view_protection("HDDVD-VIDEO", true, true));
        assert!(can_show_disc_view_protection("DVD-VIDEO", true, false));
        assert!(!can_show_disc_view_protection("BD-VIDEO", false, true));
        assert!(!can_show_disc_view_protection("HDDVD-VIDEO", false, true));
    }

    #[test]
    fn disc_view_template_omits_hidden_protection_text() {
        let mut template = disc_view_template(false, false, "", "");
        template.protection = "Sensitive BD protection".to_string();
        template.show_protection = can_show_disc_view_protection("BD-VIDEO", true, false);

        let html = template.render().unwrap();

        assert!(!html.contains("Protection"));
        assert!(!html.contains("Sensitive BD protection"));
    }

    #[test]
    fn disc_view_template_shows_visible_protection_text() {
        let mut template = disc_view_template(false, false, "", "");
        template.protection = "Visible protection".to_string();
        template.show_protection = can_show_disc_view_protection("BD-VIDEO", true, true);

        let html = template.render().unwrap();

        assert!(html.contains("Protection"));
        assert!(html.contains("Visible protection"));
    }

    #[test]
    fn ring_rows_hide_empty_layers_and_shrink_rowspan() {
        let mut entry = ring_entry(
            Some(123),
            None,
            vec![ring_layer(0, "MASTER-L0"), ring_layer(1, "")],
        );
        entry.offset_extra_value = Some(4);
        entry.sample_data_start = Some(5678);
        let entries = vec![entry];

        let rows = build_ring_rows(&entries, 2);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].layer, "L0");
        assert_eq!(rows[0].mastering_code, "MASTER-L0");
        assert_eq!(rows[0].offset, "+123");
        assert_eq!(rows[0].offset_extra, "+4");
        assert_eq!(rows[0].sample_data_start, "+5678");
        assert_eq!(rows[0].entry_rowspan, 1);
    }

    #[test]
    fn ring_rows_keep_entry_level_data_without_layer_rows() {
        let entries = vec![ring_entry(Some(-12), Some("Offset-only entry"), Vec::new())];

        let rows = build_ring_rows(&entries, 2);

        assert_eq!(rows.len(), 1);
        assert!(rows[0].layer.is_empty());
        assert_eq!(rows[0].offset, "-12");
        assert_eq!(rows[0].comment, "Offset-only entry");
        assert_eq!(rows[0].entry_rowspan, 1);
    }

    #[test]
    fn ring_table_cells_use_middle_alignment() {
        let css = include_str!("../../static/css/app.css");

        // All ring cells share middle alignment so single-row entries line up.
        assert!(css.contains(
            ".disc-view .detail-section .ring-table tbody td {\n    vertical-align: middle !important;\n}"
        ));
        assert!(css.contains(".ring-table .entry-num {\n    font-weight: 600;\n}"));
    }

    #[test]
    fn ring_entry_level_cells_are_marked_for_middle_alignment() {
        let template = include_str!("../../templates/disc_view.html");

        assert!(template.contains(r#"<td class="entry-num ring-entry-cell""#));
        assert!(template.contains(r#"<td class="ring-fixed-cell ring-entry-cell""#));
        assert!(template.contains(r#"<td class="ring-entry-cell""#));
        assert!(template
            .contains(r#"<td class="ring-layer-col"><strong>{{ row.layer }}</strong></td>"#));
        assert!(
            template.contains(r#"<td class="ring-fixed-cell">{{ row.mastering_code|safe }}</td>"#)
        );
        assert!(!template.contains(r#"<td class="ring-layer-col ring-entry-cell""#));
        assert!(!template.contains(
            r#"<td class="ring-fixed-cell ring-entry-cell">{{ row.mastering_code|safe }}</td>"#
        ));
    }

    #[test]
    fn ring_entry_offsets_render_once_per_entry_with_rowspan() {
        let mut template = disc_view_template(false, false, "", "");
        template.ring_rows = vec![
            ViewRingRow {
                entry_num: 1,
                layer: "L0".to_string(),
                mastering_code: "MASTER-L0".to_string(),
                mastering_sid: String::new(),
                mould_sids: String::new(),
                additional_moulds: String::new(),
                toolstamps: String::new(),
                offset: "+123".to_string(),
                offset_extra: "+4".to_string(),
                sample_data_start: "+5678".to_string(),
                comment: "Entry comment".to_string(),
                first_in_entry: true,
                entry_even: false,
                entry_rowspan: 2,
            },
            ViewRingRow {
                entry_num: 1,
                layer: "LS".to_string(),
                mastering_code: "MASTER-LS".to_string(),
                mastering_sid: String::new(),
                mould_sids: String::new(),
                additional_moulds: String::new(),
                toolstamps: String::new(),
                offset: "+123".to_string(),
                offset_extra: "+4".to_string(),
                sample_data_start: "+5678".to_string(),
                comment: "Entry comment".to_string(),
                first_in_entry: false,
                entry_even: false,
                entry_rowspan: 0,
            },
        ];
        template.ring_vis = RingColVis {
            layer: true,
            mastering_code: true,
            mastering_sid: false,
            mould_sids: false,
            additional_moulds: false,
            toolstamps: false,
            offset: true,
            offset_extra: true,
            sample_data_start: true,
            comment: true,
        };

        let html = template.render().unwrap();

        assert!(
            html.contains(r#"<td class="ring-fixed-cell ring-entry-cell" rowspan="2">+123</td>"#)
        );
        assert!(html.contains(r#"<td class="ring-fixed-cell ring-entry-cell" rowspan="2">+4</td>"#));
        assert!(html.contains(r#"<td class="ring-entry-cell" rowspan="2">+5678</td>"#));
        assert!(html.contains(r#"<td class="ring-entry-cell" rowspan="2">Entry comment</td>"#));
        assert_eq!(html.matches("+123").count(), 1);
        assert_eq!(html.matches("+4").count(), 1);
        assert_eq!(html.matches("+5678").count(), 1);
        assert_eq!(html.matches("Entry comment").count(), 1);
    }

    #[test]
    fn disc_identifiers_are_not_collapsed_in_disc_section() {
        let template = include_str!("../../templates/disc_view.html");
        let css = include_str!("../../static/css/app.css");

        assert!(template.contains("<td>{{ serial|safe }}</td>"));
        assert!(template.contains(r#"<td class="disc-edition-cell"><span class="disc-edition-value">{{ edition|safe }}</span></td>"#));
        assert!(template.contains("<td>{{ barcode|safe }}</td>"));
        assert!(!template.contains("serial_count > 6"));
        assert!(!template.contains("edition_count > 6"));
        assert!(!template.contains("barcode_count > 6"));
        assert!(!template.contains("td-collapse"));
        assert!(!css.contains(".td-collapse"));
    }

    #[test]
    fn disc_view_hides_disc_id_and_key_when_visibility_flags_are_false() {
        let html = disc_view_template(false, false, "secret-disc-id", "deadbeef")
            .render()
            .unwrap();

        assert!(!html.contains("<strong>Disc ID</strong>"));
        assert!(!html.contains("<strong>Disc Key</strong>"));
        assert!(!html.contains("secret-disc-id"));
        assert!(!html.contains("deadbeef"));
    }

    #[test]
    fn disc_view_shows_disc_id_and_key_when_visibility_flags_are_true() {
        let html = disc_view_template(true, true, "visible-disc-id", "deadbeef")
            .render()
            .unwrap();

        assert!(html.contains("<strong>Disc ID</strong>"));
        assert!(html.contains("<strong>Disc Key</strong>"));
        assert!(html.contains("visible-disc-id"));
        assert!(html.contains("deadbeef"));
    }

    #[test]
    fn disc_view_hides_empty_universal_hash() {
        let mut template = disc_view_template(false, false, "", "");
        template.show_universal_hash = true;

        let html = template.render().unwrap();

        assert!(!html.contains("<strong>Universal Hash</strong>"));
    }

    #[test]
    fn disc_view_shows_populated_universal_hash() {
        let mut template = disc_view_template(false, false, "", "");
        template.show_universal_hash = true;
        template.universal_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();

        let html = template.render().unwrap();

        assert!(html.contains("<strong>Universal Hash</strong>"));
        assert!(html.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn tracks_table_sectors_use_fixed_font_like_length() {
        let mut template = disc_view_template(false, false, "", "");
        template.show_tracks_table = true;
        template.track_col_vis = TrackColVis {
            track_num: false,
            type_display: true,
            flags: false,
            pregap: false,
            length: true,
            sectors: true,
            visible_count: 3,
        };
        template.track_rows = vec![
            ViewTrackTableRow {
                is_session_header: false,
                is_total_row: false,
                session_label: String::new(),
                track_num: String::new(),
                type_display: "Data".to_string(),
                flags_display: String::new(),
                pregap: String::new(),
                length: "00:02:00".to_string(),
                sectors: "12345".to_string(),
            },
            ViewTrackTableRow {
                is_session_header: false,
                is_total_row: true,
                session_label: String::new(),
                track_num: String::new(),
                type_display: "Total".to_string(),
                flags_display: String::new(),
                pregap: String::new(),
                length: "00:04:00".to_string(),
                sectors: "67890".to_string(),
            },
        ];

        let html = template.render().unwrap();

        assert!(html.contains(r#"<td class="col-num"><code>00:02:00</code></td>"#));
        assert!(html.contains(r#"<td class="col-num"><code>12345</code></td>"#));
        assert!(html.contains(r#"<td class="col-num"><strong><code>00:04:00</code></strong></td>"#));
        assert!(html.contains(r#"<td class="col-num"><strong><code>67890</code></strong></td>"#));
        assert!(!html.contains(r#"<td class="col-num">12345</td>"#));
    }

    #[test]
    fn combined_bin_track_crc32_matches_hashing_concatenated_bytes() {
        let track1 = b"first track bytes";
        let track2 = b"second track bytes";
        let track3 = b"third track bytes";
        let files = vec![
            view_file("Game (Track 3).bin", ".bin", false, 3, track3),
            view_file("Game (Track 1).bin", ".bin", false, 1, track1),
            view_file("Game (Track 2).bin", ".bin", false, 2, track2),
        ];

        let (size, crc32) = combined_bin_track_crc32(&files).unwrap();
        let expected_bytes = [track1.as_slice(), track2.as_slice(), track3.as_slice()].concat();

        assert_eq!(size, expected_bytes.len() as i64);
        assert_eq!(crc32, format!("{:08x}", crc32fast::hash(&expected_bytes)));
    }

    #[test]
    fn synthetic_img_file_is_appended_after_all_files_when_cue_exists() {
        let track0 = b"whole disc row excluded";
        let track1 = b"track one";
        let track2 = b"track two";
        let wav_track = b"wav track excluded";
        let mut files = vec![
            view_file("Game (Track 0).bin", ".bin", false, 0, track0),
            view_file("Game.cue", ".cue", true, 0, b"cue"),
            view_file("Game (Track 1).bin", ".bin", false, 1, track1),
            view_file("Game (Track 2).bin", ".bin", false, 2, track2),
            view_file("Game (Track 3).wav", ".wav", false, 3, wav_track),
        ];

        append_synthetic_img_file(&mut files);

        let img = files.last().unwrap();
        let expected_bytes = [track1.as_slice(), track2.as_slice()].concat();
        assert_eq!(img.name, "Game.img");
        assert_eq!(img.file_suffix, ".img");
        assert!(!img.is_cue);
        assert!(img.is_synthetic);
        assert_eq!(img.size, expected_bytes.len() as i64);
        assert_eq!(
            img.crc32,
            format!("{:08x}", crc32fast::hash(&expected_bytes))
        );
        assert!(img.md5.is_empty());
        assert!(img.sha1.is_empty());
    }

    #[test]
    fn synthetic_img_file_is_not_added_without_cue() {
        let mut files = vec![view_file("Game.bin", ".bin", false, 1, b"track")];

        append_synthetic_img_file(&mut files);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "Game.bin");
    }
}
