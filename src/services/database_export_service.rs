use chrono::{DateTime, Timelike, Utc};
use futures_util::TryStreamExt;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, PgPool, Postgres, Sqlite, SqliteConnection, Transaction};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

const EXPORT_CACHE_VERSION: &str = "database-v1";
const EXPORT_PREFIX: &str = "redump-discs-";
const EXPORT_SUFFIX: &str = ".sqlite.zst";
const FORCE_MARKER: &str = ".force";
const EXPORT_HOUR_UTC: u32 = 7;
const ZSTD_LEVEL: i32 = 19;

const SQLITE_SCHEMA: &str = r#"
PRAGMA user_version = 1;

CREATE TABLE export_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE media_types (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    layer_count INTEGER NOT NULL,
    pic INTEGER NOT NULL,
    rom_extension TEXT NOT NULL
);

CREATE TABLE categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE regions (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    flag_code TEXT NOT NULL,
    sort_order INTEGER NOT NULL
);

CREATE TABLE languages (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    flag_code TEXT NOT NULL,
    sort_order INTEGER NOT NULL
);

CREATE TABLE systems (
    code TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    manufacturer TEXT NOT NULL,
    name TEXT NOT NULL,
    short_name TEXT NOT NULL,
    media_types TEXT NOT NULL CHECK (json_valid(media_types)),
    has_title_foreign INTEGER NOT NULL,
    has_disc_number INTEGER NOT NULL,
    has_disc_title INTEGER NOT NULL,
    has_serial INTEGER NOT NULL,
    has_edition INTEGER NOT NULL,
    has_barcode INTEGER NOT NULL,
    has_version INTEGER NOT NULL,
    has_exe_date INTEGER NOT NULL,
    has_edc INTEGER NOT NULL,
    has_disc_id INTEGER NOT NULL,
    has_key INTEGER NOT NULL,
    has_universal_hash INTEGER NOT NULL,
    has_protection INTEGER NOT NULL,
    has_sector_ranges INTEGER NOT NULL,
    has_sbi INTEGER NOT NULL,
    has_pvd INTEGER NOT NULL,
    has_header INTEGER NOT NULL,
    has_bca INTEGER NOT NULL,
    has_sample_start INTEGER NOT NULL,
    has_offset_extra INTEGER NOT NULL
);

CREATE TABLE discs (
    id INTEGER PRIMARY KEY,
    system_code TEXT NOT NULL REFERENCES systems(code),
    media_type_code TEXT NOT NULL REFERENCES media_types(code),
    category_id INTEGER NOT NULL REFERENCES categories(id),
    title TEXT NOT NULL,
    title_foreign TEXT,
    disc_number TEXT,
    disc_title TEXT,
    filename_suffix TEXT,
    serial TEXT NOT NULL CHECK (json_valid(serial)),
    edition TEXT NOT NULL CHECK (json_valid(edition)),
    barcode TEXT NOT NULL CHECK (json_valid(barcode)),
    version TEXT,
    error_count INTEGER,
    exe_date TEXT,
    edc INTEGER NOT NULL,
    layerbreaks TEXT CHECK (layerbreaks IS NULL OR json_valid(layerbreaks)),
    disc_id TEXT,
    disc_key BLOB,
    universal_hash BLOB,
    comments TEXT,
    contents TEXT,
    protection TEXT,
    sector_ranges TEXT CHECK (sector_ranges IS NULL OR json_valid(sector_ranges)),
    sbi TEXT,
    pvd BLOB,
    header BLOB,
    bca BLOB,
    pic BLOB,
    cue TEXT,
    status TEXT NOT NULL CHECK (status IN ('Questionable', 'Unverified', 'Verified'))
);

CREATE TABLE disc_regions (
    disc_id INTEGER NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    region_code TEXT NOT NULL REFERENCES regions(code),
    PRIMARY KEY (disc_id, region_code)
);

CREATE TABLE disc_languages (
    disc_id INTEGER NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    language_code TEXT NOT NULL REFERENCES languages(code),
    PRIMARY KEY (disc_id, language_code)
);

