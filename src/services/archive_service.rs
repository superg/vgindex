use std::io::Write;
use sqlx::PgPool;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::db::models::*;
use crate::error::{AppError, AppResult};

pub struct ArchiveResult {
    pub data: Vec<u8>,
    pub filename: String,
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn archive_subdir(system: &str, archive_type: &str) -> String {
    format!("{}/archives/{}-{}", crate::config::DATA_DIR, system, archive_type)
}

fn find_cached_archive(system: &str, archive_type: &str) -> Option<ArchiveResult> {
    let dir = archive_subdir(system, archive_type);
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("zip") {
            let filename = path.file_name()?.to_str()?.to_string();
            let data = std::fs::read(&path).ok()?;
            return Some(ArchiveResult { data, filename });
        }
    }
    None
}

fn store_archive(system: &str, archive_type: &str, result: &ArchiveResult) {
    let dir = archive_subdir(system, archive_type);
    std::fs::create_dir_all(&dir).ok();
    let new_path = format!("{}/{}", dir, result.filename);
    std::fs::write(&new_path, &result.data).ok();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("zip")
                && path.file_name().and_then(|n| n.to_str()) != Some(&result.filename)
            {
                std::fs::remove_file(&path).ok();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub async fn get_or_generate_archive(
    pool: &PgPool,
    system: &str,
    archive_type: &str,
) -> AppResult<ArchiveResult> {
    if let Some(result) = find_cached_archive(system, archive_type) {
        return Ok(result);
    }

    let result = match archive_type {
        "dat" => generate_datfile_archive(pool, system).await?,
        "cue" => generate_cuesheet_archive(pool, system).await?,
        "sbi" => generate_sbi_archive(pool, system).await?,
        _ => return Err(AppError::NotFound),
    };

    store_archive(system, archive_type, &result);
    Ok(result)
}

pub async fn regenerate_system_archives(pool: &PgPool, system: &str) {
    let sys: Option<System> = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(sys) = sys else { return };

    if let Ok(result) = generate_datfile_archive(pool, &sys.code).await {
        store_archive(&sys.code, "dat", &result);
    }

    if system_has_cd_media(pool, &sys.media_types).await {
        if let Ok(result) = generate_cuesheet_archive(pool, &sys.code).await {
            store_archive(&sys.code, "cue", &result);
        }
    }

    if sys.has_sbi {
        if let Ok(result) = generate_sbi_archive(pool, &sys.code).await {
            store_archive(&sys.code, "sbi", &result);
        }
    }
}

pub async fn run_archive_worker(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    pool: PgPool,
) {
    loop {
        let Some(first) = rx.recv().await else { break };

        let mut dirty = std::collections::HashSet::new();
        dirty.insert(first);
        while let Ok(code) = rx.try_recv() {
            dirty.insert(code);
        }

        for code in dirty {
            tracing::info!("Regenerating archives for system {code}");
            regenerate_system_archives(&pool, &code).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

async fn system_has_cd_media(pool: &PgPool, media_type_codes: &[String]) -> bool {
    if media_type_codes.is_empty() {
        return false;
    }
    let result: Option<bool> = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM media_types WHERE code = ANY($1) AND rom_extension = 'bin')",
    )
    .bind(media_type_codes)
    .fetch_one(pool)
    .await
    .unwrap_or(Some(false));
    result.unwrap_or(false)
}

async fn get_disc_region_names(pool: &PgPool, disc_id: i32) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT r.name FROM disc_regions dr
         JOIN regions r ON r.code = dr.region_code
         WHERE dr.disc_id = $1 ORDER BY r.sort_order",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

async fn get_disc_language_codes(pool: &PgPool, disc_id: i32) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT l.code FROM disc_languages dl
         JOIN languages l ON l.code = dl.language_code
         WHERE dl.disc_id = $1 ORDER BY l.sort_order",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

fn timestamp_now() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H-%M-%S").to_string()
}

fn build_datfile_name(
    title: &str,
    regions: &[String],
    version: Option<&str>,
    edition: Option<&str>,
    disc_number: Option<&str>,
    disc_title: Option<&str>,
    suffix: Option<&str>,
) -> String {
    let mut name = title.to_string();
    if !regions.is_empty() {
        name.push_str(&format!(" ({})", regions.join(", ")));
    }
    if let Some(v) = version {
        if !v.is_empty() {
            name.push_str(&format!(" ({})", v));
        }
    }
    if let Some(e) = edition {
        if !e.is_empty() {
            name.push_str(&format!(" ({})", e));
        }
    }
    if let Some(n) = disc_number {
        if !n.is_empty() {
            name.push_str(&format!(" (Disc {})", n));
        }
    }
    if let Some(d) = disc_title {
        if !d.is_empty() {
            name.push_str(&format!(" ({})", d));
        }
    }
    if let Some(s) = suffix {
        if !s.is_empty() {
            name.push_str(&format!(" ({})", s));
        }
    }
    name
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---------------------------------------------------------------------------
// DAT generation
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct DatfileDisc {
    id: i32,
    title: String,
    version: Option<String>,
    edition: Option<String>,
    filename_suffix: Option<String>,
    disc_number: Option<String>,
    disc_title: Option<String>,
    category_name: String,
    rom_extension: String,
}

async fn generate_datfile_archive(pool: &PgPool, system: &str) -> AppResult<ArchiveResult> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let discs: Vec<DatfileDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.version,
                NULLIF(array_to_string(d.edition, ', '), '') AS edition,
                d.filename_suffix, d.disc_number, d.disc_title,
                c.name AS category_name,
                mt.rom_extension
         FROM discs d
         JOIN categories c ON c.id = d.category_id
         JOIN media_types mt ON mt.code = d.media_type_code
         WHERE d.system_code = $1 AND d.enabled AND NOT d.questionable
         ORDER BY d.title",
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let ts = timestamp_now();
    let disc_count = discs.len();
    let description = format!("{} - Discs ({}) ({})", sys.name, disc_count, ts);

    let mut xml = format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE datafile PUBLIC "-//Logiqx//DTD ROM Management Datafile//EN" "http://www.logiqx.com/Dats/datafile.dtd">
<datafile>
	<header>
		<name>{name}</name>
		<description>{desc}</description>
		<version>{ts}</version>
		<date>{ts}</date>
		<author>vgindex.org</author>
		<homepage>vgindex.org</homepage>
		<url>https://vgindex.org/</url>
	</header>
"#,
        name = html_escape(&sys.name),
        desc = html_escape(&description),
        ts = html_escape(&ts),
    );

    for disc in &discs {
        let files: Vec<File> = sqlx::query_as(
            "SELECT * FROM files WHERE disc_id = $1 ORDER BY track_number",
        )
        .bind(disc.id)
        .fetch_all(pool)
        .await?;

        let regions = get_disc_region_names(pool, disc.id).await;
        let game_name = build_datfile_name(
            &disc.title,
            &regions,
            disc.version.as_deref(),
            disc.edition.as_deref(),
            disc.disc_number.as_deref(),
            disc.disc_title.as_deref(),
            disc.filename_suffix.as_deref(),
        );

        xml.push_str(&format!(
            "\t<game name=\"{name}\">\n\t\t<category>{cat}</category>\n\t\t<description>{name}</description>\n",
            name = html_escape(&game_name),
            cat = html_escape(&disc.category_name),
        ));

        let total_tracks = files.iter().filter(|f| f.track_number.is_some()).count();
        for file in &files {
            let ext = if file.track_number.is_some() {
                disc.rom_extension.as_str()
            } else {
                "cue"
            };
            let rom_name = build_rom_name(
                &game_name,
                file.track_number.as_deref(),
                total_tracks,
                ext,
            );

            xml.push_str(&format!(
                "\t\t<rom name=\"{name}\" size=\"{size}\" crc=\"{crc}\" md5=\"{md5}\" sha1=\"{sha1}\"/>\n",
                name = html_escape(&rom_name),
                size = file.size,
                crc = file.crc32,
                md5 = file.md5,
                sha1 = file.sha1,
            ));
        }

        xml.push_str("\t</game>\n");
    }

    xml.push_str("</datafile>\n");

    let inner_filename = format!("{}.dat", description);
    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(&inner_filename, options)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        zip.write_all(xml.as_bytes())
            .map_err(|e| AppError::Internal(e.to_string()))?;
        zip.finish()
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    let zip_filename = format!("{} - Datfile ({}) ({}).zip", sys.name, disc_count, ts);
    Ok(ArchiveResult {
        data: buf,
        filename: zip_filename,
    })
}

// ---------------------------------------------------------------------------
// Cuesheet generation
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct CueDisc {
    id: i32,
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    cue: String,
}

async fn generate_cuesheet_archive(pool: &PgPool, system: &str) -> AppResult<ArchiveResult> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    if !system_has_cd_media(pool, &sys.media_types).await {
        return Err(AppError::NotFound);
    }

    let discs: Vec<CueDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix, d.cue
         FROM discs d
         WHERE d.system_code = $1 AND d.enabled AND NOT d.questionable
               AND d.cue IS NOT NULL AND d.cue != ''
         ORDER BY d.title",
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let ts = timestamp_now();
    let cue_count = discs.len();

    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for disc in &discs {
            let region_names = get_disc_region_names(pool, disc.id).await;
            let language_codes = get_disc_language_codes(pool, disc.id).await;
            let base_name = build_rom_base_name(
                &disc.title,
                &region_names,
                &language_codes,
                disc.disc_number.as_deref(),
                disc.disc_title.as_deref(),
                disc.filename_suffix.as_deref(),
            );
            let filename = format!("{base_name}.cue");
            zip.start_file(&filename, options)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            zip.write_all(disc.cue.as_bytes())
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }

        zip.finish()
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    let zip_filename = format!("{} - Cuesheets ({}) ({}).zip", sys.name, cue_count, ts);
    Ok(ArchiveResult {
        data: buf,
        filename: zip_filename,
    })
}

