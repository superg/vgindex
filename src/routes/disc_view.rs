use askama::Template;
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};

use crate::auth::middleware::CurrentUser;
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::AppResult;
use crate::services::disc_service;
use crate::AppState;

fn ring_tab_replace(s: &str) -> String {
    let escaped = s
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    escaped.replace('\t', "<span class=\"ring-tab-marker\" title=\"Tab\"></span>")
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/disc/{id}", get(disc_view))
        .route("/disc/{id}/", get(disc_view))
        .route("/disc/{id}/cue", get(disc_cue_download))
        .route("/disc/{id}/sbi", get(disc_sbi_download))
}

#[derive(Template)]
#[template(path = "disc_view.html")]
struct DiscViewTemplate {
    current_user: Option<String>,
    can_edit: bool,
    disc_id: i32,
    title: String,
    system_name: String,
    system_code: String,
    media_type: String,
    category: String,
    regions: Vec<ViewFlag>,
    lang_flags: Vec<ViewFlag>,
    title_foreign: String,
    disc_title: String,
    disc_number: String,
    serial: String,
    serial_count: usize,
    exe_date: String,
    version: String,
    edition: String,
    edition_count: usize,
    barcode: String,
    barcode_count: usize,
    layerbreaks: String,
    comments: String,
    contents: String,
    edc_display: String,
    protection: String,
    error_count: String,
    file_count: usize,
    status_class: String,
    status_emoji: String,
    status_display: String,
    dumper_count: usize,
    created_at: String,
    updated_at: String,
    dumpers_display: String,
    ring_rows: Vec<ViewRingRow>,
    ring_vis: RingColVis,
    files: Vec<ViewFile>,
    sbi_rows: Vec<SbiRow>,
    pvd_rows: Vec<PvdRow>,
    pic_rows: Vec<HeaderRow>,
    show_keys: bool,
    disc_key: String,
    disc_id_hex: String,
    sector_ranges: Vec<ProtectionRangeRow>,
    header_rows: Vec<HeaderRow>,
    bca_rows: Vec<HeaderRow>,
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

impl RingColVis {
    fn from_rows(rows: &[ViewRingRow], has_sample_start: bool, has_offset_extra: bool) -> Self {
        Self {
            layer: rows.iter().any(|r| r.entry_rowspan > 1),
            mastering_code: rows.iter().any(|r| !r.mastering_code.is_empty()),
            mastering_sid: rows.iter().any(|r| !r.mastering_sid.is_empty()),
            mould_sids: rows.iter().any(|r| !r.mould_sids.is_empty()),
            additional_moulds: rows.iter().any(|r| !r.additional_moulds.is_empty()),
            toolstamps: rows.iter().any(|r| !r.toolstamps.is_empty()),
            offset: rows.iter().any(|r| !r.offset.is_empty()),
            offset_extra: has_offset_extra,
            sample_data_start: has_sample_start && rows.iter().any(|r| !r.sample_data_start.is_empty()),
            comment: rows.iter().any(|r| !r.comment.is_empty()),
        }
    }
}

struct ViewFile {
    name: String,
    is_cue: bool,
    size: i64,
    crc32: String,
    md5: String,
    sha1: String,
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

async fn disc_view(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;

    let can_edit = user.user().map_or(false, |u| u.role.can_submit());

    let mut sorted_entries = detail.ring_entries.clone();
    sorted_entries.sort_by(|a, b| {
        let max_layers = a.layers.len().max(b.layers.len());
        for i in 0..max_layers {
            let al = a.layers.get(i);
            let bl = b.layers.get(i);
            let a_mc = al.and_then(|l| l.mastering_code.as_deref()).unwrap_or("");
            let b_mc = bl.and_then(|l| l.mastering_code.as_deref()).unwrap_or("");
            match a_mc.cmp(b_mc) {
                std::cmp::Ordering::Equal => {}
                ord => return ord,
            }
            let a_ms = al.and_then(|l| l.mastering_sid.as_deref()).unwrap_or("");
            let b_ms = bl.and_then(|l| l.mastering_sid.as_deref()).unwrap_or("");
            match a_ms.cmp(b_ms) {
                std::cmp::Ordering::Equal => {}
                ord => return ord,
            }
        }
        std::cmp::Ordering::Equal
    });

    let ring_display_layers = detail.disc.media_type.max_layers().max(2) as usize;

    let ring_rows: Vec<ViewRingRow> = sorted_entries.iter().enumerate().flat_map(|(i, e)| {
        let offset = format_signed_offset(e.offset_value);
        let offset_extra = format_signed_offset(e.offset_extra_value);
        let sample_data_start = e.sample_data_start.map(|v| v.to_string()).unwrap_or_default();
        let comment = e.comment.clone().unwrap_or_default();
        let entry_num = i + 1;
        let entry_even = entry_num % 2 == 0;

        if e.layers.is_empty() {
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

        let display_count = ring_display_layers;
        (0..display_count).map(move |li| {
            let layer = e.layers.iter().find(|l| l.layer == li as i32);
            ViewRingRow {
                entry_num,
                layer: format!("L{}", li),
                mastering_code: ring_tab_replace(&layer.and_then(|l| l.mastering_code.clone()).unwrap_or_default()),
                mastering_sid: ring_tab_replace(&layer.and_then(|l| l.mastering_sid.clone()).unwrap_or_default()),
                mould_sids: { let mut v = layer.map(|l| l.mould_sids.clone()).unwrap_or_default(); v.sort_unstable(); ring_tab_replace(&v.join(", ")) },
                additional_moulds: { let mut v = layer.map(|l| l.additional_moulds.clone()).unwrap_or_default(); v.sort_unstable(); ring_tab_replace(&v.join(", ")) },
                toolstamps: { let mut v = layer.map(|l| l.toolstamps.clone()).unwrap_or_default(); v.sort_unstable(); ring_tab_replace(&v.join(", ")) },
                offset: offset.clone(),
                offset_extra: offset_extra.clone(),
                sample_data_start: sample_data_start.clone(),
                comment: comment.clone(),
                first_in_entry: li == 0,
                entry_even,
                entry_rowspan: if li == 0 { display_count } else { 0 },
            }
        }).collect()
    }).collect();

    let region_names: Vec<String> = detail.regions.iter().map(|r| r.name.clone()).collect();
    let language_codes: Vec<String> = detail.languages.iter().map(|l| l.code.clone()).collect();
    let rom_extension = detail.disc.media_type.rom_extension();
    let rom_base_name = build_rom_base_name(
        &detail.disc.title,
        &region_names,
        &language_codes,
        detail.disc.disc_number.as_deref(),
        detail.disc.disc_title.as_deref(),
        detail.disc.filename_suffix.as_deref(),
    );

    let total_tracks = detail.files.iter().filter(|f| f.track_number.is_some()).count();

    let files: Vec<ViewFile> = detail.files.iter().map(|f| {
        let is_cue = f.track_number.is_none();
        let (ext, track) = if is_cue {
            ("cue", None)
        } else {
            (rom_extension, f.track_number.as_deref())
        };
        ViewFile {
            name: build_rom_name(&rom_base_name, track, total_tracks, ext),
            is_cue,
            size: f.size,
            crc32: f.crc32.clone(),
            md5: f.md5.clone(),
            sha1: f.sha1.clone(),
        }
    }).collect();

    let sbi_rows = detail.disc.sbi.as_deref()
        .map(|text| parse_sbi_display(text))
        .unwrap_or_default();

    let pvd_rows = detail.disc.pvd.as_ref()
        .map(|data| parse_pvd_rows(data))
        .unwrap_or_default();

    let pic_rows = detail.disc.pic.as_ref()
        .map(|data| parse_header_rows(data))
        .unwrap_or_default();

    let keys = detail.disc.keys.as_deref().unwrap_or_default();
    let disc_key = keys.first().cloned().unwrap_or_default();
    let disc_id_hex = keys.get(1).cloned().unwrap_or_default();

    let sector_ranges: Vec<ProtectionRangeRow> = detail.sector_ranges.iter()
        .enumerate()
        .map(|(i, r)| ProtectionRangeRow { num: i + 1, start: r.range_start, end: r.range_end })
        .collect();

    let header_rows = detail.disc.header.as_ref()
        .map(|data| parse_header_rows(data))
        .unwrap_or_default();

    let bca_rows = detail.disc.bca.as_ref()
        .map(|data| parse_header_rows(data))
        .unwrap_or_default();

    Ok(Html(
        DiscViewTemplate {
            current_user: user.user().map(|u| u.username.clone()),
            can_edit,
            disc_id: id,
            title: format_display_title(
                &detail.disc.title,
                detail.disc.disc_number.as_deref(),
                detail.disc.disc_title.as_deref(),
                detail.disc.filename_suffix.as_deref(),
            ),
            system_name: detail.system.name.clone(),
            system_code: detail.system.code.clone(),
            media_type: detail.disc.media_type.to_string(),
            category: detail.disc.category.to_string(),
            regions: detail.regions.iter().map(|r| ViewFlag {
                code: r.flag_code.trim().to_lowercase(),
                region_code: r.code.trim().to_string(),
                name: r.name.clone(),
            }).collect(),
            lang_flags: detail.languages.iter().map(|l| ViewFlag {
                code: l.flag_code.trim().to_lowercase(),
                region_code: String::new(),
                name: l.name.clone(),
            }).collect(),
            title_foreign: detail.disc.title_foreign.clone().unwrap_or_default(),
            disc_title: detail.disc.disc_title.clone().unwrap_or_default(),
            disc_number: detail.disc.disc_number.clone().unwrap_or_default(),
            serial_count: detail.disc.serial.len(),
            serial: detail.disc.serial.join("<br>"),
            exe_date: detail.disc.exe_date.clone().unwrap_or_default(),
            version: detail.disc.version.clone().unwrap_or_default(),
            edition_count: detail.disc.edition.len(),
            edition: detail.disc.edition.join("<br>"),
            barcode_count: detail.disc.barcode.len(),
            barcode: detail.disc.barcode.join("<br>"),
            layerbreaks: detail.disc.layerbreaks.as_deref().unwrap_or_default()
                .iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "),
            comments: format_comments(&detail.disc.comments.clone().unwrap_or_default()),
            contents: format_comments(&detail.disc.contents.clone().unwrap_or_default()),
            edc_display: detail.disc.edc.map(|e| if e { "Yes" } else { "No" }.to_string()).unwrap_or_default(),
            protection: detail.disc.protection.clone().unwrap_or_default(),
            error_count: detail.disc.error_count.map(|e| e.to_string()).unwrap_or_default(),
            file_count: detail.files.len(),
            status_class: if detail.disc.enabled {
                let dumper_count = detail.dumpers.len() as i64;
                DiscStatus::compute(detail.disc.questionable, dumper_count).css_class().to_string()
            } else {
                "bad".to_string()
            },
            status_emoji: if detail.disc.enabled {
                let dumper_count = detail.dumpers.len() as i64;
                DiscStatus::compute(detail.disc.questionable, dumper_count).emoji().to_string()
            } else {
                "🔴".to_string()
            },
            status_display: if detail.disc.enabled {
                let dumper_count = detail.dumpers.len() as i64;
                DiscStatus::compute(detail.disc.questionable, dumper_count).to_string()
            } else {
                "Disabled".to_string()
            },
            created_at: detail.added_at.map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default(),
            updated_at: detail.modified_at.map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default(),
            dumper_count: detail.dumpers.len(),
            dumpers_display: if detail.dumpers.is_empty() {
                "Unknown".to_string()
            } else {
                detail.dumpers.iter()
                    .map(|d| format!(
                        "<a href=\"/discs/?dumper={}\">{}</a>",
                        d.user_id,
                        d.username.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"),
                    ))
                    .collect::<Vec<_>>()
                    .join("<br>")
            },
            ring_vis: RingColVis::from_rows(
                &ring_rows,
                detail.system.has_sample_start,
                detail.system.has_offset_extra,
            ),
            ring_rows,
            files,
            sbi_rows,
            pvd_rows,
            pic_rows,
            show_keys: detail.system.has_keys,
            disc_key,
            disc_id_hex,
            sector_ranges,
            header_rows,
            bca_rows,
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
            b'>' => { result.push_str("&gt;"); i += 1; }
            b'&' => { result.push_str("&amp;"); i += 1; }
            b'"' => { result.push_str("&quot;"); i += 1; }
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
        "<b>", "</b>", "<B>", "</B>",
        "<u>", "</u>", "<U>", "</U>",
        "<i>", "</i>",
        "<s>", "</s>",
        "<code>", "</code>",
        "<tt>", "</tt>",
        "<xmp>", "</xmp>",
        "<li>", "</li>",
        "<ul>", "</ul>",
        "<center>", "</center>",
        "<del>", "</del>",
        "<sup>", "</sup>",
        "<small>", "</small>",
        "<br>", "<br />", "<BR>",
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

    LABELS.iter().enumerate().map(|(i, label)| {
        let start = PVD_DATE_OFFSET + i * PVD_DATE_SIZE;
        let field = &data[start..start + PVD_DATE_SIZE];

        let hex_contents = field.iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ");

        let ascii: String = field[..16].iter()
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
    }).collect()
}

fn parse_header_rows(data: &[u8]) -> Vec<HeaderRow> {
    data.chunks(16).enumerate().map(|(i, chunk)| {
        let hex_contents = chunk.iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii: String = chunk.iter().map(|&b| {
            if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }
        }).collect();
        HeaderRow {
            offset: format!("{:04X}", i * 16),
            hex_contents,
            ascii,
        }
    }).collect()
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

fn parse_sbi_display(text: &str) -> Vec<SbiRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let msf = line.strip_prefix("MSF: ")
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("")
            .to_string();
        let qdata_str = line.find("Q-Data: ")
            .map(|i| &line[i + 8..])
            .unwrap_or("");
        let qdata_bytes = parse_qdata_bytes(qdata_str);
        let sector = parse_msf_to_sector(&msf).unwrap_or(0);
        let contents = format_sbi_contents(sector, &qdata_bytes);
        let xor = if qdata_bytes.len() >= 12 {
            compute_sbi_xor(sector, &qdata_bytes)
        } else {
            String::new()
        };
        rows.push(SbiRow { sector, msf, contents, xor });
    }
    rows
}

fn parse_qdata_bytes(qdata: &str) -> Vec<u8> {
    let cleaned: String = qdata.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    (0..cleaned.len() / 2)
        .filter_map(|i| u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

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
    if parts.len() != 3 { return None; }
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
    Path(id): Path<i32>,
) -> AppResult<impl IntoResponse> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;

    let cue = detail.disc.cue
        .filter(|c| !c.is_empty())
        .ok_or(crate::error::AppError::NotFound)?;

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
            (http::header::CONTENT_TYPE, "application/x-cuesheet".to_string()),
            (http::header::CONTENT_DISPOSITION, disposition),
        ],
        cue,
    ))
}

async fn disc_sbi_download(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> AppResult<impl IntoResponse> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;

    let sbi_text = detail.disc.sbi
        .filter(|s| !s.is_empty())
        .ok_or(crate::error::AppError::NotFound)?;

    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"SBI\0");
    for line in sbi_text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let msf_str = line.strip_prefix("MSF: ")
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("");
        let msf_parts: Vec<&str> = msf_str.split(':').collect();
        if msf_parts.len() != 3 { continue; }
        let msf_bytes: Vec<u8> = msf_parts.iter()
            .filter_map(|p| u8::from_str_radix(p, 16).ok())
            .collect();
        if msf_bytes.len() != 3 { continue; }
        let qdata_str = line.find("Q-Data: ")
            .map(|i| &line[i + 8..])
            .unwrap_or("");
        let qdata = parse_qdata_bytes(qdata_str);
        if qdata.len() < 10 { continue; }
        buf.extend_from_slice(&msf_bytes);
        buf.push(0x01);
        buf.extend_from_slice(&qdata[..10]);
    }

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
            (http::header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (http::header::CONTENT_DISPOSITION, disposition),
        ],
        buf,
    ))
}