CREATE TABLE disc_ring_code_entries (
    id INTEGER PRIMARY KEY,
    disc_id INTEGER NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    offset_value INTEGER,
    offset_extra_value INTEGER,
    sample_data_start INTEGER,
    comment TEXT
);

CREATE TABLE disc_ring_code_layers (
    id INTEGER PRIMARY KEY,
    entry_id INTEGER NOT NULL REFERENCES disc_ring_code_entries(id) ON DELETE CASCADE,
    layer INTEGER NOT NULL,
    mastering_code TEXT,
    mastering_sid TEXT,
    toolstamps TEXT NOT NULL,
    mould_sids TEXT NOT NULL,
    additional_moulds TEXT NOT NULL,
    UNIQUE (entry_id, layer)
);

CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    disc_id INTEGER NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    track_number TEXT,
    size INTEGER NOT NULL,
    crc32 TEXT NOT NULL,
    md5 TEXT NOT NULL,
    sha1 TEXT NOT NULL,
    UNIQUE (disc_id, track_number)
);

CREATE UNIQUE INDEX files_disc_cue_unique ON files (disc_id) WHERE track_number IS NULL;
"#;

#[derive(Clone, Debug)]
pub struct DatabaseExportInfo {
    pub path: PathBuf,
    pub filename: String,
    pub created_at: DateTime<Utc>,
    pub size: u64,
}

#[derive(sqlx::FromRow)]
struct MediaTypeRow {
    code: String,
    name: String,
    layer_count: i32,
    pic: bool,
    rom_extension: String,
}

#[derive(sqlx::FromRow)]
struct CategoryRow {
    id: i32,
    name: String,
}

#[derive(sqlx::FromRow)]
struct LookupRow {
    code: String,
    name: String,
    flag_code: String,
    sort_order: i32,
}

#[derive(sqlx::FromRow)]
struct SystemRow {
    code: String,
    system_type: String,
    manufacturer: String,
    name: String,
    short_name: String,
    media_types: String,
    has_title_foreign: bool,
    has_disc_number: bool,
    has_disc_title: bool,
    has_serial: bool,
    has_edition: bool,
    has_barcode: bool,
    has_version: bool,
    has_exe_date: bool,
    has_edc: bool,
    has_disc_id: bool,
    has_key: bool,
    has_universal_hash: bool,
    has_protection: bool,
    has_sector_ranges: bool,
    has_sbi: bool,
    has_pvd: bool,
    has_header: bool,
    has_bca: bool,
    has_sample_start: bool,
    has_offset_extra: bool,
}

#[derive(sqlx::FromRow)]
struct DiscRow {
    id: i32,
    system_code: String,
    media_type_code: String,
    category_id: i32,
    title: String,
    title_foreign: Option<String>,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    serial: String,
    edition: String,
    barcode: String,
    version: Option<String>,
    error_count: Option<i32>,
    exe_date: Option<String>,
    edc: bool,
    layerbreaks: Option<String>,
    disc_id: Option<String>,
    disc_key: Option<Vec<u8>>,
    universal_hash: Option<Vec<u8>>,
    comments: Option<String>,
    contents: Option<String>,
    protection: Option<String>,
    sector_ranges: Option<String>,
    sbi: Option<String>,
    pvd: Option<Vec<u8>>,
    header: Option<Vec<u8>>,
    bca: Option<Vec<u8>>,
    pic: Option<Vec<u8>>,
    cue: Option<String>,
    status: String,
}

#[derive(sqlx::FromRow)]
struct DiscRegionRow {
    disc_id: i32,
    region_code: String,
}

#[derive(sqlx::FromRow)]
struct DiscLanguageRow {
    disc_id: i32,
    language_code: String,
}

#[derive(sqlx::FromRow)]
struct RingEntryRow {
    id: i32,
    disc_id: i32,
    offset_value: Option<i32>,
    offset_extra_value: Option<i32>,
    sample_data_start: Option<i32>,
    comment: Option<String>,
}

