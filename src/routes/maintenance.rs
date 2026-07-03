use askama::Template;
use axum::{
    extract::{Path as AxumPath, Query, Request, State},
    http::{header, HeaderValue},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::auth::{
    csrf::{self, CsrfForm},
    middleware::{AuthenticatedUser, RequireAdmin, RequireModerator},
};
use crate::config::SiteConfig;
use crate::db::models::System;
use crate::error::{AppError, AppResult};
use crate::services::{archive_service, disc_service};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/maintenance", get(maintenance_page))
        .route("/maintenance/systems", post(save_systems))
        .route("/maintenance/regions", post(save_regions))
        .route("/maintenance/languages", post(save_languages))
        .route("/maintenance/rebuild-cue", post(rebuild_cue))
        .route(
            "/maintenance/trigger-archive-generation",
            post(trigger_archive_generation),
        )
        .route("/maintenance/backups/{filename}", get(download_backup))
}

const BACKUP_DIR: &str = "./backups";
const BACKUP_PREFIX: &str = "vgindex-backup-";
const BACKUP_SUFFIX: &str = ".tar.gz";

#[derive(Deserialize, Default)]
struct MaintenanceQuery {
    status: Option<String>,
    error: Option<String>,
    tab: Option<String>,
}

#[derive(Template)]
#[template(path = "maintenance.html")]
struct MaintenanceTemplate {
    current_user: Option<AuthenticatedUser>,
    status_message: String,
    error_message: String,
    maintenance_errors: Vec<String>,
    system_rows: Vec<SystemEditorRow>,
    region_rows: Vec<LookupEditorRow>,
    language_rows: Vec<LookupEditorRow>,
    flag_options: Vec<FlagOption>,
    system_input_sizes: SystemInputSizes,
    region_input_sizes: LookupInputSizes,
    language_input_sizes: LookupInputSizes,
    backup_files: Vec<BackupFile>,
    can_admin: bool,
    show_general: bool,
    show_systems: bool,
    show_regions: bool,
    show_languages: bool,
    show_misc: bool,
    show_backup: bool,
}
impl SiteConfig for MaintenanceTemplate {}

struct BackupFile {
    filename: String,
    created_at: String,
    size: String,
}

async fn maintenance_page(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Query(query): Query<MaintenanceQuery>,
) -> AppResult<Html<String>> {
    if query.tab.as_deref() == Some("backup") && !user.role.can_admin() {
        return Err(AppError::Forbidden);
    }

    let template =
        build_maintenance_template(&state.pool, user, query, Vec::new(), None, None, None).await?;
    Ok(Html(template.render().unwrap()))
}

async fn download_backup(
    RequireAdmin(user): RequireAdmin,
    AxumPath(filename): AxumPath<String>,
    request: Request,
) -> Response {
    if backup_timestamp(&filename).is_none() {
        return AppError::NotFound.into_response();
    }

    let path = Path::new(BACKUP_DIR).join(&filename);
    let is_regular_file = std::fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false);
    if !is_regular_file {
        return AppError::NotFound.into_response();
    }

    tracing::info!(
        user_id = user.id,
        username = %user.username,
        backup = %filename,
        "Administrator downloaded database backup"
    );

    let mut response = ServeFile::new(path)
        .oneshot(request)
        .await
        .expect("ServeFile is infallible")
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .expect("validated backup filename is a valid header value"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(Deserialize)]
struct SystemsForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    systems_payload: String,
}

#[derive(Deserialize)]
struct RegionsForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    regions_payload: String,
}

#[derive(Deserialize)]
struct LanguagesForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    languages_payload: String,
}

async fn save_systems(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<SystemsForm>,
) -> AppResult<Response> {
    csrf::verify_token(&user, &form.csrf_token)?;

    let media_types = fetch_media_types(&state.pool).await?;
    let existing_systems = disc_service::get_all_systems(&state.pool).await?;
    let payload = match serde_json::from_str::<SystemsPayload>(&form.systems_payload) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!("Failed to parse systems maintenance payload: {err}");
            let rows = build_system_editor_rows(&existing_systems);
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some("systems".to_string()),
                    ..Default::default()
                },
                vec![
                    "Systems form payload was invalid. Reload the page and try again.".to_string(),
                ],
                Some(rows),
                None,
                None,
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let rows_for_errors = build_payload_system_rows(&payload);
    let validated = match validate_systems_payload(&payload, &existing_systems, &media_types) {
        Ok(rows) => rows,
        Err(errors) => {
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some("systems".to_string()),
                    ..Default::default()
                },
                errors,
                Some(rows_for_errors),
                None,
                None,
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let saved = match save_validated_systems(&state.pool, &validated, &existing_systems).await {
        Ok(saved) => saved,
        Err(err) => {
            tracing::error!("Failed to save systems: {err}");
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some("systems".to_string()),
                    ..Default::default()
                },
                vec!["Failed to save systems.".to_string()],
                Some(rows_for_errors),
                None,
                None,
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    Ok(redirect_with_message(
        "systems",
        "status",
        &format!("Saved {saved} system change(s)."),
    ))
}

async fn save_regions(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<RegionsForm>,
) -> AppResult<Response> {
    save_lookup_table(
        &state,
        user,
        &form.csrf_token,
        &form.regions_payload,
        LookupTable::Regions,
    )
    .await
}

async fn save_languages(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<LanguagesForm>,
) -> AppResult<Response> {
    save_lookup_table(
        &state,
        user,
        &form.csrf_token,
        &form.languages_payload,
        LookupTable::Languages,
    )
    .await
}

async fn save_lookup_table(
    state: &AppState,
    user: AuthenticatedUser,
    csrf_token: &str,
    payload_json: &str,
    table: LookupTable,
) -> AppResult<Response> {
    csrf::verify_token(&user, csrf_token)?;

    let flag_options = load_flag_options()?;
    let existing_rows = fetch_lookup_records(&state.pool, table).await?;
    let payload = match serde_json::from_str::<LookupPayload>(payload_json) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!(
                "Failed to parse {} maintenance payload: {err}",
                table.slug()
            );
            let rows = build_lookup_editor_rows(&existing_rows);
            let (region_rows, language_rows) = table.lookup_row_overrides(rows);
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some(table.slug().to_string()),
                    ..Default::default()
                },
                vec![format!(
                    "{} form payload was invalid. Reload the page and try again.",
                    table.plural_title()
                )],
                None,
                region_rows,
                language_rows,
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let rows_for_errors = build_payload_lookup_rows(&payload);
    let validated = match validate_lookup_payload(&payload, &existing_rows, &flag_options, table) {
        Ok(rows) => rows,
        Err(errors) => {
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some(table.slug().to_string()),
                    ..Default::default()
                },
                errors,
                None,
                table.region_rows_override(rows_for_errors.clone()),
                table.language_rows_override(rows_for_errors),
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let saved =
        match save_validated_lookup_rows(&state.pool, &validated, &existing_rows, table).await {
            Ok(saved) => saved,
            Err(err) => {
                tracing::error!("Failed to save {}: {err}", table.slug());
                let template = build_maintenance_template(
                    &state.pool,
                    user,
                    MaintenanceQuery {
                        tab: Some(table.slug().to_string()),
                        ..Default::default()
                    },
                    vec![format!("Failed to save {}.", table.plural_lower())],
                    None,
                    table.region_rows_override(rows_for_errors.clone()),
                    table.language_rows_override(rows_for_errors),
                )
                .await?;
                return Ok(Html(template.render().unwrap()).into_response());
            }
        };

    Ok(redirect_with_message(
        table.slug(),
        "status",
        &format!("Saved {saved} {} change(s).", table.singular_lower()),
    ))
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct MaintenanceMediaType {
    code: String,
    name: String,
}

#[derive(Clone, Debug)]
struct SystemEditorRow {
    original_code: String,
    code: String,
    system_type: String,
    manufacturer: String,
    name: String,
    short_name: String,
    media_types_csv: String,
    flags: Vec<SystemFlagOption>,
}

#[derive(Clone, Debug)]
struct SystemInputSizes {
    code: usize,
    system_type: usize,
    manufacturer: usize,
    name: usize,
    short_name: usize,
    media_types: usize,
}

#[derive(Clone, Debug)]
struct LookupEditorRow {
    original_code: String,
    code: String,
    name: String,
    flag_code: String,
    sort_order: String,
}

#[derive(Clone, Debug)]
struct LookupInputSizes {
    code: usize,
    name: usize,
    sort_order: usize,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct LookupRecord {
    code: String,
    name: String,
    flag_code: String,
    sort_order: i32,
}

#[derive(Clone, Debug)]
struct FlagOption {
    code: String,
}

#[derive(Clone, Debug)]
struct SystemFlagOption {
    field: &'static str,
    label: &'static str,
    checked: bool,
}

#[derive(Clone, Copy, Debug)]
struct SystemFlagDefinition {
    field: &'static str,
    label: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupTable {
    Regions,
    Languages,
}

impl LookupTable {
    fn slug(self) -> &'static str {
        match self {
            LookupTable::Regions => "regions",
            LookupTable::Languages => "languages",
        }
    }

    fn singular_title(self) -> &'static str {
        match self {
            LookupTable::Regions => "Region",
            LookupTable::Languages => "Language",
        }
    }

    fn plural_title(self) -> &'static str {
        match self {
            LookupTable::Regions => "Regions",
            LookupTable::Languages => "Languages",
        }
    }

    fn singular_lower(self) -> &'static str {
        match self {
            LookupTable::Regions => "region",
            LookupTable::Languages => "language",
        }
    }

    fn plural_lower(self) -> &'static str {
        match self {
            LookupTable::Regions => "regions",
            LookupTable::Languages => "languages",
        }
    }

    fn name_max_len(self) -> usize {
        match self {
            LookupTable::Regions => 32,
            LookupTable::Languages => 16,
        }
    }

    fn lookup_row_overrides(
        self,
        rows: Vec<LookupEditorRow>,
    ) -> (Option<Vec<LookupEditorRow>>, Option<Vec<LookupEditorRow>>) {
        match self {
            LookupTable::Regions => (Some(rows), None),
            LookupTable::Languages => (None, Some(rows)),
        }
    }

    fn region_rows_override(self, rows: Vec<LookupEditorRow>) -> Option<Vec<LookupEditorRow>> {
        match self {
            LookupTable::Regions => Some(rows),
            LookupTable::Languages => None,
        }
    }

    fn language_rows_override(self, rows: Vec<LookupEditorRow>) -> Option<Vec<LookupEditorRow>> {
        match self {
            LookupTable::Regions => None,
            LookupTable::Languages => Some(rows),
        }
    }

    fn insert_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => INSERT_REGION_SQL,
            LookupTable::Languages => INSERT_LANGUAGE_SQL,
        }
    }

    fn update_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => UPDATE_REGION_SQL,
            LookupTable::Languages => UPDATE_LANGUAGE_SQL,
        }
    }

    fn rename_junction_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => RENAME_DISC_REGIONS_SQL,
            LookupTable::Languages => RENAME_DISC_LANGUAGES_SQL,
        }
    }

    fn delete_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => DELETE_REGION_SQL,
            LookupTable::Languages => DELETE_LANGUAGE_SQL,
        }
    }
}

