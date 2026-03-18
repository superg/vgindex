use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

// --- Enums ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "media_type_enum", rename_all = "kebab-case")]
pub enum MediaType {
    #[sqlx(rename = "CD")]
    #[serde(rename = "CD")]
    Cd,
    #[sqlx(rename = "GD-ROM")]
    #[serde(rename = "GD-ROM")]
    GdRom,
    #[sqlx(rename = "DVD-5")]
    #[serde(rename = "DVD-5")]
    Dvd5,
    #[sqlx(rename = "DVD-9")]
    #[serde(rename = "DVD-9")]
    Dvd9,
    #[sqlx(rename = "HD-DVD")]
    #[serde(rename = "HD-DVD")]
    HdDvd,
    #[sqlx(rename = "BD-25")]
    #[serde(rename = "BD-25")]
    Bd25,
    #[sqlx(rename = "BD-50")]
    #[serde(rename = "BD-50")]
    Bd50,
    #[sqlx(rename = "BD-66")]
    #[serde(rename = "BD-66")]
    Bd66,
    #[sqlx(rename = "BD-100")]
    #[serde(rename = "BD-100")]
    Bd100,
    #[sqlx(rename = "UMD")]
    #[serde(rename = "UMD")]
    Umd,
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cd => write!(f, "CD"),
            Self::GdRom => write!(f, "GD-ROM"),
            Self::Dvd5 => write!(f, "DVD-5"),
            Self::Dvd9 => write!(f, "DVD-9"),
            Self::HdDvd => write!(f, "HD-DVD"),
            Self::Bd25 => write!(f, "BD-25"),
            Self::Bd50 => write!(f, "BD-50"),
            Self::Bd66 => write!(f, "BD-66"),
            Self::Bd100 => write!(f, "BD-100"),
            Self::Umd => write!(f, "UMD"),
        }
    }
}

impl MediaType {
    pub fn is_cd(&self) -> bool {
        matches!(self, Self::Cd | Self::GdRom)
    }

    pub fn max_layers(&self) -> u32 {
        match self {
            Self::Cd | Self::GdRom | Self::Umd => 1,
            Self::Dvd5 | Self::Dvd9 | Self::HdDvd => 2,
            Self::Bd25 | Self::Bd50 | Self::Bd66 | Self::Bd100 => 4,
        }
    }

    pub const ALL: &[MediaType] = &[
        Self::Cd, Self::GdRom, Self::Dvd5, Self::Dvd9, Self::HdDvd,
        Self::Bd25, Self::Bd50, Self::Bd66, Self::Bd100, Self::Umd,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "category_enum", rename_all = "PascalCase")]
pub enum Category {
    Games,
    Demos,
    Video,
    Audio,
    Multimedia,
    Applications,
    Coverdiscs,
    Educational,
    #[sqlx(rename = "Bonus Discs")]
    #[serde(rename = "Bonus Discs")]
    BonusDiscs,
    Betas,
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BonusDiscs => write!(f, "Bonus Discs"),
            other => write!(f, "{:?}", other),
        }
    }
}

impl Category {
    pub const ALL: &[Category] = &[
        Self::Games, Self::Demos, Self::Video, Self::Audio, Self::Multimedia,
        Self::Applications, Self::Coverdiscs, Self::Educational, Self::BonusDiscs, Self::Betas,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "disc_status_enum", rename_all = "PascalCase")]
pub enum DiscStatus {
    Verified,
    Good,
    Questionable,
    Bad,
}

impl DiscStatus {
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Good => "good",
            Self::Questionable => "questionable",
            Self::Bad => "bad",
        }
    }

    pub const ALL: &[DiscStatus] = &[Self::Verified, Self::Good, Self::Questionable, Self::Bad];
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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct System {
    pub id: i32,
    pub short_code: String,
    pub full_name: String,
    pub allowed_media: Vec<MediaType>,
    pub allowed_system_regions: Vec<i32>,
    pub has_date_field: bool,
    pub has_sbi: bool,
    pub has_pvd: bool,
    pub has_edc_field: bool,
    pub has_pic: bool,
    pub has_security_ranges: bool,
    pub has_header: bool,
    pub has_bca: bool,
    pub has_universal_hash: bool,
    pub display_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct SystemRegion {
    pub id: i32,
    pub name: String,
    pub flag_code: String,
    pub display_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct ReleaseRegion {
    pub id: i32,
    pub code: String,
    pub name: String,
    pub flag_code: String,
    pub display_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Language {
    pub id: i32,
    pub code: String,
    pub name: String,
    pub flag_code: String,
    pub display_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct TitleType {
    pub id: i32,
    pub name: String,
    pub display_order: i32,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct SerialType {
    pub id: i32,
    pub name: String,
    pub display_order: i32,
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
    pub system_id: i32,
    pub media_type: MediaType,
    pub title: String,
    pub category: Category,
    pub system_region_id: Option<i32>,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub barcode: Option<String>,
    pub comments: Option<String>,
    pub filename_suffix: Option<String>,
    pub error_count: Option<i32>,
    pub exe_date: Option<NaiveDate>,
    pub edc: Option<bool>,
    pub protection: Option<String>,
    pub sbi_data: Option<Vec<u8>>,
    pub pvd_data: Option<Vec<u8>>,
    pub pic_data: Option<Vec<u8>>,
    pub header_data: Option<Vec<u8>>,
    pub bca_data: Option<Vec<u8>>,
    pub universal_hash: Option<String>,
    pub status: DiscStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct DiscAltTitle {
    pub id: i32,
    pub disc_id: i32,
    pub title_type_id: i32,
    pub title: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DiscRingCodeEntry {
    pub id: i32,
    pub disc_id: i32,
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
    pub offset_value: Option<String>,
    pub sample_data_start: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct DiscSerial {
    pub id: i32,
    pub disc_id: i32,
    pub serial_type_id: i32,
    pub serial: String,
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
    pub system_short: String,
    pub system_full: String,
    pub media_type: MediaType,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub status: DiscStatus,
    pub system_region_flag: Option<String>,
    pub system_region_name: Option<String>,
    pub region_flags: Vec<FlagInfo>,
    pub language_flags: Vec<FlagInfo>,
    pub serials: Vec<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FlagInfo {
    pub flag_code: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct DiscDetail {
    pub disc: Disc,
    pub system: System,
    pub system_region: Option<SystemRegion>,
    pub release_regions: Vec<ReleaseRegion>,
    pub languages: Vec<Language>,
    pub alt_titles: Vec<(String, String)>,
    pub serials: Vec<(String, String)>,
    pub ring_entries: Vec<RingEntryView>,
    pub files: Vec<File>,
    pub dumpers: Vec<DumperInfo>,
}

#[derive(Debug, Clone)]
pub struct RingEntryView {
    pub layers: Vec<DiscRingCodeLayer>,
}

#[derive(Debug, Clone)]
pub struct DumperInfo {
    pub user_id: i32,
    pub username: String,
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