#[derive(sqlx::FromRow)]
struct RingLayerRow {
    id: i32,
    entry_id: i32,
    layer: i32,
    mastering_code: Option<String>,
    mastering_sid: Option<String>,
    toolstamps: String,
    mould_sids: String,
    additional_moulds: String,
}

#[derive(sqlx::FromRow)]
struct FileRow {
    id: i32,
    disc_id: i32,
    track_number: Option<String>,
    size: i64,
    crc32: String,
    md5: String,
    sha1: String,
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::Internal(error.to_string())
}

fn export_directory() -> PathBuf {
    Path::new(crate::config::DATA_DIR)
        .join("archives")
        .join(EXPORT_CACHE_VERSION)
}

fn force_marker_path() -> PathBuf {
    export_directory().join(FORCE_MARKER)
}

pub fn mark_export_dirty() -> io::Result<()> {
    let directory = export_directory();
    fs::create_dir_all(&directory)?;
    fs::write(directory.join(FORCE_MARKER), b"")
}

pub fn latest_export() -> Option<DatabaseExportInfo> {
    latest_export_in_dir(&export_directory())
}

fn latest_export_in_dir(directory: &Path) -> Option<DatabaseExportInfo> {
    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let filename = entry.file_name().into_string().ok()?;
            let created_at = parse_export_filename(&filename)?;
            let metadata = entry.metadata().ok()?;
            Some(DatabaseExportInfo {
                path: entry.path(),
                filename,
                created_at,
                size: metadata.len(),
            })
        })
        .max_by_key(|export| export.created_at)
}

fn parse_export_filename(filename: &str) -> Option<DateTime<Utc>> {
    let timestamp = filename
        .strip_prefix(EXPORT_PREFIX)?
        .strip_suffix(EXPORT_SUFFIX)?;
    chrono::NaiveDateTime::parse_from_str(timestamp, "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|value| value.and_utc())
}

fn export_is_due(now: DateTime<Utc>, latest: Option<&DatabaseExportInfo>, forced: bool) -> bool {
    if forced || latest.is_none() {
        return true;
    }
    now.hour() >= EXPORT_HOUR_UTC
        && latest.is_some_and(|export| export.created_at.date_naive() < now.date_naive())
}

pub async fn regenerate_if_due(
    pool: &PgPool,
    source_url: &str,
    now: DateTime<Utc>,
) -> AppResult<Option<DatabaseExportInfo>> {
    let pool = pool.clone();
    let source_url = source_url.to_string();
    tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(internal)?;
        runtime.block_on(regenerate_if_due_inner(&pool, &source_url, now))
    })
    .await
    .map_err(internal)?
}

async fn regenerate_if_due_inner(
    pool: &PgPool,
    source_url: &str,
    now: DateTime<Utc>,
) -> AppResult<Option<DatabaseExportInfo>> {
    let forced = force_marker_path().is_file();
    let latest = latest_export();
    if !export_is_due(now, latest.as_ref(), forced) {
        return Ok(None);
    }

    tracing::info!(forced, "Generating SQLite disc database export");
    generate_export(pool, source_url, now).await.map(Some)
}

async fn generate_export(
    pool: &PgPool,
    source_url: &str,
    generated_at: DateTime<Utc>,
) -> AppResult<DatabaseExportInfo> {
    let directory = export_directory();
    fs::create_dir_all(&directory).map_err(internal)?;
    remove_stale_partials(&directory);

    let timestamp = generated_at.format("%Y%m%dT%H%M%SZ").to_string();
    let filename = format!("{EXPORT_PREFIX}{timestamp}{EXPORT_SUFFIX}");
    let sqlite_partial = directory.join(format!(".{EXPORT_PREFIX}{timestamp}.sqlite.partial"));
    let compressed_partial = directory.join(format!(".{filename}.partial"));
    let final_path = directory.join(&filename);

    let result = generate_export_inner(
        pool,
        source_url,
        generated_at,
        &sqlite_partial,
        &compressed_partial,
    )
    .await;

    if let Err(error) = result {
        let _ = fs::remove_file(&sqlite_partial);
        let _ = fs::remove_file(&compressed_partial);
        return Err(error);
    }

    fs::rename(&compressed_partial, &final_path).map_err(internal)?;
    let _ = fs::remove_file(&sqlite_partial);
    remove_old_exports(&directory, &final_path);
    let _ = fs::remove_file(force_marker_path());

    let size = fs::metadata(&final_path).map_err(internal)?.len();
    tracing::info!(filename, size, "SQLite disc database export complete");
    Ok(DatabaseExportInfo {
        path: final_path,
        filename,
        created_at: generated_at,
        size,
    })
}