// ---------------------------------------------------------------------------
// SBI generation
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct SbiArchiveDisc {
    id: i32,
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    sbi: String,
}

async fn generate_sbi_archive(pool: &PgPool, system: &str) -> AppResult<ArchiveResult> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    if !sys.has_sbi {
        return Err(AppError::NotFound);
    }

    let discs: Vec<SbiArchiveDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix, d.sbi
         FROM discs d
         WHERE d.system_code = $1 AND d.sbi IS NOT NULL AND d.sbi != ''
               AND d.enabled AND NOT d.questionable
         ORDER BY d.title",
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let ts = timestamp_now();
    let sbi_count = discs.len();

    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for disc in &discs {
            let region_names = get_disc_region_names(pool, disc.id).await;
            let language_codes = get_disc_language_codes(pool, disc.id).await;
            let base_name = build_rom_base_name(
                &disc.title,
                &region_names,
                &language_codes,
                disc.disc_number.as_deref(),
                disc.disc_title.as_deref(),
                disc.filename_suffix.as_deref(),
            );
            let filename = format!("{base_name}.sbi");
            let sbi_binary = build_sbi_binary(&disc.sbi);
            zip.start_file(&filename, options)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            zip.write_all(&sbi_binary)
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }

        zip.finish()
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    let zip_filename = format!(
        "{} - SBI Subchannels ({}) ({}).zip",
        sys.name, sbi_count, ts
    );
    Ok(ArchiveResult {
        data: buf,
        filename: zip_filename,
    })
}
