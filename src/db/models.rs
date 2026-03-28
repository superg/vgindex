use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha1::Digest;

// --- Enums ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaType {
    code: String,
    name: String,
    layer_count: i32,
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
        self.rom_extension == "bin"
    }

    pub fn rom_extension(&self) -> &str {
        &self.rom_extension
    }

    pub fn max_layers(&self) -> u32 {
        self.layer_count as u32
    }
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
    pub rom_extension: String,
}

impl From<MediaTypeRow> for MediaType {
    fn from(row: MediaTypeRow) -> Self {
        Self {
            code: row.code,
            name: row.name,
            layer_count: row.layer_count,
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
        Self::Games, Self::Demos, Self::Coverdiscs, Self::BonusDiscs,
        Self::Applications, Self::Multimedia, Self::AddOns, Self::Educational,
        Self::Preproduction, Self::Video, Self::Audio,
    ];
}

/// Computed disc verification status (not stored; derived from `questionable` flag + dumper count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscStatus {
    Verified,
    Unverified,
    Questionable,
}

impl DiscStatus {
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::Questionable => "questionable",
        }
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Verified => "🟢",
            Self::Unverified => "🔵",
            Self::Questionable => "🟡",
        }
    }

    pub fn compute(questionable: bool, dumper_count: i64) -> Self {
        if questionable {
            Self::Questionable
        } else if dumper_count > 1 {
            Self::Verified
        } else {
            Self::Unverified
        }
    }
}

impl std::fmt::Display for DiscStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, sqlx::Type)]
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
    #[sqlx(rename = "New Dump")]
    #[serde(rename = "New Dump")]
    NewDump,
    Verification,
    Edit,
}

impl std::fmt::Display for SubmissionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewDump => write!(f, "New Dump"),
            Self::Verification => write!(f, "Verification"),
            Self::Edit => write!(f, "Edit"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "submission_status_enum", rename_all = "PascalCase")]
pub enum SubmissionStatus {
    Pending,
    Approved,
    Denied,
}

impl SubmissionStatus {
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Pending => "status-pending",
            Self::Approved => "status-approved",
            Self::Denied => "status-denied",
        }
    }
}

impl std::fmt::Display for SubmissionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// --- Row structs ---

/// Platform row from `systems` (`code` is the PK, VARCHAR(16); `name` matches Redump's system name).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct System {
    pub code: String,
    pub name: String,
    pub media_types: Vec<String>,
    pub has_exe_date: bool,
    pub has_protection_sbi: bool,
    pub has_pvd: bool,
    pub has_m2f2_edc: bool,
    pub has_title_foreign: bool,
    pub has_title_disc: bool,
    pub has_title_disc_number: bool,
    pub has_serial: bool,
    pub has_barcode: bool,
    pub has_version: bool,
    pub has_edition: bool,
    pub has_error_count: bool,
    pub has_protection: bool,
    pub has_pic: bool,
    pub has_protection_ranges: bool,
    pub has_header: bool,
    pub has_bca: bool,
    pub has_sample_start: bool,
    pub sort_order: i32,
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
    pub email: String,
    pub password_hash: String,
    pub role: UserRole,
    pub email_verified: bool,
    pub email_verify_token: Option<String>,
    pub email_verify_expires_at: Option<DateTime<Utc>>,
    pub password_reset_token: Option<String>,
    pub password_reset_expires_at: Option<DateTime<Utc>>,
    pub failed_login_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub user_id: Option<i32>,
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
    pub title_disc: Option<String>,
    pub title_disc_number: Option<String>,
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
    pub m2f2_edc: Option<bool>,
    pub layerbreaks: Option<Vec<i32>>,
    pub protection: Option<String>,
    pub protection_sbi: Option<String>,
    pub protection_keys: Option<Vec<String>>,
    pub cue: Option<String>,
    pub pvd: Option<Vec<u8>>,
    pub pic: Option<Vec<u8>>,
    pub header: Option<Vec<u8>>,
    pub bca: Option<Vec<u8>>,
    pub enabled: bool,
    pub questionable: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscRingCodeEntry {
    pub id: i32,
    pub disc_id: i32,
    pub offset_value: Option<Vec<i32>>,
    pub sample_data_start: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscRingCodeLayer {
    pub id: i32,
    pub entry_id: i32,
    pub layer: i32,
    pub mastering_code: Option<String>,
    pub mastering_sid: Option<String>,
    pub mould_sids: Vec<String>,
    pub toolstamps: Vec<String>,
    pub additional_moulds: Vec<String>,
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
    pub target_disc_id: Option<i32>,
    pub data: serde_json::Value,
    pub dump_log: Option<String>,
    pub extra_files_path: Option<String>,
    pub status: SubmissionStatus,
    pub reviewer_id: Option<i32>,
    pub review_comment: Option<String>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OAuthClient {
    pub id: i32,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
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
    pub enabled: bool,
    pub questionable: bool,
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
    pub protection_ranges: Vec<ProtectionRange>,
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
    pub offset_value: Option<Vec<i32>>,
    pub sample_data_start: Option<String>,
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
    title_disc_number: Option<&str>,
    title_disc: Option<&str>,
    filename_suffix: Option<&str>,
) -> String {
    let mut result = title.to_string();
    if let Some(n) = title_disc_number {
        if !n.is_empty() {
            result.push_str(&format!(" (Disc {n})"));
        }
    }
    if let Some(d) = title_disc {
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

pub fn build_rom_base_name(
    title: &str,
    region_names: &[String],
    language_codes: &[String],
    title_disc_number: Option<&str>,
    title_disc: Option<&str>,
    filename_suffix: Option<&str>,
) -> String {
    let mut name = title.to_string();
    if !region_names.is_empty() {
        name.push_str(&format!(" ({})", region_names.join(", ")));
    }
    if language_codes.len() > 1 {
        let capitalized: Vec<String> = language_codes.iter()
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
    if let Some(n) = title_disc_number {
        if !n.is_empty() {
            name.push_str(&format!(" (Disc {n})"));
        }
    }
    if let Some(d) = title_disc {
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

pub fn finalize_cue(raw_cue: &str, base_name: &str, extension: &str) -> String {
    let lines: Vec<&str> = raw_cue.lines().collect();
    let total_tracks = lines.iter()
        .filter(|l| l.trim_start().starts_with("TRACK "))
        .count();

    let mut result = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if trimmed.starts_with("FILE ") {
            // Find the next TRACK line to get the track number
            let track_num = lines[i + 1..].iter()
                .find_map(|l| {
                    let lt = l.trim_start();
                    if lt.starts_with("TRACK ") {
                        lt.split_whitespace().nth(1)
                            .map(|n| n.trim_start_matches('0'))
                            .map(|n| if n.is_empty() { "0" } else { n })
                    } else {
                        None
                    }
                });
            let rom_name = build_rom_name(
                base_name,
                track_num,
                total_tracks,
                extension,
            );
            let file_type = trimmed.rsplit_once(' ')
                .map(|(_, t)| t)
                .unwrap_or("BINARY");
            result.push(format!("FILE \"{rom_name}\" {file_type}"));
        } else {
            result.push(lines[i].to_string());
        }
        i += 1;
    }
    result.join("\n")
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
    pub submitter: String,
    pub status: SubmissionStatus,
    pub review_comment: Option<String>,
    pub created_at: DateTime<Utc>,
}