async fn generate_export_inner(
    pool: &PgPool,
    source_url: &str,
    generated_at: DateTime<Utc>,
    sqlite_path: &Path,
    compressed_path: &Path,
) -> AppResult<()> {
    let options = SqliteConnectOptions::new()
        .filename(sqlite_path)
        .create_if_missing(true)
        .foreign_keys(false)
        .journal_mode(SqliteJournalMode::Off)
        .synchronous(SqliteSynchronous::Off);
    let mut sqlite = SqliteConnection::connect_with(&options).await?;
    let mut postgres = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *postgres)
        .await?;
    let mut output = sqlite.begin().await?;
    sqlx::raw_sql(SQLITE_SCHEMA).execute(&mut *output).await?;

    insert_metadata(&mut output, generated_at, source_url).await?;
    tracing::info!(
        rows = export_media_types(&mut postgres, &mut output).await?,
        table = "media_types"
    );
    tracing::info!(
        rows = export_categories(&mut postgres, &mut output).await?,
        table = "categories"
    );
    tracing::info!(
        rows = export_regions(&mut postgres, &mut output).await?,
        table = "regions"
    );
    tracing::info!(
        rows = export_languages(&mut postgres, &mut output).await?,
        table = "languages"
    );
    tracing::info!(
        rows = export_systems(&mut postgres, &mut output).await?,
        table = "systems"
    );
    tracing::info!(
        rows = export_discs(&mut postgres, &mut output).await?,
        table = "discs"
    );
    tracing::info!(
        rows = export_disc_regions(&mut postgres, &mut output).await?,
        table = "disc_regions"
    );
    tracing::info!(
        rows = export_disc_languages(&mut postgres, &mut output).await?,
        table = "disc_languages"
    );
    tracing::info!(
        rows = export_ring_entries(&mut postgres, &mut output).await?,
        table = "disc_ring_code_entries"
    );
    tracing::info!(
        rows = export_ring_layers(&mut postgres, &mut output).await?,
        table = "disc_ring_code_layers"
    );
    tracing::info!(
        rows = export_files(&mut postgres, &mut output).await?,
        table = "files"
    );

    output.commit().await?;
    postgres.commit().await?;

    sqlx::query("VACUUM").execute(&mut sqlite).await?;
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut sqlite)
        .await?;
    if integrity != "ok" {
        return Err(internal(format!(
            "SQLite integrity check failed: {integrity}"
        )));
    }
    if sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut sqlite)
        .await?
        .is_some()
    {
        return Err(internal("SQLite foreign key check failed"));
    }
    let _: String = sqlx::query_scalar("PRAGMA journal_mode=DELETE")
        .fetch_one(&mut sqlite)
        .await?;
    sqlite.close().await?;

    compress_zstd(sqlite_path, compressed_path)
}