const SYSTEM_FLAGS: &[SystemFlagDefinition] = &[
    SystemFlagDefinition {
        field: "has_title_foreign",
        label: "Foreign Title",
    },
    SystemFlagDefinition {
        field: "has_disc_number",
        label: "Disc Number",
    },
    SystemFlagDefinition {
        field: "has_disc_title",
        label: "Disc Title",
    },
    SystemFlagDefinition {
        field: "has_serial",
        label: "Serial",
    },
    SystemFlagDefinition {
        field: "has_edition",
        label: "Edition",
    },
    SystemFlagDefinition {
        field: "has_barcode",
        label: "Barcode",
    },
    SystemFlagDefinition {
        field: "has_version",
        label: "Version",
    },
    SystemFlagDefinition {
        field: "has_exe_date",
        label: "EXE Date",
    },
    SystemFlagDefinition {
        field: "has_edc",
        label: "EDC",
    },
    SystemFlagDefinition {
        field: "has_disc_id",
        label: "Disc ID",
    },
    SystemFlagDefinition {
        field: "has_key",
        label: "Disc Key",
    },
    SystemFlagDefinition {
        field: "has_universal_hash",
        label: "Universal Hash",
    },
    SystemFlagDefinition {
        field: "has_protection",
        label: "Protection",
    },
    SystemFlagDefinition {
        field: "has_sector_ranges",
        label: "Sector Ranges",
    },
    SystemFlagDefinition {
        field: "has_sbi",
        label: "SBI",
    },
    SystemFlagDefinition {
        field: "has_pvd",
        label: "PVD",
    },
    SystemFlagDefinition {
        field: "has_header",
        label: "Header",
    },
    SystemFlagDefinition {
        field: "has_bca",
        label: "BCA",
    },
    SystemFlagDefinition {
        field: "has_sample_start",
        label: "Sample Start",
    },
    SystemFlagDefinition {
        field: "has_offset_extra",
        label: "Offset Extra",
    },
];

