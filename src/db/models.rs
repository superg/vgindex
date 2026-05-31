use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha1::Digest;

// --- Enums ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaType {
    code: String,
    name: String,
    layer_count: i32,
    pic: bool,
    rom_extension: String,
}

impl MediaType {
    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_cd(&self) -> bool {
        is_cd_rom_extension(&self.rom_extension)
    }

    pub fn has_pic(&self) -> bool {
        self.pic
    }

    pub fn rom_extension(&self) -> &str {
        &self.rom_extension
    }

    pub fn max_layers(&self) -> u32 {
        self.layer_count as u32
    }
}

pub fn is_cd_rom_extension(rom_extension: &str) -> bool {
    rom_extension.eq_ignore_ascii_case("bin")
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl sqlx::Type<sqlx::Postgres> for MediaType {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for MediaType {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let code = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(Self {
            code: code.trim().to_string(),
            name: String::new(),
            layer_count: 1,
            pic: false,
            rom_extension: String::new(),
        })
    }
}

impl<'q> sqlx::Encode<'q, sqlx::Postgres> for MediaType {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(&self.code, buf)
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaTypeRow {
    pub code: String,
    pub name: String,
    pub layer_count: i32,
    pub pic: bool,
    pub rom_extension: String,
}

impl From<MediaTypeRow> for MediaType {
    fn from(row: MediaTypeRow) -> Self {
        Self {
            code: row.code,
            name: row.name,
            layer_count: row.layer_count,
            pic: row.pic,
            rom_extension: row.rom_extension,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[repr(i32)]
pub enum Category {
    Games = 1,
    Demos = 2,
    Coverdiscs = 3,
    #[serde(rename = "Bonus Discs")]
    BonusDiscs = 4,
    Applications = 5,
    Multimedia = 6,
    #[serde(rename = "Add-Ons")]
    AddOns = 7,
    Educational = 8,
    Preproduction = 9,
    Video = 10,
    Audio = 11,
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BonusDiscs => write!(f, "Bonus Discs"),
            Self::AddOns => write!(f, "Add-Ons"),
            other => write!(f, "{:?}", other),
        }
    }
}

impl Category {
    pub const ALL: &[Category] = &[
        Self::Games,
        Self::Demos,
        Self::Coverdiscs,
        Self::BonusDiscs,
        Self::Applications,
        Self::Multimedia,
        Self::AddOns,
        Self::Educational,
        Self::Preproduction,
        Self::Video,
        Self::Audio,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "disc_status_enum", rename_all = "PascalCase")]
pub enum DiscStatus {
    Disabled,
    Questionable,
    Unverified,
    Verified,
}

impl DiscStatus {
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Disabled => "bad",
            Self::Questionable => "questionable",
            Self::Unverified => "unverified",
            Self::Verified => "verified",
        }
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Disabled => "🔴",
            Self::Questionable => "🟡",
            Self::Unverified => "🔵",
            Self::Verified => "🟢",
        }
    }
}

impl std::fmt::Display for DiscStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type,
)]
#[sqlx(type_name = "user_role_enum")]
pub enum UserRole {
    User,
    #[sqlx(rename = "User+")]
    #[serde(rename = "User+")]
    UserPlus,
    Moderator,
    Admin,
}

impl UserRole {
    pub fn can_edit_directly(&self) -> bool {
        *self >= Self::UserPlus
    }

    pub fn can_moderate(&self) -> bool {
        *self >= Self::Moderator
    }

    pub fn can_admin(&self) -> bool {
        *self >= Self::Admin
    }

    pub fn can_submit(&self) -> bool {
        *self >= Self::User
    }

    pub fn can_edit_wiki(&self) -> bool {
        *self >= Self::UserPlus
    }
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "User"),
            Self::UserPlus => write!(f, "User+"),
            Self::Moderator => write!(f, "Moderator"),
            Self::Admin => write!(f, "Admin"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "submission_type_enum")]
pub enum SubmissionType {
    Disc,
    Edit,
}

impl std::fmt::Display for SubmissionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disc => write!(f, "Disc"),
            Self::Edit => write!(f, "Edit"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "submission_status_enum", rename_all = "PascalCase")]
pub enum SubmissionStatus {
    Pending,
    Approved,
    Rejected,
    Legacy,
}

impl SubmissionStatus {
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Pending => "status-pending",
            Self::Approved => "status-approved",
            Self::Rejected => "status-rejected",
            Self::Legacy => "status-legacy",
        }
    }
}