async fn insert_metadata(
    sqlite: &mut Transaction<'_, Sqlite>,
    generated_at: DateTime<Utc>,
    source_url: &str,
) -> AppResult<()> {
    for (key, value) in [
        ("schema_version", "1".to_string()),
        ("generated_at_utc", generated_at.to_rfc3339()),
        ("source_url", source_url.to_string()),
        (
            "included_statuses",
            "Questionable,Unverified,Verified".to_string(),
        ),
        ("array_encoding", "JSON text".to_string()),
        ("binary_encoding", "SQLite BLOB".to_string()),
        ("compression", "Zstandard level 19".to_string()),
    ] {
        sqlx::query("INSERT INTO export_metadata (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(&mut **sqlite)
            .await?;
    }
    Ok(())
}

async fn export_media_types(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, MediaTypeRow>(
        "SELECT code, name, layer_count, pic, rom_extension FROM media_types ORDER BY code",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("INSERT INTO media_types VALUES (?, ?, ?, ?, ?)")
            .bind(row.code)
            .bind(row.name)
            .bind(row.layer_count)
            .bind(row.pic)
            .bind(row.rom_extension)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_categories(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, CategoryRow>("SELECT id, name FROM categories ORDER BY id")
        .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("INSERT INTO categories VALUES (?, ?)")
            .bind(row.id)
            .bind(row.name)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_regions(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    export_lookup_table(postgres, sqlite, "regions").await
}

async fn export_languages(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    export_lookup_table(postgres, sqlite, "languages").await
}

async fn export_lookup_table(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
    table: &str,
) -> AppResult<u64> {
    let select = format!(
        "SELECT TRIM(code) AS code, name, TRIM(flag_code) AS flag_code, sort_order FROM {table} ORDER BY sort_order, code"
    );
    let insert = format!("INSERT INTO {table} VALUES (?, ?, ?, ?)");
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, LookupRow>(&select).fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query(&insert)
            .bind(row.code)
            .bind(row.name)
            .bind(row.flag_code)
            .bind(row.sort_order)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_systems(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, SystemRow>(
        "SELECT code, type AS system_type, manufacturer, name, short_name,
                to_json(media_types)::text AS media_types,
                has_title_foreign, has_disc_number, has_disc_title, has_serial,
                has_edition, has_barcode, has_version, has_exe_date, has_edc,
                has_disc_id, has_key, has_universal_hash, has_protection,
                has_sector_ranges, has_sbi, has_pvd, has_header, has_bca,
                has_sample_start, has_offset_extra
         FROM systems ORDER BY code",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query(
            "INSERT INTO systems VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.code)
        .bind(row.system_type)
        .bind(row.manufacturer)
        .bind(row.name)
        .bind(row.short_name)
        .bind(row.media_types)
        .bind(row.has_title_foreign)
        .bind(row.has_disc_number)
        .bind(row.has_disc_title)
        .bind(row.has_serial)
        .bind(row.has_edition)
        .bind(row.has_barcode)
        .bind(row.has_version)
        .bind(row.has_exe_date)
        .bind(row.has_edc)
        .bind(row.has_disc_id)
        .bind(row.has_key)
        .bind(row.has_universal_hash)
        .bind(row.has_protection)
        .bind(row.has_sector_ranges)
        .bind(row.has_sbi)
        .bind(row.has_pvd)
        .bind(row.has_header)
        .bind(row.has_bca)
        .bind(row.has_sample_start)
        .bind(row.has_offset_extra)
        .execute(&mut **sqlite)
        .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_discs(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, DiscRow>(
        "SELECT id, system_code, media_type_code, category_id, title, title_foreign,
                disc_number, disc_title, filename_suffix,
                to_json(serial)::text AS serial,
                to_json(edition)::text AS edition,
                to_json(barcode)::text AS barcode,
                version, error_count, exe_date, edc,
                to_json(layerbreaks)::text AS layerbreaks,
                disc_id, disc_key, universal_hash, comments, contents, protection,
                to_json(sector_ranges)::text AS sector_ranges,
                sbi, pvd, header, bca, pic, cue, status::text AS status
         FROM discs
         WHERE status <> 'Disabled'
         ORDER BY id",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query(
            "INSERT INTO discs VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.id)
        .bind(row.system_code)
        .bind(row.media_type_code)
        .bind(row.category_id)
        .bind(row.title)
        .bind(row.title_foreign)
        .bind(row.disc_number)
        .bind(row.disc_title)
        .bind(row.filename_suffix)
        .bind(row.serial)
        .bind(row.edition)
        .bind(row.barcode)
        .bind(row.version)
        .bind(row.error_count)
        .bind(row.exe_date)
        .bind(row.edc)
        .bind(row.layerbreaks)
        .bind(row.disc_id)
        .bind(row.disc_key)
        .bind(row.universal_hash)
        .bind(row.comments)
        .bind(row.contents)
        .bind(row.protection)
        .bind(row.sector_ranges)
        .bind(row.sbi)
        .bind(row.pvd)
        .bind(row.header)
        .bind(row.bca)
        .bind(row.pic)
        .bind(row.cue)
        .bind(row.status)
        .execute(&mut **sqlite)
        .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_disc_regions(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, DiscRegionRow>(
        "SELECT dr.disc_id, TRIM(dr.region_code) AS region_code
         FROM disc_regions dr JOIN discs d ON d.id = dr.disc_id
         WHERE d.status <> 'Disabled' ORDER BY dr.disc_id, dr.region_code",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("INSERT INTO disc_regions VALUES (?, ?)")
            .bind(row.disc_id)
            .bind(row.region_code)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_disc_languages(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, DiscLanguageRow>(
        "SELECT dl.disc_id, TRIM(dl.language_code) AS language_code
         FROM disc_languages dl JOIN discs d ON d.id = dl.disc_id
         WHERE d.status <> 'Disabled' ORDER BY dl.disc_id, dl.language_code",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("INSERT INTO disc_languages VALUES (?, ?)")
            .bind(row.disc_id)
            .bind(row.language_code)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_ring_entries(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, RingEntryRow>(
        "SELECT e.id, e.disc_id, e.offset_value, e.offset_extra_value,
                e.sample_data_start, e.comment
         FROM disc_ring_code_entries e JOIN discs d ON d.id = e.disc_id
         WHERE d.status <> 'Disabled' ORDER BY e.id",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("INSERT INTO disc_ring_code_entries VALUES (?, ?, ?, ?, ?, ?)")
            .bind(row.id)
            .bind(row.disc_id)
            .bind(row.offset_value)
            .bind(row.offset_extra_value)
            .bind(row.sample_data_start)
            .bind(row.comment)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_ring_layers(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, RingLayerRow>(
        "SELECT l.id, l.entry_id, l.layer, l.mastering_code, l.mastering_sid,
                l.toolstamps, l.mould_sids, l.additional_moulds
         FROM disc_ring_code_layers l
         JOIN disc_ring_code_entries e ON e.id = l.entry_id
         JOIN discs d ON d.id = e.disc_id
         WHERE d.status <> 'Disabled' ORDER BY l.id",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("INSERT INTO disc_ring_code_layers VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(row.id)
            .bind(row.entry_id)
            .bind(row.layer)
            .bind(row.mastering_code)
            .bind(row.mastering_sid)
            .bind(row.toolstamps)
            .bind(row.mould_sids)
            .bind(row.additional_moulds)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

async fn export_files(
    postgres: &mut Transaction<'_, Postgres>,
    sqlite: &mut Transaction<'_, Sqlite>,
) -> AppResult<u64> {
    let mut count = 0;
    let mut rows = sqlx::query_as::<_, FileRow>(
        "SELECT f.id, f.disc_id, f.track_number, f.size, f.crc32, f.md5, f.sha1
         FROM files f JOIN discs d ON d.id = f.disc_id
         WHERE d.status <> 'Disabled' ORDER BY f.id",
    )
    .fetch(&mut **postgres);
    while let Some(row) = rows.try_next().await? {
        sqlx::query("INSERT INTO files VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(row.id)
            .bind(row.disc_id)
            .bind(row.track_number)
            .bind(row.size)
            .bind(row.crc32)
            .bind(row.md5)
            .bind(row.sha1)
            .execute(&mut **sqlite)
            .await?;
        count += 1;
    }
    Ok(count)
}

fn compress_zstd(source: &Path, destination: &Path) -> AppResult<()> {
    let mut input = fs::File::open(source).map_err(internal)?;
    let output = fs::File::create(destination).map_err(internal)?;
    let mut encoder = zstd::stream::write::Encoder::new(output, ZSTD_LEVEL).map_err(internal)?;
    encoder.include_checksum(true).map_err(internal)?;
    io::copy(&mut input, &mut encoder).map_err(internal)?;
    encoder
        .finish()
        .map_err(internal)?
        .sync_all()
        .map_err(internal)
}

fn remove_stale_partials(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_partial = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(&format!(".{EXPORT_PREFIX}")) && name.ends_with(".partial")
            });
        if is_partial {
            let _ = fs::remove_file(path);
        }
    }
}

fn remove_old_exports(directory: &Path, current: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path != current
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(parse_export_filename)
                .is_some()
        {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export_info(timestamp: &str) -> DatabaseExportInfo {
        let filename = format!("{EXPORT_PREFIX}{timestamp}{EXPORT_SUFFIX}");
        DatabaseExportInfo {
            path: PathBuf::from(&filename),
            created_at: parse_export_filename(&filename).unwrap(),
            filename,
            size: 1,
        }
    }

    #[test]
    fn completed_export_filename_is_strict() {
        assert!(parse_export_filename("redump-discs-20260703T070000Z.sqlite.zst").is_some());
        assert!(
            parse_export_filename(".redump-discs-20260703T070000Z.sqlite.zst.partial").is_none()
        );
        assert!(parse_export_filename("vgindex-discs-20260703T070000Z.sqlite.zst").is_none());
        assert!(parse_export_filename("redump-discs-20260703.sqlite.zst").is_none());
    }

    #[test]
    fn export_is_immediate_when_missing_or_forced() {
        let now = "2026-07-03T01:00:00Z".parse().unwrap();
        let current = export_info("20260703T000000Z");
        assert!(export_is_due(now, None, false));
        assert!(export_is_due(now, Some(&current), true));
        assert!(!export_is_due(now, Some(&current), false));
    }

    #[test]
    fn daily_export_waits_until_seven_utc() {
        let previous = export_info("20260702T070000Z");
        let current = export_info("20260703T070000Z");
        let before = "2026-07-03T06:59:59Z".parse().unwrap();
        let after = "2026-07-03T07:00:00Z".parse().unwrap();
        assert!(!export_is_due(before, Some(&previous), false));
        assert!(export_is_due(after, Some(&previous), false));
        assert!(!export_is_due(after, Some(&current), false));
    }

    #[tokio::test]
    async fn sqlite_schema_has_only_required_explicit_index() {
        let mut database = SqliteConnection::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(SQLITE_SCHEMA)
            .execute(&mut database)
            .await
            .unwrap();

        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master
             WHERE type = 'index' AND name NOT LIKE 'sqlite_autoindex_%'
             ORDER BY name",
        )
        .fetch_all(&mut database)
        .await
        .unwrap();
        assert_eq!(indexes, ["files_disc_cue_unique"]);

        let disc_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('discs')")
                .fetch_all(&mut database)
                .await
                .unwrap();
        let system_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('systems')")
                .fetch_all(&mut database)
                .await
                .unwrap();
        assert!(!disc_columns.iter().any(|name| name == "search_vector"));
        assert!(!system_columns.iter().any(|name| name == "archives_dirty"));
    }

    #[test]
    fn latest_export_ignores_partial_and_old_files() {
        let directory =
            std::env::temp_dir().join(format!("database-export-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("redump-discs-20260702T070000Z.sqlite.zst"),
            b"old",
        )
        .unwrap();
        fs::write(
            directory.join("redump-discs-20260703T070000Z.sqlite.zst"),
            b"new",
        )
        .unwrap();
        fs::write(
            directory.join(".redump-discs-20260704T070000Z.sqlite.zst.partial"),
            b"partial",
        )
        .unwrap();

        let latest = latest_export_in_dir(&directory).unwrap();
        assert_eq!(latest.filename, "redump-discs-20260703T070000Z.sqlite.zst");
        assert_eq!(latest.size, 3);
        fs::remove_dir_all(directory).unwrap();
    }
}