#[derive(Clone, Debug, Default, Deserialize)]
struct SystemsPayload {
    #[serde(default)]
    rows: Vec<SystemPayloadRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SystemPayloadRow {
    #[serde(default)]
    original_code: String,
    #[serde(default)]
    code: String,
    #[serde(default, rename = "type")]
    system_type: String,
    #[serde(default)]
    manufacturer: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    short_name: String,
    #[serde(default)]
    media_types: String,
    #[serde(default)]
    flags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedSystemRow {
    original_code: Option<String>,
    code: String,
    system_type: String,
    manufacturer: String,
    name: String,
    short_name: String,
    media_types: Vec<String>,
    flags: HashSet<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LookupPayload {
    #[serde(default)]
    rows: Vec<LookupPayloadRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LookupPayloadRow {
    #[serde(default)]
    original_code: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    flag_code: String,
    #[serde(default)]
    sort_order: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedLookupRow {
    original_code: Option<String>,
    code: String,
    name: String,
    flag_code: String,
    sort_order: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SystemSaveAction {
    Unchanged,
    Insert,
    Update,
    Rename,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupSaveAction {
    Unchanged,
    Insert,
    Update,
    Rename,
}

const INSERT_SYSTEM_SQL: &str = "
    INSERT INTO systems
        (code, type, manufacturer, name, short_name, media_types,
         has_title_foreign, has_disc_number, has_disc_title, has_serial,
         has_edition, has_barcode, has_version, has_exe_date, has_edc,
         has_disc_id, has_key, has_universal_hash, has_protection,
         has_sector_ranges, has_sbi, has_pvd, has_header, has_bca,
         has_sample_start, has_offset_extra, archives_dirty)
    VALUES
        ($1, $2, $3, $4, $5, $6,
         $7, $8, $9, $10,
         $11, $12, $13, $14, $15,
         $16, $17, $18, $19,
         $20, $21, $22, $23, $24,
         $25, $26, TRUE)";

const UPDATE_SYSTEM_SQL: &str = "
    UPDATE systems
    SET type = $2,
        manufacturer = $3,
        name = $4,
        short_name = $5,
        media_types = $6,
        has_title_foreign = $7,
        has_disc_number = $8,
        has_disc_title = $9,
        has_serial = $10,
        has_edition = $11,
        has_barcode = $12,
        has_version = $13,
        has_exe_date = $14,
        has_edc = $15,
        has_disc_id = $16,
        has_key = $17,
        has_universal_hash = $18,
        has_protection = $19,
        has_sector_ranges = $20,
        has_sbi = $21,
        has_pvd = $22,
        has_header = $23,
        has_bca = $24,
        has_sample_start = $25,
        has_offset_extra = $26,
        archives_dirty = TRUE
    WHERE code = $1";

const RENAME_DISCS_SYSTEM_SQL: &str = "UPDATE discs SET system_code = $1 WHERE system_code = $2";
const DELETE_SYSTEM_SQL: &str = "DELETE FROM systems WHERE code = $1";

const INSERT_REGION_SQL: &str =
    "INSERT INTO regions (code, name, flag_code, sort_order) VALUES ($1, $2, $3, $4)";
const UPDATE_REGION_SQL: &str =
    "UPDATE regions SET name = $2, flag_code = $3, sort_order = $4 WHERE code = $1";
const RENAME_DISC_REGIONS_SQL: &str =
    "UPDATE disc_regions SET region_code = $1 WHERE region_code = $2";
const DELETE_REGION_SQL: &str = "DELETE FROM regions WHERE code = $1";

const INSERT_LANGUAGE_SQL: &str =
    "INSERT INTO languages (code, name, flag_code, sort_order) VALUES ($1, $2, $3, $4)";
const UPDATE_LANGUAGE_SQL: &str =
    "UPDATE languages SET name = $2, flag_code = $3, sort_order = $4 WHERE code = $1";
const RENAME_DISC_LANGUAGES_SQL: &str =
    "UPDATE disc_languages SET language_code = $1 WHERE language_code = $2";
const DELETE_LANGUAGE_SQL: &str = "DELETE FROM languages WHERE code = $1";

async fn build_maintenance_template(
    pool: &sqlx::PgPool,
    user: AuthenticatedUser,
    query: MaintenanceQuery,
    maintenance_errors: Vec<String>,
    system_rows: Option<Vec<SystemEditorRow>>,
    region_rows: Option<Vec<LookupEditorRow>>,
    language_rows: Option<Vec<LookupEditorRow>>,
) -> AppResult<MaintenanceTemplate> {
    let system_rows = match system_rows {
        Some(rows) => rows,
        None => {
            let systems = disc_service::get_all_systems(pool).await?;
            build_system_editor_rows(&systems)
        }
    };
    let region_rows = match region_rows {
        Some(rows) => rows,
        None => {
            let regions = fetch_lookup_records(pool, LookupTable::Regions).await?;
            build_lookup_editor_rows(&regions)
        }
    };
    let language_rows = match language_rows {
        Some(rows) => rows,
        None => {
            let languages = fetch_lookup_records(pool, LookupTable::Languages).await?;
            build_lookup_editor_rows(&languages)
        }
    };
    let flag_options = load_flag_options()?;
    let backup_files = if user.role.can_admin() {
        list_backup_files(Path::new(BACKUP_DIR))
    } else {
        Vec::new()
    };

    Ok(maintenance_template(
        user,
        query,
        maintenance_errors,
        system_rows,
        region_rows,
        language_rows,
        flag_options,
        backup_files,
    ))
}

fn maintenance_template(
    user: AuthenticatedUser,
    query: MaintenanceQuery,
    maintenance_errors: Vec<String>,
    system_rows: Vec<SystemEditorRow>,
    region_rows: Vec<LookupEditorRow>,
    language_rows: Vec<LookupEditorRow>,
    flag_options: Vec<FlagOption>,
    backup_files: Vec<BackupFile>,
) -> MaintenanceTemplate {
    let active_tab = match query.tab.as_deref() {
        Some("systems") => "systems",
        Some("regions") => "regions",
        Some("languages") => "languages",
        Some("misc") => "misc",
        Some("backup") if user.role.can_admin() => "backup",
        _ => "general",
    };
    let system_input_sizes = system_input_sizes(&system_rows);
    let region_input_sizes = lookup_input_sizes(&region_rows);
    let language_input_sizes = lookup_input_sizes(&language_rows);
    let can_admin = user.role.can_admin();
    MaintenanceTemplate {
        current_user: Some(user),
        status_message: query.status.unwrap_or_default(),
        error_message: query.error.unwrap_or_default(),
        maintenance_errors,
        system_rows,
        region_rows,
        language_rows,
        flag_options,
        system_input_sizes,
        region_input_sizes,
        language_input_sizes,
        backup_files,
        can_admin,
        show_general: active_tab == "general",
        show_systems: active_tab == "systems",
        show_regions: active_tab == "regions",
        show_languages: active_tab == "languages",
        show_misc: active_tab == "misc",
        show_backup: active_tab == "backup",
    }
}

fn list_backup_files(directory: &Path) -> Vec<BackupFile> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(path = %directory.display(), "Could not list backups: {err}");
            return Vec::new();
        }
    };

    let mut backups = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let filename = entry.file_name().into_string().ok()?;
            let created_at = backup_timestamp(&filename)?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some(BackupFile {
                filename,
                created_at,
                size: format_file_size(metadata.len()),
            })
        })
        .collect::<Vec<_>>();
    backups.sort_by(|left, right| right.filename.cmp(&left.filename));
    backups
}

fn backup_timestamp(filename: &str) -> Option<String> {
    let timestamp = filename
        .strip_prefix(BACKUP_PREFIX)?
        .strip_suffix(BACKUP_SUFFIX)?;
    let bytes = timestamp.as_bytes();
    if bytes.len() != 16
        || bytes[8] != b'T'
        || bytes[15] != b'Z'
        || !bytes[..8].iter().all(u8::is_ascii_digit)
        || !bytes[9..15].iter().all(u8::is_ascii_digit)
    {
        return None;
    }

    Some(format!(
        "{}-{}-{} {}:{}:{} UTC",
        &timestamp[0..4],
        &timestamp[4..6],
        &timestamp[6..8],
        &timestamp[9..11],
        &timestamp[11..13],
        &timestamp[13..15],
    ))
}

fn format_file_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

async fn fetch_media_types(pool: &sqlx::PgPool) -> AppResult<Vec<MaintenanceMediaType>> {
    Ok(
        sqlx::query_as("SELECT code, name FROM media_types ORDER BY LOWER(name), code")
            .fetch_all(pool)
            .await?,
    )
}

async fn fetch_lookup_records(
    pool: &sqlx::PgPool,
    table: LookupTable,
) -> AppResult<Vec<LookupRecord>> {
    let sql = match table {
        LookupTable::Regions => {
            "SELECT TRIM(code)::TEXT AS code, name, flag_code::TEXT AS flag_code, sort_order \
             FROM regions ORDER BY sort_order, LOWER(name), code"
        }
        LookupTable::Languages => {
            "SELECT TRIM(code)::TEXT AS code, name, flag_code::TEXT AS flag_code, sort_order \
             FROM languages ORDER BY sort_order, LOWER(name), code"
        }
    };
    Ok(sqlx::query_as(sql).fetch_all(pool).await?)
}

fn load_flag_options() -> AppResult<Vec<FlagOption>> {
    let flag_dir = Path::new("static/flags");
    let entries = std::fs::read_dir(flag_dir).map_err(|err| {
        AppError::Internal(format!(
            "Failed to read flag directory {}: {err}",
            flag_dir.display()
        ))
    })?;
    let mut codes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            AppError::Internal(format!(
                "Failed to read flag directory entry {}: {err}",
                flag_dir.display()
            ))
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("svg") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            codes.push(stem.to_string());
        }
    }
    codes.sort();
    Ok(codes.into_iter().map(|code| FlagOption { code }).collect())
}

fn build_system_editor_rows(systems: &[System]) -> Vec<SystemEditorRow> {
    let mut rows: Vec<_> = systems.iter().map(system_editor_row).collect();
    rows.push(blank_system_editor_row());
    rows
}

fn system_editor_row(system: &System) -> SystemEditorRow {
    SystemEditorRow {
        original_code: system.code.clone(),
        code: system.code.clone(),
        system_type: system.system_type.clone(),
        manufacturer: system.manufacturer.clone(),
        name: system.name.clone(),
        short_name: system.short_name.clone(),
        media_types_csv: system.media_types.join(","),
        flags: flag_options_for_system(system),
    }
}

fn blank_system_editor_row() -> SystemEditorRow {
    SystemEditorRow {
        original_code: String::new(),
        code: String::new(),
        system_type: String::new(),
        manufacturer: String::new(),
        name: String::new(),
        short_name: String::new(),
        media_types_csv: String::new(),
        flags: flag_options_from_set(&HashSet::new()),
    }
}

fn build_payload_system_rows(payload: &SystemsPayload) -> Vec<SystemEditorRow> {
    let mut rows: Vec<_> = payload
        .rows
        .iter()
        .map(|row| {
            let flags: HashSet<String> = row
                .flags
                .iter()
                .map(|flag| flag.trim().to_string())
                .collect();
            SystemEditorRow {
                original_code: row.original_code.trim().to_string(),
                code: row.code.trim().to_string(),
                system_type: row.system_type.trim().to_string(),
                manufacturer: row.manufacturer.trim().to_string(),
                name: row.name.trim().to_string(),
                short_name: row.short_name.trim().to_string(),
                media_types_csv: row.media_types.trim().to_string(),
                flags: flag_options_from_set(&flags),
            }
        })
        .collect();
    if !rows.iter().any(|row| row.original_code.is_empty()) {
        rows.push(blank_system_editor_row());
    }
    rows
}

fn build_lookup_editor_rows(records: &[LookupRecord]) -> Vec<LookupEditorRow> {
    let mut rows: Vec<_> = records.iter().map(lookup_editor_row).collect();
    rows.push(blank_lookup_editor_row());
    rows
}

fn lookup_editor_row(record: &LookupRecord) -> LookupEditorRow {
    LookupEditorRow {
        original_code: record.code.trim().to_string(),
        code: record.code.trim().to_string(),
        name: record.name.clone(),
        flag_code: record.flag_code.trim().to_string(),
        sort_order: record.sort_order.to_string(),
    }
}

fn blank_lookup_editor_row() -> LookupEditorRow {
    LookupEditorRow {
        original_code: String::new(),
        code: String::new(),
        name: String::new(),
        flag_code: String::new(),
        sort_order: String::new(),
    }
}