impl std::fmt::Display for SubmissionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// --- Row structs ---

/// Platform row from `systems` (`code` is the PK, VARCHAR(16)).
///
/// Display naming is derived from `type` / `manufacturer` / `name` via
/// [`build_system_name`] and [`build_dat_system_name`]; do not concatenate
/// these fields by hand at call sites.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct System {
    pub code: String,
    #[sqlx(rename = "type")]
    pub system_type: String,
    pub manufacturer: String,
    pub name: String,
    pub short_name: String,
    pub media_types: Vec<String>,
    pub has_exe_date: bool,
    pub has_sbi: bool,
    pub has_pvd: bool,
    pub has_edc: bool,
    pub has_disc_id: bool,
    pub has_key: bool,
    pub has_title_foreign: bool,
    pub has_disc_title: bool,
    pub has_disc_number: bool,
    pub has_serial: bool,
    pub has_barcode: bool,
    pub has_version: bool,
    pub has_edition: bool,
    pub has_protection: bool,
    pub has_sector_ranges: bool,
    pub has_header: bool,
    pub has_bca: bool,
    pub has_sample_start: bool,
    pub has_offset_extra: bool,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Region {
    pub code: String,
    pub name: String,
    pub flag_code: String,
    pub sort_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Language {
    pub code: String,
    pub name: String,
    pub flag_code: String,
    pub sort_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: i32,
    pub username: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub user_id: Option<i32>,
    pub role: Option<UserRole>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Disc {
    pub id: i32,
    pub system_code: String,
    #[sqlx(rename = "media_type_code")]
    pub media_type: MediaType,
    pub title: String,
    pub title_foreign: Option<String>,
    pub disc_title: Option<String>,
    pub disc_number: Option<String>,
    pub serial: Vec<String>,
    #[sqlx(rename = "category_id")]
    pub category: Category,
    pub version: Option<String>,
    pub edition: Vec<String>,
    pub barcode: Vec<String>,
    pub comments: Option<String>,
    pub contents: Option<String>,
    pub filename_suffix: Option<String>,
    pub error_count: Option<i32>,
    pub exe_date: Option<String>,
    pub edc: bool,
    pub layerbreaks: Option<Vec<i32>>,
    pub protection: Option<String>,
    pub sbi: Option<String>,
    pub disc_id: Option<String>,
    pub disc_key: Option<Vec<u8>>,
    pub cue: Option<String>,
    pub pvd: Option<Vec<u8>>,
    pub pic: Option<Vec<u8>>,
    pub header: Option<Vec<u8>>,
    pub bca: Option<Vec<u8>>,
    pub status: DiscStatus,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscRingCodeEntry {
    pub id: i32,
    pub disc_id: i32,
    pub offset_value: Option<i32>,
    pub offset_extra_value: Option<i32>,
    pub sample_data_start: Option<i32>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscRingCodeLayer {
    pub id: i32,
    pub entry_id: i32,
    pub layer: i32,
    pub mastering_code: Option<String>,
    pub mastering_sid: Option<String>,
    pub mould_sids: String,
    pub toolstamps: String,
    pub additional_moulds: String,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct File {
    pub id: i32,
    pub disc_id: i32,
    pub track_number: Option<String>,
    pub size: i64,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscSubmission {
    pub id: i32,
    pub submission_type: SubmissionType,
    pub submitter_id: i32,
    pub submission_comment: Option<String>,
    pub target_disc_id: Option<i32>,
    pub changes: serde_json::Value,
    pub dump_log: Option<String>,
    pub extra_upload_url: Option<String>,
    pub status: SubmissionStatus,
    pub reviewer_id: Option<i32>,
    pub review_comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

// --- Composite/view structs for rendering ---

#[derive(Debug, Clone)]
pub struct DiscListRow {
    pub id: i32,
    pub title: String,
    pub system_code: String,
    pub system_full: String,
    pub media_type: MediaType,
    pub version: Option<String>,
    pub edition: Vec<String>,
    pub status: DiscStatus,
    pub dumper_count: i64,
    pub region_flags: Vec<FlagInfo>,
    pub language_flags: Vec<FlagInfo>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FlagInfo {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct DiscDetail {
    pub disc: Disc,
    pub system: System,
    pub regions: Vec<Region>,
    pub languages: Vec<Language>,
    pub ring_entries: Vec<RingEntryView>,
    pub files: Vec<File>,
    pub dumpers: Vec<DumperInfo>,
    pub disc_submission_count: i64,
    pub sector_ranges: Vec<ProtectionRange>,
    pub added_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProtectionRange {
    pub range_start: i32,
    pub range_end: i32,
}

#[derive(Debug, Clone)]
pub struct RingEntryView {
    pub id: i32,
    pub offset_value: Option<i32>,
    pub offset_extra_value: Option<i32>,
    pub sample_data_start: Option<i32>,
    pub comment: Option<String>,
    pub layers: Vec<DiscRingCodeLayer>,
}

#[derive(Debug, Clone)]
pub struct DumperInfo {
    pub user_id: i32,
    pub username: String,
}

pub fn format_display_title(
    title: &str,
    disc_number: Option<&str>,
    disc_title: Option<&str>,
    filename_suffix: Option<&str>,
) -> String {
    let mut result = title.to_string();
    if let Some(n) = disc_number {
        if !n.is_empty() {
            result.push_str(&format!(" (Disc {n})"));
        }
    }
    if let Some(d) = disc_title {
        if !d.is_empty() {
            result.push_str(&format!(" ({d})"));
        }
    }
    if let Some(s) = filename_suffix {
        if !s.is_empty() {
            result.push_str(&format!(" ({s})"));
        }
    }
    result
}

pub fn sanitize_filename(s: &str) -> String {
    // Longer multi-char replacements first (order matters: ": " before ":")
    const REPLACEMENTS: &[(&str, &str)] = &[
        ("Böse", "Boese"),
        (": ", " - "),
        ("\"", ""),
        ("*", "-"),
        (":", "-"),
        ("/", "-"),
        ("?", ""),
        ("°", ""),
        ("Ä", "A"),
        ("å", "a"),
        ("ä", "a"),
        ("É", "E"),
        ("é", "e"),
        ("ё", "e"),
        ("Ö", "O"),
        ("ö", "o"),
        ("Ñ", "N"),
        ("ñ", "n"),
        ("³", " 3"),
        ("α", "Alpha"),
    ];
    let mut result = s.to_string();
    for &(from, to) in REPLACEMENTS {
        result = result.replace(from, to);
    }
    result
}

/// Join non-empty parts with `separator`, dropping empties so we never end up
/// with leading/trailing/duplicated separators.
fn join_non_empty(parts: &[&str], separator: &str) -> String {
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(separator)
}

/// Canonical user-facing system name: `"{manufacturer} {name}"`, with empty
/// parts omitted (no leading or duplicate spaces).
pub fn build_system_name(manufacturer: &str, name: &str) -> String {
    join_non_empty(&[manufacturer, name], " ")
}

/// Canonical DAT/archive system name: `"{type} - {manufacturer} - {name}"`,
/// with empty parts (and their dashes) omitted, then run through the shared
/// filename sanitizer so it is safe to embed in zip/dat filenames.
pub fn build_dat_system_name(system_type: &str, manufacturer: &str, name: &str) -> String {
    let joined = join_non_empty(&[system_type, manufacturer, name], " - ");
    sanitize_filename(&joined)
}

impl System {
    /// Full human-readable system name (manufacturer + product name).
    pub fn system_name(&self) -> String {
        build_system_name(&self.manufacturer, &self.name)
    }

    /// Filename-safe system name used for DAT/archive zips and DAT XML.
    pub fn dat_system_name(&self) -> String {
        build_dat_system_name(&self.system_type, &self.manufacturer, &self.name)
    }

    /// Compact UI label: `short_name` if set, otherwise the system code.
    pub fn short_display(&self) -> String {
        if self.short_name.is_empty() {
            self.code.clone()
        } else {
            self.short_name.clone()
        }
    }
}

/// Compact UI label given a system's `short_name` and `code` columns.
/// Mirrors [`System::short_display`] for callers that only have the two
/// strings (e.g. raw query rows).
pub fn short_system_display(short_name: &str, code: &str) -> String {
    if short_name.is_empty() {
        code.to_string()
    } else {
        short_name.to_string()
    }
}

pub fn build_rom_base_name(
    title: &str,
    region_names: &[String],
    language_codes: &[String],
    disc_number: Option<&str>,
    disc_title: Option<&str>,
    filename_suffix: Option<&str>,
) -> String {
    let mut name = title.to_string();
    if !region_names.is_empty() {
        name.push_str(&format!(" ({})", region_names.join(", ")));
    }
    if language_codes.len() > 1 {
        let capitalized: Vec<String> = language_codes
            .iter()
            .map(|c| {
                let mut chars = c.chars();
                match chars.next() {
                    Some(first) => {
                        let upper: String = first.to_uppercase().collect();
                        format!("{upper}{}", chars.as_str())
                    }
                    None => String::new(),
                }
            })
            .collect();
        name.push_str(&format!(" ({})", capitalized.join(",")));
    }
    if let Some(n) = disc_number {
        if !n.is_empty() {
            name.push_str(&format!(" (Disc {n})"));
        }
    }
    if let Some(d) = disc_title {
        if !d.is_empty() {
            name.push_str(&format!(" ({d})"));
        }
    }
    if let Some(s) = filename_suffix {
        if !s.is_empty() {
            name.push_str(&format!(" ({s})"));
        }
    }
    sanitize_filename(&name)
}

pub fn build_rom_name(
    base_name: &str,
    track_number: Option<&str>,
    total_tracks: usize,
    extension: &str,
) -> String {
    let mut name = base_name.to_string();
    if total_tracks > 1 {
        if let Some(t) = track_number {
            let n: u32 = t.parse().unwrap_or(0);
            if total_tracks >= 10 {
                name.push_str(&format!(" (Track {n:02})"));
            } else {
                name.push_str(&format!(" (Track {n})"));
            }
        }
    }
    name.push('.');
    name.push_str(extension);
    name
}

pub fn build_simple_track_name(
    track_number: Option<&str>,
    total_tracks: usize,
    extension: &str,
) -> String {
    let mut name = String::from("Track");
    if total_tracks > 1 {
        if let Some(t) = track_number {
            let n: u32 = t.parse().unwrap_or(0);
            if total_tracks >= 10 {
                name.push_str(&format!(" {n:02}"));
            } else {
                name.push_str(&format!(" {n}"));
            }
        }
    }
    name.push('.');
    name.push_str(extension);
    name
}

/// Returns true for `REM LEAD-OUT|LEAD-IN|PREGAP` lines (case-insensitive,
/// whitespace-delimited) which carry no information beyond the multi-session
/// structure itself and are therefore dropped during cue canonicalization.
fn is_strippable_rem(trimmed: &str) -> bool {
    let upper = trimmed.to_ascii_uppercase();
    let rest = match upper.strip_prefix("REM ") {
        Some(r) => r.trim_start(),
        None => return false,
    };
    let tag = rest.split_whitespace().next().unwrap_or("");
    matches!(tag, "LEAD-OUT" | "LEAD-IN" | "PREGAP")
}

fn ensure_single_trailing_newline(mut s: String) -> String {
    while s.ends_with('\n') || s.ends_with('\r') {
        s.pop();
    }
    s.push('\n');
    s
}

pub fn simplify_cue(raw_cue: &str, extension: &str) -> String {
    let normalized = raw_cue.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.lines().collect();
    let total_tracks = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("TRACK "))
        .count();

    let mut result = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if is_strippable_rem(trimmed) {
            i += 1;
            continue;
        }
        if trimmed.starts_with("FILE ") {
            let track_num = lines[i + 1..].iter().find_map(|l| {
                let lt = l.trim_start();
                if lt.starts_with("TRACK ") {
                    lt.split_whitespace()
                        .nth(1)
                        .map(|n| n.trim_start_matches('0'))
                        .map(|n| if n.is_empty() { "0" } else { n })
                } else {
                    None
                }
            });
            let simple_name = build_simple_track_name(track_num, total_tracks, extension);
            let file_type = trimmed.rsplit_once(' ').map(|(_, t)| t).unwrap_or("BINARY");
            result.push(format!("FILE \"{simple_name}\" {file_type}"));
        } else {
            result.push(lines[i].to_string());
        }
        i += 1;
    }
    ensure_single_trailing_newline(result.join("\n"))
}

pub fn simplify_files_xml(raw: &str, extension: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let total_tracks = lines
        .iter()
        .filter(|l| l.trim().starts_with("<rom "))
        .count();

    lines
        .iter()
        .map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with("<rom ") {
                return (*line).to_string();
            }
            let name = extract_rom_name_attr(trimmed);
            let track_num = name.as_deref().and_then(extract_track_from_filename);
            let track_str = track_num
                .as_deref()
                .map(|n| n.trim_start_matches('0'))
                .map(|n| if n.is_empty() { "0" } else { n });
            let simple_name = build_simple_track_name(track_str, total_tracks, extension);
            match name {
                Some(ref old_name) => trimmed.replacen(
                    &format!("name=\"{old_name}\""),
                    &format!("name=\"{simple_name}\""),
                    1,
                ),
                None => trimmed.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_rom_name_attr(line: &str) -> Option<String> {
    let needle = "name=\"";
    let start = line.find(needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

pub(crate) fn extract_track_from_filename(filename: &str) -> Option<String> {
    if filename.ends_with(".iso") {
        return Some("1".to_string());
    }
    let lower = filename.to_lowercase();
    if lower.starts_with("track.") {
        return Some("1".to_string());
    }
    if let Some(pos) = lower.find("track ") {
        let rest = &filename[pos + 6..];
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !num.is_empty() {
            return Some(num);
        }
    }
    None
}

pub fn finalize_cue(raw_cue: &str, base_name: &str, extension: &str) -> String {
    let lines: Vec<&str> = raw_cue.lines().collect();
    let total_tracks = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("TRACK "))
        .count();

    let mut result = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if is_strippable_rem(trimmed) {
            i += 1;
            continue;
        }
        if trimmed.starts_with("FILE ") {
            // Find the next TRACK line to get the track number
            let track_num = lines[i + 1..].iter().find_map(|l| {
                let lt = l.trim_start();
                if lt.starts_with("TRACK ") {
                    lt.split_whitespace()
                        .nth(1)
                        .map(|n| n.trim_start_matches('0'))
                        .map(|n| if n.is_empty() { "0" } else { n })
                } else {
                    None
                }
            });
            let rom_name = build_rom_name(base_name, track_num, total_tracks, extension);
            let file_type = trimmed.rsplit_once(' ').map(|(_, t)| t).unwrap_or("BINARY");
            result.push(format!("FILE \"{rom_name}\" {file_type}"));
        } else {
            result.push(lines[i].to_string());
        }
        i += 1;
    }
    ensure_single_trailing_newline(result.join("\n"))
}

pub fn parse_qdata_bytes(qdata: &str) -> Vec<u8> {
    let cleaned: String = qdata.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (0..cleaned.len() / 2)
        .filter_map(|i| u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

pub fn build_sbi_binary(sbi_text: &str) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(b"SBI\0");
    for line in sbi_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msf_str = match line.strip_prefix("MSF: ") {
            Some(s) => s.split_whitespace().next().unwrap_or(""),
            None => continue,
        };
        let msf_parts: Vec<&str> = msf_str.split(':').collect();
        if msf_parts.len() != 3 {
            continue;
        }
        let msf_bytes: Vec<u8> = msf_parts
            .iter()
            .filter_map(|p| u8::from_str_radix(p, 16).ok())
            .collect();
        if msf_bytes.len() != 3 {
            continue;
        }
        let qdata_str = match line.find("Q-Data: ") {
            Some(i) => &line[i + 8..],
            None => continue,
        };
        let qdata = parse_qdata_bytes(qdata_str);
        if qdata.len() < 10 {
            continue;
        }
        buf.extend_from_slice(&msf_bytes);
        buf.push(0x01);
        buf.extend_from_slice(&qdata[..10]);
    }
    buf
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn compute_file_hashes(data: &[u8]) -> (i64, String, String, String) {
    let size = data.len() as i64;

    let crc = crc32fast::hash(data);
    let crc32_hex = format!("{crc:08x}");

    let md5_hex = format!("{:x}", <md5::Md5 as Digest>::digest(data));
    let sha1_hex = format!("{:x}", <sha1::Sha1 as Digest>::digest(data));

    (size, crc32_hex, md5_hex, sha1_hex)
}

#[derive(Debug, Clone)]
pub struct SubmissionListRow {
    pub id: i32,
    pub submission_type: SubmissionType,
    pub title: String,
    pub system_code: String,
    pub system_display: String,
    pub submitter: String,
    pub submitter_id: i32,
    pub reviewer: Option<String>,
    pub reviewer_id: Option<i32>,
    pub status: SubmissionStatus,
    pub target_disc_id: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_cue_outputs_lf_for_crlf_input() {
        let cue = "FILE \"Game.bin\" BINARY\r\n  TRACK 01 AUDIO\r\n    INDEX 01 00:00:00\r\n";
        let simplified = simplify_cue(cue, "bin");

        assert_eq!(
            simplified,
            "FILE \"Track.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n"
        );
        assert!(!simplified.contains('\r'));
    }

    #[test]
    fn simplify_cue_strips_lead_and_pregap_rems_keeps_others() {
        let cue = "REM SESSION 01\n\
FILE \"Track 01.bin\" BINARY\n\
  TRACK 01 AUDIO\n\
    INDEX 01 00:00:00\n\
REM LEAD-OUT 02:00:00\n\
REM SESSION 02\n\
  rem lead-in 01:00:00\n\
REM PREGAP 00:02:00\n\
FILE \"Track 02.bin\" BINARY\n\
  TRACK 02 AUDIO\n\
    INDEX 01 00:00:00\n";
        let simplified = simplify_cue(cue, "bin");

        let expected = "REM SESSION 01\n\
FILE \"Track 1.bin\" BINARY\n\
  TRACK 01 AUDIO\n\
    INDEX 01 00:00:00\n\
REM SESSION 02\n\
FILE \"Track 2.bin\" BINARY\n\
  TRACK 02 AUDIO\n\
    INDEX 01 00:00:00\n";
        assert_eq!(simplified, expected);
        assert!(!simplified.contains("LEAD-OUT"));
        assert!(!simplified.contains("LEAD-IN"));
        assert!(!simplified.to_uppercase().contains("PREGAP"));
        assert!(simplified.contains("REM SESSION 01"));
        assert!(simplified.contains("REM SESSION 02"));
    }

    #[test]
    fn simplify_cue_ends_with_single_newline() {
        let no_trailing = "FILE \"Game.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00";
        let many_trailing =
            "FILE \"Game.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n\n\n";

        for input in [no_trailing, many_trailing] {
            let out = simplify_cue(input, "bin");
            assert!(out.ends_with('\n'), "missing trailing newline: {out:?}");
            assert!(
                !out.ends_with("\n\n"),
                "more than one trailing newline: {out:?}"
            );
        }
    }

    #[test]
    fn finalize_cue_strips_lead_rems_and_ends_with_newline() {
        let cue = "FILE \"Track 01.bin\" BINARY\n\
                   TRACK 01 AUDIO\n\
                   INDEX 01 00:00:00\n\
                   REM LEAD-OUT 02:00:00\n\
                   REM LEAD-IN 01:00:00\n\
                   REM PREGAP 00:02:00\n";
        let out = finalize_cue(cue, "Awesome Game", "bin");

        assert!(out.ends_with('\n'));
        assert!(!out.ends_with("\n\n"));
        assert!(!out.contains("LEAD-OUT"));
        assert!(!out.contains("LEAD-IN"));
        assert!(!out.to_uppercase().contains("PREGAP"));
        assert!(out.contains("FILE \"Awesome Game.bin\" BINARY"));
    }

    #[test]
    fn system_name_drops_empty_parts() {
        assert_eq!(build_system_name("Sony", "PlayStation"), "Sony PlayStation");
        assert_eq!(build_system_name("", "Audio CD"), "Audio CD");
        assert_eq!(build_system_name("Sony", ""), "Sony");
        assert_eq!(build_system_name("", ""), "");
        assert_eq!(
            build_system_name("  Sony  ", "  PlayStation  "),
            "Sony PlayStation"
        );
    }

    #[test]
    fn dat_system_name_drops_empty_parts_and_sanitizes() {
        assert_eq!(
            build_dat_system_name("Arcade", "Sega", "Naomi"),
            "Arcade - Sega - Naomi"
        );
        assert_eq!(
            build_dat_system_name("", "Sony", "PlayStation"),
            "Sony - PlayStation"
        );
        assert_eq!(
            build_dat_system_name("Arcade", "", "Lindbergh"),
            "Arcade - Lindbergh"
        );
        assert_eq!(build_dat_system_name("", "", "Audio CD"), "Audio CD");
        assert_eq!(build_dat_system_name("", "", ""), "");
    }

    #[test]
    fn dat_system_name_colon_sanitization() {
        assert_eq!(
            build_dat_system_name("", "", "Foo: Bar"),
            "Foo - Bar",
            "': ' must be replaced with ' - ' before ':' is mapped to '-'"
        );
        assert_eq!(build_dat_system_name("", "", "Foo:Bar"), "Foo-Bar");
    }

    #[test]
    fn short_system_display_falls_back_to_code() {
        assert_eq!(short_system_display("Wii", "WII"), "Wii");
        assert_eq!(short_system_display("", "PSX"), "PSX");
    }
}