fn build_payload_lookup_rows(payload: &LookupPayload) -> Vec<LookupEditorRow> {
    let mut rows: Vec<_> = payload
        .rows
        .iter()
        .map(|row| LookupEditorRow {
            original_code: row.original_code.trim().to_string(),
            code: row.code.trim().to_string(),
            name: row.name.trim().to_string(),
            flag_code: row.flag_code.trim().to_string(),
            sort_order: row.sort_order.trim().to_string(),
        })
        .collect();
    if !rows.iter().any(|row| row.original_code.is_empty()) {
        rows.push(blank_lookup_editor_row());
    }
    rows
}

fn system_input_sizes(rows: &[SystemEditorRow]) -> SystemInputSizes {
    SystemInputSizes {
        code: column_input_size(rows.iter().map(|row| row.code.as_str()), 4, 16),
        system_type: column_input_size(rows.iter().map(|row| row.system_type.as_str()), 4, 8),
        manufacturer: column_input_size(rows.iter().map(|row| row.manufacturer.as_str()), 12, 32),
        name: column_input_size(rows.iter().map(|row| row.name.as_str()), 12, 64),
        short_name: column_input_size(rows.iter().map(|row| row.short_name.as_str()), 10, 32),
        media_types: column_input_size(rows.iter().map(|row| row.media_types_csv.as_str()), 11, 80),
    }
}

fn lookup_input_sizes(rows: &[LookupEditorRow]) -> LookupInputSizes {
    LookupInputSizes {
        code: column_input_size(rows.iter().map(|row| row.code.as_str()), 4, 2),
        name: column_input_size(rows.iter().map(|row| row.name.as_str()), 12, 32),
        sort_order: column_input_size(rows.iter().map(|row| row.sort_order.as_str()), 5, 11),
    }
}

fn column_input_size<'a>(
    values: impl Iterator<Item = &'a str>,
    min_size: usize,
    max_size: usize,
) -> usize {
    values
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0)
        .max(min_size)
        .min(max_size)
}

fn flag_options_for_system(system: &System) -> Vec<SystemFlagOption> {
    SYSTEM_FLAGS
        .iter()
        .map(|flag| SystemFlagOption {
            field: flag.field,
            label: flag.label,
            checked: system_flag_value(system, flag.field),
        })
        .collect()
}

fn flag_options_from_set(flags: &HashSet<String>) -> Vec<SystemFlagOption> {
    SYSTEM_FLAGS
        .iter()
        .map(|flag| SystemFlagOption {
            field: flag.field,
            label: flag.label,
            checked: flags.contains(flag.field),
        })
        .collect()
}

fn system_flag_value(system: &System, field: &str) -> bool {
    match field {
        "has_title_foreign" => system.has_title_foreign,
        "has_disc_number" => system.has_disc_number,
        "has_disc_title" => system.has_disc_title,
        "has_serial" => system.has_serial,
        "has_edition" => system.has_edition,
        "has_barcode" => system.has_barcode,
        "has_version" => system.has_version,
        "has_exe_date" => system.has_exe_date,
        "has_edc" => system.has_edc,
        "has_disc_id" => system.has_disc_id,
        "has_key" => system.has_key,
        "has_universal_hash" => system.has_universal_hash,
        "has_protection" => system.has_protection,
        "has_sector_ranges" => system.has_sector_ranges,
        "has_sbi" => system.has_sbi,
        "has_pvd" => system.has_pvd,
        "has_header" => system.has_header,
        "has_bca" => system.has_bca,
        "has_sample_start" => system.has_sample_start,
        "has_offset_extra" => system.has_offset_extra,
        _ => false,
    }
}

fn validate_systems_payload(
    payload: &SystemsPayload,
    existing_systems: &[System],
    media_types: &[MaintenanceMediaType],
) -> Result<Vec<ValidatedSystemRow>, Vec<String>> {
    let existing_codes: HashSet<String> = existing_systems
        .iter()
        .map(|system| system.code.clone())
        .collect();
    let media_codes: HashSet<String> = media_types
        .iter()
        .map(|media_type| media_type.code.clone())
        .collect();
    let valid_flags: HashSet<&'static str> = SYSTEM_FLAGS.iter().map(|flag| flag.field).collect();
    let mut errors = Vec::new();
    let mut original_codes_seen = HashSet::new();
    let mut rows = Vec::new();

    for (index, row) in payload.rows.iter().enumerate() {
        let normalized = normalize_system_payload_row(row);
        if normalized.original_code.is_none() && normalized.is_blank() {
            continue;
        }

        let label = normalized.row_label(index + 1);
        if let Some(original_code) = &normalized.original_code {
            if !existing_codes.contains(original_code) {
                errors.push(format!("{label}: original system does not exist."));
            }
            if !original_codes_seen.insert(original_code.clone()) {
                errors.push(format!("{label}: original system appears more than once."));
            }
        }

        validate_required_and_lengths(&normalized, &label, &mut errors);
        validate_media_types(&normalized, &label, &media_codes, &mut errors);
        validate_flags(&normalized, &label, &valid_flags, &mut errors);
        rows.push(normalized);
    }

    validate_code_collisions(&rows, &existing_codes, &mut errors);

    if errors.is_empty() {
        Ok(rows)
    } else {
        Err(errors)
    }
}

fn normalize_system_payload_row(row: &SystemPayloadRow) -> ValidatedSystemRow {
    let flags = row
        .flags
        .iter()
        .map(|flag| flag.trim().to_string())
        .filter(|flag| !flag.is_empty())
        .collect();
    let media_types = row
        .media_types
        .split(',')
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty())
        .collect();
    ValidatedSystemRow {
        original_code: non_empty_trimmed(&row.original_code),
        code: row.code.trim().to_string(),
        system_type: row.system_type.trim().to_string(),
        manufacturer: row.manufacturer.trim().to_string(),
        name: row.name.trim().to_string(),
        short_name: row.short_name.trim().to_string(),
        media_types,
        flags,
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl ValidatedSystemRow {
    fn is_blank(&self) -> bool {
        self.code.is_empty()
            && self.system_type.is_empty()
            && self.manufacturer.is_empty()
            && self.name.is_empty()
            && self.short_name.is_empty()
            && self.media_types.is_empty()
            && self.flags.is_empty()
    }

    fn row_label(&self, index: usize) -> String {
        if let Some(original_code) = &self.original_code {
            format!("System {original_code}")
        } else if self.code.is_empty() {
            format!("New system row {index}")
        } else {
            format!("New system {}", self.code)
        }
    }

    fn flag(&self, field: &str) -> bool {
        self.flags.contains(field)
    }
}

fn validate_required_and_lengths(row: &ValidatedSystemRow, label: &str, errors: &mut Vec<String>) {
    if row.code.is_empty() {
        errors.push(format!("{label}: code is required."));
    }
    if row.name.is_empty() {
        errors.push(format!("{label}: name is required."));
    }
    validate_length(label, "code", &row.code, 16, errors);
    validate_length(label, "type", &row.system_type, 8, errors);
    validate_length(label, "manufacturer", &row.manufacturer, 32, errors);
    validate_length(label, "name", &row.name, 64, errors);
    validate_length(label, "short_name", &row.short_name, 32, errors);
}

fn validate_length(
    label: &str,
    field: &str,
    value: &str,
    max_len: usize,
    errors: &mut Vec<String>,
) {
    if value.chars().count() > max_len {
        errors.push(format!(
            "{label}: {field} must be {max_len} characters or less."
        ));
    }
}

fn validate_media_types(
    row: &ValidatedSystemRow,
    label: &str,
    media_codes: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    if row.media_types.is_empty() {
        errors.push(format!("{label}: at least one media type is required."));
        return;
    }

    let mut seen = HashSet::new();
    for code in &row.media_types {
        if !media_codes.contains(code) {
            errors.push(format!("{label}: media type {code} does not exist."));
        }
        if !seen.insert(code) {
            errors.push(format!(
                "{label}: media type {code} is selected more than once."
            ));
        }
    }
}

fn validate_flags(
    row: &ValidatedSystemRow,
    label: &str,
    valid_flags: &HashSet<&'static str>,
    errors: &mut Vec<String>,
) {
    for flag in &row.flags {
        if !valid_flags.contains(flag.as_str()) {
            errors.push(format!("{label}: capability flag {flag} does not exist."));
        }
    }
}

fn validate_code_collisions(
    rows: &[ValidatedSystemRow],
    existing_codes: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    let mut final_codes: HashMap<&str, &ValidatedSystemRow> = HashMap::new();
    for row in rows {
        if row.code.is_empty() {
            continue;
        }
        if let Some(previous) = final_codes.insert(row.code.as_str(), row) {
            errors.push(format!(
                "{}: code duplicates {}.",
                row.row_label(0),
                previous.row_label(0)
            ));
        }

        match &row.original_code {
            Some(original_code) if original_code == &row.code => {}
            Some(_) | None => {
                if existing_codes.contains(&row.code) {
                    errors.push(format!(
                        "{}: code {} already exists.",
                        row.row_label(0),
                        row.code
                    ));
                }
            }
        }
    }
}

fn validate_lookup_payload(
    payload: &LookupPayload,
    existing_records: &[LookupRecord],
    flag_options: &[FlagOption],
    table: LookupTable,
) -> Result<Vec<ValidatedLookupRow>, Vec<String>> {
    let existing_codes: HashSet<String> = existing_records
        .iter()
        .map(|record| record.code.trim().to_string())
        .collect();
    let flag_codes: HashSet<String> = flag_options
        .iter()
        .map(|option| option.code.clone())
        .collect();
    let mut errors = Vec::new();
    let mut original_codes_seen = HashSet::new();
    let mut rows = Vec::new();

    for (index, row) in payload.rows.iter().enumerate() {
        let normalized = normalize_lookup_payload_row(row);
        if normalized.original_code.is_none() && lookup_payload_row_is_blank(row) {
            continue;
        }

        let label = normalized.row_label(table, index + 1);
        if let Some(original_code) = &normalized.original_code {
            if !existing_codes.contains(original_code) {
                errors.push(format!(
                    "{label}: original {} does not exist.",
                    table.singular_lower()
                ));
            }
            if !original_codes_seen.insert(original_code.clone()) {
                errors.push(format!(
                    "{label}: original {} appears more than once.",
                    table.singular_lower()
                ));
            }
        }

        validate_lookup_required_and_lengths(&normalized, &label, table, &mut errors);
        validate_lookup_flag_code(&normalized, &label, &flag_codes, &mut errors);
        validate_lookup_sort_order_text(row, &label, &mut errors);
        rows.push(normalized);
    }

    validate_lookup_code_collisions(&rows, &existing_codes, table, &mut errors);

    if errors.is_empty() {
        Ok(rows)
    } else {
        Err(errors)
    }
}

fn normalize_lookup_payload_row(row: &LookupPayloadRow) -> ValidatedLookupRow {
    ValidatedLookupRow {
        original_code: non_empty_trimmed(&row.original_code),
        code: row.code.trim().to_string(),
        name: row.name.trim().to_string(),
        flag_code: row.flag_code.trim().to_string(),
        sort_order: row.sort_order.trim().parse::<i32>().unwrap_or_default(),
    }
}

fn lookup_payload_row_is_blank(row: &LookupPayloadRow) -> bool {
    row.code.trim().is_empty()
        && row.name.trim().is_empty()
        && row.flag_code.trim().is_empty()
        && row.sort_order.trim().is_empty()
}

impl ValidatedLookupRow {
    fn is_blank(&self) -> bool {
        self.code.is_empty()
            && self.name.is_empty()
            && self.flag_code.is_empty()
            && self.sort_order == 0
    }

    fn row_label(&self, table: LookupTable, index: usize) -> String {
        if let Some(original_code) = &self.original_code {
            format!("{} {original_code}", table.singular_title())
        } else if self.code.is_empty() {
            format!("New {} row {index}", table.singular_lower())
        } else {
            format!("New {} {}", table.singular_lower(), self.code)
        }
    }
}

fn validate_lookup_required_and_lengths(
    row: &ValidatedLookupRow,
    label: &str,
    table: LookupTable,
    errors: &mut Vec<String>,
) {
    if row.code.is_empty() {
        errors.push(format!("{label}: code is required."));
    } else if row.code.chars().count() != 2 {
        errors.push(format!("{label}: code must be exactly 2 characters."));
    }
    if row.name.is_empty() {
        errors.push(format!("{label}: name is required."));
    }
    if row.flag_code.is_empty() {
        errors.push(format!("{label}: flag_code is required."));
    }
    validate_length(label, "name", &row.name, table.name_max_len(), errors);
    validate_length(label, "flag_code", &row.flag_code, 16, errors);
}

fn validate_lookup_flag_code(
    row: &ValidatedLookupRow,
    label: &str,
    flag_codes: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    if !row.flag_code.is_empty() && !flag_codes.contains(&row.flag_code) {
        errors.push(format!(
            "{label}: flag_code {} does not match a static flag SVG.",
            row.flag_code
        ));
    }
}

fn validate_lookup_sort_order_text(row: &LookupPayloadRow, label: &str, errors: &mut Vec<String>) {
    let sort_order = row.sort_order.trim();
    if sort_order.is_empty() {
        errors.push(format!("{label}: sort_order is required."));
    } else if sort_order.parse::<i32>().is_err() {
        errors.push(format!("{label}: sort_order must be a valid integer."));
    }
}

fn validate_lookup_code_collisions(
    rows: &[ValidatedLookupRow],
    existing_codes: &HashSet<String>,
    table: LookupTable,
    errors: &mut Vec<String>,
) {
    let mut final_codes: HashMap<&str, &ValidatedLookupRow> = HashMap::new();
    for row in rows {
        if row.code.is_empty() {
            continue;
        }
        if let Some(previous) = final_codes.insert(row.code.as_str(), row) {
            errors.push(format!(
                "{}: code duplicates {}.",
                row.row_label(table, 0),
                previous.row_label(table, 0)
            ));
        }

        match &row.original_code {
            Some(original_code) if original_code == &row.code => {}
            Some(_) | None => {
                if existing_codes.contains(&row.code) {
                    errors.push(format!(
                        "{}: code {} already exists.",
                        row.row_label(table, 0),
                        row.code
                    ));
                }
            }
        }
    }
}

async fn save_validated_systems(
    pool: &sqlx::PgPool,
    rows: &[ValidatedSystemRow],
    existing_systems: &[System],
) -> AppResult<usize> {
    let existing_by_code: HashMap<String, System> = existing_systems
        .iter()
        .map(|system| (system.code.clone(), system.clone()))
        .collect();
    let mut saved = 0;
    let mut tx = pool.begin().await?;

    for row in rows {
        match system_save_action(row, &existing_by_code) {
            SystemSaveAction::Unchanged => {}
            SystemSaveAction::Insert => {
                insert_system(&mut tx, row).await?;
                saved += 1;
            }
            SystemSaveAction::Update => {
                update_system(&mut tx, row).await?;
                saved += 1;
            }
            SystemSaveAction::Rename => {
                let old_code = row.original_code.as_deref().unwrap_or_default();
                insert_system(&mut tx, row).await?;
                sqlx::query(RENAME_DISCS_SYSTEM_SQL)
                    .bind(&row.code)
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(DELETE_SYSTEM_SQL)
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                saved += 1;
            }
        }
    }

    tx.commit().await?;
    Ok(saved)
}

fn system_save_action(
    row: &ValidatedSystemRow,
    existing_by_code: &HashMap<String, System>,
) -> SystemSaveAction {
    let Some(original_code) = &row.original_code else {
        return SystemSaveAction::Insert;
    };
    let Some(existing) = existing_by_code.get(original_code) else {
        return SystemSaveAction::Insert;
    };
    if original_code != &row.code {
        return SystemSaveAction::Rename;
    }
    if system_matches_row(existing, row) {
        SystemSaveAction::Unchanged
    } else {
        SystemSaveAction::Update
    }
}

fn system_matches_row(system: &System, row: &ValidatedSystemRow) -> bool {
    system.code == row.code
        && system.system_type == row.system_type
        && system.manufacturer == row.manufacturer
        && system.name == row.name
        && system.short_name == row.short_name
        && system.media_types == row.media_types
        && SYSTEM_FLAGS
            .iter()
            .all(|flag| system_flag_value(system, flag.field) == row.flag(flag.field))
}

async fn insert_system(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedSystemRow,
) -> Result<(), sqlx::Error> {
    bind_system_fields(sqlx::query(INSERT_SYSTEM_SQL), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn update_system(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedSystemRow,
) -> Result<(), sqlx::Error> {
    bind_system_fields(sqlx::query(UPDATE_SYSTEM_SQL), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn bind_system_fields<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    row: &'q ValidatedSystemRow,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(&row.code)
        .bind(&row.system_type)
        .bind(&row.manufacturer)
        .bind(&row.name)
        .bind(&row.short_name)
        .bind(&row.media_types)
        .bind(row.flag("has_title_foreign"))
        .bind(row.flag("has_disc_number"))
        .bind(row.flag("has_disc_title"))
        .bind(row.flag("has_serial"))
        .bind(row.flag("has_edition"))
        .bind(row.flag("has_barcode"))
        .bind(row.flag("has_version"))
        .bind(row.flag("has_exe_date"))
        .bind(row.flag("has_edc"))
        .bind(row.flag("has_disc_id"))
        .bind(row.flag("has_key"))
        .bind(row.flag("has_universal_hash"))
        .bind(row.flag("has_protection"))
        .bind(row.flag("has_sector_ranges"))
        .bind(row.flag("has_sbi"))
        .bind(row.flag("has_pvd"))
        .bind(row.flag("has_header"))
        .bind(row.flag("has_bca"))
        .bind(row.flag("has_sample_start"))
        .bind(row.flag("has_offset_extra"))
}

async fn save_validated_lookup_rows(
    pool: &sqlx::PgPool,
    rows: &[ValidatedLookupRow],
    existing_records: &[LookupRecord],
    table: LookupTable,
) -> AppResult<usize> {
    let existing_by_code: HashMap<String, LookupRecord> = existing_records
        .iter()
        .map(|record| (record.code.trim().to_string(), record.clone()))
        .collect();
    let mut saved = 0;
    let mut tx = pool.begin().await?;

    for row in rows {
        match lookup_save_action(row, &existing_by_code) {
            LookupSaveAction::Unchanged => {}
            LookupSaveAction::Insert => {
                insert_lookup_row(&mut tx, row, table).await?;
                saved += 1;
            }
            LookupSaveAction::Update => {
                update_lookup_row(&mut tx, row, table).await?;
                saved += 1;
            }
            LookupSaveAction::Rename => {
                let old_code = row.original_code.as_deref().unwrap_or_default();
                insert_lookup_row(&mut tx, row, table).await?;
                sqlx::query(table.rename_junction_sql())
                    .bind(&row.code)
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(table.delete_sql())
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                saved += 1;
            }
        }
    }

    tx.commit().await?;
    Ok(saved)
}

fn lookup_save_action(
    row: &ValidatedLookupRow,
    existing_by_code: &HashMap<String, LookupRecord>,
) -> LookupSaveAction {
    let Some(original_code) = &row.original_code else {
        return LookupSaveAction::Insert;
    };
    let Some(existing) = existing_by_code.get(original_code) else {
        return LookupSaveAction::Insert;
    };
    if original_code != &row.code {
        return LookupSaveAction::Rename;
    }
    if lookup_record_matches_row(existing, row) {
        LookupSaveAction::Unchanged
    } else {
        LookupSaveAction::Update
    }
}

fn lookup_record_matches_row(record: &LookupRecord, row: &ValidatedLookupRow) -> bool {
    record.code.trim() == row.code
        && record.name == row.name
        && record.flag_code.trim() == row.flag_code
        && record.sort_order == row.sort_order
}

async fn insert_lookup_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedLookupRow,
    table: LookupTable,
) -> Result<(), sqlx::Error> {
    bind_lookup_fields(sqlx::query(table.insert_sql()), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn update_lookup_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedLookupRow,
    table: LookupTable,
) -> Result<(), sqlx::Error> {
    bind_lookup_fields(sqlx::query(table.update_sql()), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn bind_lookup_fields<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    row: &'q ValidatedLookupRow,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(&row.code)
        .bind(&row.name)
        .bind(&row.flag_code)
        .bind(row.sort_order)
}

async fn rebuild_cue(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<CsrfForm>,
) -> crate::error::AppResult<Response> {
    csrf::verify_form(&user, &form)?;

    Ok(match disc_service::regenerate_all_cue_entries(&state.pool).await {
        Ok(summary) => redirect_with_message(
            "misc",
            "status",
            &format!(
                "Rebuilt CUE for {} disc(s): {} active, {} cue text update(s), {} file metadata upsert(s), {} file metadata delete(s), {} unchanged.",
                summary.total,
                summary.active,
                summary.updated_cues,
                summary.upserted_file_entries,
                summary.deleted_file_entries,
                summary.skipped,
            ),
        ),
        Err(err) => {
            tracing::error!("Failed to rebuild database cue: {err}");
            redirect_with_message("misc", "error", "Failed to rebuild database cue.")
        }
    })
}

async fn trigger_archive_generation(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<CsrfForm>,
) -> crate::error::AppResult<Response> {
    csrf::verify_form(&user, &form)?;

    Ok(
        match archive_service::mark_all_system_archives_dirty(&state.pool).await {
            Ok(count) => redirect_with_message(
                "misc",
                "status",
                &format!("Triggered archive generation for {count} system(s)."),
            ),
            Err(err) => {
                tracing::error!("Failed to trigger archive generation: {err}");
                redirect_with_message("misc", "error", "Failed to trigger archive generation.")
            }
        },
    )
}

fn redirect_with_message(tab: &str, param: &str, message: &str) -> Response {
    let location = format!(
        "/maintenance?tab={}&{param}={}",
        urlencoding::encode(tab),
        urlencoding::encode(message)
    );
    Redirect::to(&location).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::middleware::AuthenticatedUser;
    use crate::db::models::UserRole;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    #[derive(Template)]
    #[template(
        source = "{% extends \"base.html\" %}{% block title %}Test{% endblock %}{% block content %}{% endblock %}",
        ext = "html"
    )]
    struct BaseMenuTemplate {
        current_user: Option<AuthenticatedUser>,
    }
    impl SiteConfig for BaseMenuTemplate {}

    fn auth_user(role: UserRole) -> AuthenticatedUser {
        AuthenticatedUser {
            id: 1,
            username: "tester".to_string(),
            role,
            csrf_token: "test-csrf-token".to_string(),
            avatar_url: None,
        }
    }

    fn render_menu(role: UserRole) -> String {
        BaseMenuTemplate {
            current_user: Some(auth_user(role)),
        }
        .render()
        .unwrap()
    }

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
            transliteration: Arc::new(
                crate::transliteration::TransliterationRegistry::new().unwrap(),
            ),
        }
    }

    fn test_media_types() -> Vec<MaintenanceMediaType> {
        vec![
            MaintenanceMediaType {
                code: "cd".to_string(),
                name: "CD".to_string(),
            },
            MaintenanceMediaType {
                code: "dvd5".to_string(),
                name: "DVD-5".to_string(),
            },
        ]
    }

    fn test_system(code: &str, name: &str) -> System {
        System {
            code: code.to_string(),
            system_type: String::new(),
            manufacturer: "Example".to_string(),
            name: name.to_string(),
            short_name: code.to_string(),
            media_types: vec!["cd".to_string()],
            has_exe_date: false,
            has_sbi: false,
            has_pvd: false,
            has_edc: false,
            has_disc_id: false,
            has_key: false,
            has_universal_hash: false,
            has_title_foreign: false,
            has_disc_title: false,
            has_disc_number: false,
            has_serial: false,
            has_barcode: false,
            has_version: false,
            has_edition: false,
            has_protection: false,
            has_sector_ranges: false,
            has_header: false,
            has_bca: false,
            has_sample_start: false,
            has_offset_extra: false,
            archives_dirty: false,
        }
    }

    fn test_lookup_record(
        code: &str,
        name: &str,
        flag_code: &str,
        sort_order: i32,
    ) -> LookupRecord {
        LookupRecord {
            code: code.to_string(),
            name: name.to_string(),
            flag_code: flag_code.to_string(),
            sort_order,
        }
    }

    fn test_flag_options() -> Vec<FlagOption> {
        vec![
            FlagOption {
                code: "gb".to_string(),
            },
            FlagOption {
                code: "gb-eng".to_string(),
            },
            FlagOption {
                code: "jp".to_string(),
            },
            FlagOption {
                code: "us".to_string(),
            },
        ]
    }

    fn maintenance_template_for_role(
        role: UserRole,
        tab: Option<&str>,
        backup_files: Vec<BackupFile>,
    ) -> MaintenanceTemplate {
        let systems = vec![test_system("SYS", "System")];
        let system_rows = build_system_editor_rows(&systems);
        let region_rows = build_lookup_editor_rows(&[test_lookup_record("us", "USA", "us", 1)]);
        let language_rows =
            build_lookup_editor_rows(&[test_lookup_record("en", "English", "gb", 1)]);
        maintenance_template(
            auth_user(role),
            MaintenanceQuery {
                tab: tab.map(str::to_string),
                ..Default::default()
            },
            Vec::new(),
            system_rows,
            region_rows,
            language_rows,
            test_flag_options(),
            backup_files,
        )
    }

    fn maintenance_template_for_tests(show_misc: bool) -> MaintenanceTemplate {
        maintenance_template_for_role(UserRole::Moderator, show_misc.then_some("misc"), Vec::new())
    }

    fn payload_row(original_code: &str, code: &str, name: &str) -> SystemPayloadRow {
        SystemPayloadRow {
            original_code: original_code.to_string(),
            code: code.to_string(),
            system_type: String::new(),
            manufacturer: "Example".to_string(),
            name: name.to_string(),
            short_name: code.to_string(),
            media_types: "cd".to_string(),
            flags: Vec::new(),
        }
    }

    fn lookup_payload_row(
        original_code: &str,
        code: &str,
        name: &str,
        flag_code: &str,
        sort_order: &str,
    ) -> LookupPayloadRow {
        LookupPayloadRow {
            original_code: original_code.to_string(),
            code: code.to_string(),
            name: name.to_string(),
            flag_code: flag_code.to_string(),
            sort_order: sort_order.to_string(),
        }
    }

    fn validation_errors(payload: SystemsPayload) -> Vec<String> {
        validate_systems_payload(
            &payload,
            &[test_system("SYS", "System"), test_system("OTHER", "Other")],
            &test_media_types(),
        )
        .unwrap_err()
    }

    fn lookup_records(table: LookupTable) -> Vec<LookupRecord> {
        match table {
            LookupTable::Regions => vec![
                test_lookup_record("us", "USA", "us", 1),
                test_lookup_record("jp", "Japan", "jp", 2),
            ],
            LookupTable::Languages => vec![
                test_lookup_record("en", "English", "gb", 1),
                test_lookup_record("ja", "Japanese", "jp", 2),
            ],
        }
    }

    fn lookup_validation_errors(table: LookupTable, payload: LookupPayload) -> Vec<String> {
        validate_lookup_payload(
            &payload,
            &lookup_records(table),
            &test_flag_options(),
            table,
        )
        .unwrap_err()
    }

    fn assert_error_contains(errors: &[String], expected: &str) {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "expected an error containing {expected:?}, got {errors:?}"
        );
    }

    #[test]
    fn user_menu_removes_settings_for_all_roles() {
        for role in [
            UserRole::User,
            UserRole::UserPlus,
            UserRole::Moderator,
            UserRole::Admin,
        ] {
            let html = render_menu(role);
            assert!(!html.contains("Settings"));
        }
    }

    #[test]
    fn user_menu_shows_maintenance_only_to_moderators_and_admins() {
        for role in [UserRole::User, UserRole::UserPlus] {
            let html = render_menu(role);
            assert!(!html.contains(r#"<a href="/maintenance">Maintenance</a>"#));
        }

        for role in [UserRole::Moderator, UserRole::Admin] {
            let html = render_menu(role);
            assert!(html.contains(r#"<a href="/maintenance">Maintenance</a>"#));
        }
    }

    #[test]
    fn user_menu_places_maintenance_above_logout() {
        let html = render_menu(UserRole::Moderator);
        let maintenance_pos = html
            .find(r#"<a href="/maintenance">Maintenance</a>"#)
            .unwrap();
        let logout_pos = html.find(r#"action="/logout""#).unwrap();

        assert!(maintenance_pos < logout_pos);
    }

    #[test]
    fn logged_in_base_template_emits_csrf_meta_and_logout_field() {
        let html = render_menu(UserRole::User);

        assert!(html.contains(r#"<meta name="csrf-token" content="test-csrf-token">"#));
        assert!(html.contains(r#"<input type="hidden" name="_csrf" value="test-csrf-token">"#));
    }

    #[test]
    fn maintenance_forms_include_csrf_fields() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        assert_eq!(
            html.matches(r#"name="_csrf" value="test-csrf-token""#)
                .count(),
            6
        );
        assert!(html.contains(r#"action="/maintenance/systems""#));
        assert!(html.contains(r#"action="/maintenance/regions""#));
        assert!(html.contains(r#"action="/maintenance/languages""#));
        assert!(html.contains(r#"action="/maintenance/rebuild-cue""#));
        assert!(html.contains(r#"action="/maintenance/trigger-archive-generation""#));
    }

    #[test]
    fn maintenance_tabs_place_systems_before_miscellaneous() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        let systems_tab = html.find(r#"href="/maintenance?tab=systems""#).unwrap();
        let regions_tab = html.find(r#"href="/maintenance?tab=regions""#).unwrap();
        let languages_tab = html.find(r#"href="/maintenance?tab=languages""#).unwrap();
        let misc_tab = html.find(r#"href="/maintenance?tab=misc""#).unwrap();
        let systems_panel = html.find(r#"id="maintenance-systems-panel""#).unwrap();
        let regions_panel = html.find(r#"id="maintenance-regions-panel""#).unwrap();
        let languages_panel = html.find(r#"id="maintenance-languages-panel""#).unwrap();
        let misc_panel = html.find(r#"id="maintenance-misc-panel""#).unwrap();

        assert!(systems_tab < misc_tab);
        assert!(systems_tab < regions_tab);
        assert!(regions_tab < languages_tab);
        assert!(languages_tab < misc_tab);
        assert!(systems_panel < misc_panel);
        assert!(systems_panel < regions_panel);
        assert!(regions_panel < languages_panel);
        assert!(languages_panel < misc_panel);
    }

    #[test]
    fn backup_tab_is_visible_only_to_admins() {
        let moderator = maintenance_template_for_role(UserRole::Moderator, None, Vec::new())
            .render()
            .unwrap();
        assert!(!moderator.contains(r#"href="/maintenance?tab=backup""#));
        assert!(!moderator.contains(r#"id="maintenance-backup-panel""#));

        let admin = maintenance_template_for_role(UserRole::Admin, Some("backup"), Vec::new())
            .render()
            .unwrap();
        assert!(admin.contains(r#"href="/maintenance?tab=backup" class="active""#));
        assert!(admin.contains(r#"id="maintenance-backup-panel""#));
    }

    #[test]
    fn backup_timestamp_accepts_only_completed_backup_names() {
        assert_eq!(
            backup_timestamp("vgindex-backup-20260703T060000Z.tar.gz").as_deref(),
            Some("2026-07-03 06:00:00 UTC")
        );
        assert!(backup_timestamp(".vgindex-backup-20260703T060000Z.tar.gz.partial").is_none());
        assert!(backup_timestamp("vgindex-backup-20260703.tar.gz").is_none());
        assert!(backup_timestamp("../vgindex-backup-20260703T060000Z.tar.gz").is_none());
    }

    #[test]
    fn backup_listing_sorts_newest_first_and_ignores_partial_files() {
        let directory =
            std::env::temp_dir().join(format!("vgindex-backups-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("vgindex-backup-20260702T060000Z.tar.gz"),
            b"old",
        )
        .unwrap();
        std::fs::write(
            directory.join("vgindex-backup-20260703T060000Z.tar.gz"),
            b"new",
        )
        .unwrap();
        std::fs::write(
            directory.join(".vgindex-backup-20260704T060000Z.tar.gz.partial"),
            b"partial",
        )
        .unwrap();

        let backups = list_backup_files(&directory);
        assert_eq!(backups.len(), 2);
        assert_eq!(
            backups[0].filename,
            "vgindex-backup-20260703T060000Z.tar.gz"
        );
        assert_eq!(
            backups[1].filename,
            "vgindex-backup-20260702T060000Z.tar.gz"
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn maintenance_systems_form_includes_two_line_rows_and_csv_media_field() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        assert!(html.contains(r#"name="systems_payload""#));
        assert!(html.contains("maintenance-system-text-row"));
        assert!(html.contains("maintenance-system-flags-row"));
        assert!(html.contains(r#"data-system-field="code""#));
        assert!(html.contains(r#"data-system-field="media_types""#));
        assert!(html.contains(r#"maxlength="16""#));
        assert!(html.contains(r#"data-system-flag="has_title_foreign""#));
        assert!(!html.contains("Add System"));
        assert!(!html.contains("<details"));
        assert!(!html.contains(r#"data-media-list"#));
    }

    #[test]
    fn maintenance_regions_and_languages_forms_include_one_line_rows_and_flag_options() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        assert!(html.contains(r#"name="regions_payload""#));
        assert!(html.contains(r#"name="languages_payload""#));
        assert!(html.contains(r#"class="maintenance-simple-row""#));
        assert!(html.contains(r#"data-maintenance-field="flag_code""#));
        assert!(html.contains(r#"data-flag-select"#));
        assert!(html.contains(r#"data-flag-preview"#));
        assert!(html.contains(r#"/static/flags/us.svg"#));
        assert!(html.contains(r#"/static/flags/gb.svg"#));
        assert!(html.contains(r#"<option value="gb-eng""#));
        assert!(html.contains(r#"maxlength="2""#));
        assert!(html.contains(r#"maxlength="32""#));
        assert!(html.contains(r#"maxlength="16""#));
    }

    #[test]
    fn systems_validation_ignores_blank_add_row() {
        let payload = SystemsPayload {
            rows: vec![SystemPayloadRow::default()],
        };

        let rows = validate_systems_payload(
            &payload,
            &[test_system("SYS", "System")],
            &test_media_types(),
        )
        .unwrap();

        assert!(rows.is_empty());
    }

    #[test]
    fn systems_validation_rejects_missing_required_fields() {
        let mut row = payload_row("", "", "");
        row.media_types = String::new();
        row.system_type = "Console".to_string();
        let errors = validation_errors(SystemsPayload { rows: vec![row] });

        assert_error_contains(&errors, "code is required");
        assert_error_contains(&errors, "name is required");
        assert_error_contains(&errors, "at least one media type is required");
    }

    #[test]
    fn systems_validation_rejects_invalid_and_duplicate_media() {
        let mut row = payload_row("", "NEW", "New System");
        row.media_types = "cd, bad, cd".to_string();
        let errors = validation_errors(SystemsPayload { rows: vec![row] });

        assert_error_contains(&errors, "media type bad does not exist");
        assert_error_contains(&errors, "media type cd is selected more than once");
    }

    #[test]
    fn systems_validation_rejects_duplicate_codes() {
        let errors = validation_errors(SystemsPayload {
            rows: vec![
                payload_row("", "NEW", "New System"),
                payload_row("", "NEW", "New System 2"),
            ],
        });

        assert_error_contains(&errors, "code duplicates");
    }

    #[test]
    fn systems_validation_rejects_rename_collision() {
        let errors = validation_errors(SystemsPayload {
            rows: vec![payload_row("SYS", "OTHER", "Renamed System")],
        });

        assert_error_contains(&errors, "code OTHER already exists");
    }

    #[test]
    fn lookup_validation_ignores_blank_add_row() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let payload = LookupPayload {
                rows: vec![LookupPayloadRow::default()],
            };
            let rows = validate_lookup_payload(
                &payload,
                &lookup_records(table),
                &test_flag_options(),
                table,
            )
            .unwrap();

            assert!(rows.is_empty());
        }
    }

    #[test]
    fn lookup_validation_rejects_missing_required_fields_and_invalid_sort_order() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let row = lookup_payload_row("", "", "", "", "abc");
            let errors = lookup_validation_errors(table, LookupPayload { rows: vec![row] });

            assert_error_contains(&errors, "code is required");
            assert_error_contains(&errors, "name is required");
            assert_error_contains(&errors, "flag_code is required");
            assert_error_contains(&errors, "sort_order must be a valid integer");
        }
    }

    #[test]
    fn lookup_validation_rejects_duplicate_codes_invalid_flags_and_rename_collisions() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let errors = lookup_validation_errors(
                table,
                LookupPayload {
                    rows: vec![
                        lookup_payload_row("", "zz", "Zulu", "missing", "1"),
                        lookup_payload_row("", "zz", "Zulu 2", "us", "2"),
                    ],
                },
            );

            assert_error_contains(&errors, "flag_code missing does not match");
            assert_error_contains(&errors, "code duplicates");
        }

        let region_errors = lookup_validation_errors(
            LookupTable::Regions,
            LookupPayload {
                rows: vec![lookup_payload_row("us", "jp", "Renamed", "jp", "1")],
            },
        );
        assert_error_contains(&region_errors, "code jp already exists");

        let language_errors = lookup_validation_errors(
            LookupTable::Languages,
            LookupPayload {
                rows: vec![lookup_payload_row("en", "ja", "Renamed", "jp", "1")],
            },
        );
        assert_error_contains(&language_errors, "code ja already exists");
    }

    #[test]
    fn lookup_validation_enforces_table_lengths() {
        let region_errors = lookup_validation_errors(
            LookupTable::Regions,
            LookupPayload {
                rows: vec![lookup_payload_row(
                    "",
                    "abc",
                    "A name that is definitely longer than thirty two characters",
                    "gb-eng-too-long-for-the-column",
                    "1",
                )],
            },
        );
        assert_error_contains(&region_errors, "code must be exactly 2 characters");
        assert_error_contains(&region_errors, "name must be 32 characters or less");
        assert_error_contains(&region_errors, "flag_code must be 16 characters or less");

        let language_errors = lookup_validation_errors(
            LookupTable::Languages,
            LookupPayload {
                rows: vec![lookup_payload_row(
                    "",
                    "zz",
                    "A very long language",
                    "us",
                    "1",
                )],
            },
        );
        assert_error_contains(&language_errors, "name must be 16 characters or less");
    }

    #[test]
    fn system_save_action_plans_unchanged_update_insert_and_rename() {
        let existing = test_system("SYS", "System");
        let existing_by_code = HashMap::from([(existing.code.clone(), existing.clone())]);
        let unchanged = validate_systems_payload(
            &SystemsPayload {
                rows: vec![payload_row("SYS", "SYS", "System")],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);
        let mut changed_payload = payload_row("SYS", "SYS", "System");
        changed_payload.media_types = "cd,dvd5".to_string();
        let update = validate_systems_payload(
            &SystemsPayload {
                rows: vec![changed_payload],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);
        let insert = validate_systems_payload(
            &SystemsPayload {
                rows: vec![payload_row("", "NEW", "New System")],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);
        let rename = validate_systems_payload(
            &SystemsPayload {
                rows: vec![payload_row("SYS", "NEW", "System")],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);

        assert_eq!(
            system_save_action(&unchanged, &existing_by_code),
            SystemSaveAction::Unchanged
        );
        assert_eq!(
            system_save_action(&update, &existing_by_code),
            SystemSaveAction::Update
        );
        assert_eq!(
            system_save_action(&insert, &existing_by_code),
            SystemSaveAction::Insert
        );
        assert_eq!(
            system_save_action(&rename, &existing_by_code),
            SystemSaveAction::Rename
        );
    }

    #[test]
    fn system_save_sql_marks_changed_rows_archives_dirty() {
        assert!(INSERT_SYSTEM_SQL.contains("archives_dirty"));
        assert!(INSERT_SYSTEM_SQL.contains("TRUE"));
        assert!(UPDATE_SYSTEM_SQL.contains("archives_dirty = TRUE"));
        assert!(RENAME_DISCS_SYSTEM_SQL.contains("UPDATE discs SET system_code"));
    }

    #[test]
    fn lookup_save_action_plans_unchanged_update_insert_and_rename() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let existing = test_lookup_record("aa", "Alpha", "us", 1);
            let existing_by_code = HashMap::from([(existing.code.clone(), existing.clone())]);
            let unchanged = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("aa", "aa", "Alpha", "us", "1")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);
            let update = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("aa", "aa", "Alpha", "jp", "2")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);
            let insert = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("", "bb", "Beta", "gb", "2")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);
            let rename = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("aa", "cc", "Alpha", "us", "1")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);

            assert_eq!(
                lookup_save_action(&unchanged, &existing_by_code),
                LookupSaveAction::Unchanged
            );
            assert_eq!(
                lookup_save_action(&update, &existing_by_code),
                LookupSaveAction::Update
            );
            assert_eq!(
                lookup_save_action(&insert, &existing_by_code),
                LookupSaveAction::Insert
            );
            assert_eq!(
                lookup_save_action(&rename, &existing_by_code),
                LookupSaveAction::Rename
            );
        }
    }

    #[test]
    fn lookup_save_sql_renames_update_disc_junction_tables() {
        assert!(RENAME_DISC_REGIONS_SQL.contains("UPDATE disc_regions SET region_code"));
        assert!(RENAME_DISC_LANGUAGES_SQL.contains("UPDATE disc_languages SET language_code"));
    }

    #[tokio::test]
    async fn maintenance_page_rejects_guests() {
        let response = routes()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/maintenance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn backup_download_rejects_guests() {
        let response = routes()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/maintenance/backups/vgindex-backup-20260703T060000Z.tar.gz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn maintenance_post_routes_reject_guests() {
        for uri in [
            "/maintenance/systems",
            "/maintenance/regions",
            "/maintenance/languages",
            "/maintenance/rebuild-cue",
            "/maintenance/trigger-archive-generation",
        ] {
            let response = routes()
                .with_state(test_state())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}
