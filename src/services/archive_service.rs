use std::io::Write;
use sqlx::PgPool;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::config::Config;
use crate::db::models::*;
use crate::error::{AppError, AppResult};

pub async fn get_or_generate_archive(
    pool: &PgPool,
    config: &Config,
    system: &str,
    archive_type: &str,
) -> AppResult<Vec<u8>> {
    let cache_dir = format!("{}/archives", config.data_dir);
    std::fs::create_dir_all(&cache_dir).ok();

    let cache_path = format!("{cache_dir}/{system}-{archive_type}.zip");

    if let Ok(data) = std::fs::read(&cache_path) {
        return Ok(data);
    }

    let data = match archive_type {
        "dat" => generate_datfile_archive(pool, system).await?,
        "cue" => generate_cuesheet_archive(pool, system).await?,
        "sbi" => generate_sbi_archive(pool, system).await?,
        _ => return Err(AppError::NotFound),
    };

    std::fs::write(&cache_path, &data).ok();
    Ok(data)
}

pub async fn invalidate_cache(config: &Config, system: &str) {
    let cache_dir = format!("{}/archives", config.data_dir);
    for ext in &["dat", "cue", "sbi"] {
        let path = format!("{cache_dir}/{system}-{ext}.zip");
        std::fs::remove_file(&path).ok();
    }
}

async fn generate_datfile_archive(pool: &PgPool, system: &str) -> AppResult<Vec<u8>> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let discs: Vec<DatfileDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.version, d.edition, d.filename_suffix
         FROM discs d
         WHERE d.system_code = $1 AND d.enabled AND NOT d.questionable
         ORDER BY d.title"
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let mut xml = format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE datafile PUBLIC "-//Logiqx//DTD ROM Management Datafile//EN" "http://www.logiqx.com/Dats/datafile.dtd">
<datafile>
    <header>
        <name>{}</name>
        <description>{}</description>
        <version>{}</version>
        <homepage>https://vgindex.org</homepage>
    </header>
"#,
        html_escape(&sys.name),
        html_escape(&sys.name),
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
    );

    for disc in &discs {
        let files: Vec<File> = sqlx::query_as(
            "SELECT * FROM files WHERE disc_id = $1 ORDER BY track_number"
        )
        .bind(disc.id)
        .fetch_all(pool)
        .await?;

        let regions = get_disc_region_names(pool, disc.id).await;
        let game_name = build_datfile_name(&disc.title, &regions, disc.version.as_deref(), disc.edition.as_deref(), disc.filename_suffix.as_deref());

        xml.push_str(&format!(
            "\t<game name=\"{}\">\n\t\t<description>{}</description>\n",
            html_escape(&game_name),
            html_escape(&game_name),
        ));

        for file in &files {
            let ext = if file.track_number.is_some() {
                if sys.media_types.iter().any(|s| MediaType::from_code(s).map_or(false, |m| m.is_cd())) {
                    "bin"
                } else {
                    "iso"
                }
            } else {
                "cue"
            };
            let track_suffix = file
                .track_number
                .as_ref()
                .map(|t| format!(" (Track {t})"))
                .unwrap_or_default();
            let rom_name = format!("{game_name}{track_suffix}.{ext}");

            xml.push_str(&format!(
                "\t\t<rom name=\"{}\" size=\"{}\" crc=\"{}\" md5=\"{}\" sha1=\"{}\" />\n",
                html_escape(&rom_name),
                file.size,
                file.crc32,
                file.md5,
                file.sha1,
            ));
        }

        xml.push_str("\t</game>\n");
    }

    xml.push_str("</datafile>\n");

    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let filename = format!("{} ({}).dat", sys.name, chrono::Utc::now().format("%Y-%m-%d"));
        zip.start_file(filename, options).map_err(|e| AppError::Internal(e.to_string()))?;
        zip.write_all(xml.as_bytes()).map_err(|e| AppError::Internal(e.to_string()))?;
        zip.finish().map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(buf)
}

async fn generate_cuesheet_archive(pool: &PgPool, system: &str) -> AppResult<Vec<u8>> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    if !sys.media_types.iter().any(|s| MediaType::from_code(s).map_or(false, |m| m.is_cd())) {
        return Err(AppError::NotFound);
    }

    let discs: Vec<DatfileDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.version, d.edition, d.filename_suffix
         FROM discs d
         WHERE d.system_code = $1 AND d.enabled AND NOT d.questionable
         ORDER BY d.title"
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for disc in &discs {
            let files: Vec<File> = sqlx::query_as(
                "SELECT * FROM files WHERE disc_id = $1 AND track_number IS NOT NULL ORDER BY track_number"
            )
            .bind(disc.id)
            .fetch_all(pool)
            .await?;

            let regions = get_disc_region_names(pool, disc.id).await;
            let game_name = build_datfile_name(&disc.title, &regions, disc.version.as_deref(), disc.edition.as_deref(), disc.filename_suffix.as_deref());

            let cue = generate_cuesheet(&game_name, &files);
            let filename = format!("{game_name}.cue");
            zip.start_file(&filename, options).map_err(|e| AppError::Internal(e.to_string()))?;
            zip.write_all(cue.as_bytes()).map_err(|e| AppError::Internal(e.to_string()))?;
        }

        zip.finish().map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(buf)
}

async fn generate_sbi_archive(pool: &PgPool, system: &str) -> AppResult<Vec<u8>> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    if !sys.has_protection_sbi {
        return Err(AppError::NotFound);
    }

    let discs: Vec<SbiDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.protection_sbi
         FROM discs d
         WHERE d.system_code = $1 AND d.protection_sbi IS NOT NULL AND d.enabled AND NOT d.questionable
         ORDER BY d.title"
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for disc in &discs {
            if let Some(ref sbi) = disc.protection_sbi {
                let filename = format!("{}.txt", disc.title);
                zip.start_file(&filename, options).map_err(|e| AppError::Internal(e.to_string()))?;
                zip.write_all(sbi.as_bytes()).map_err(|e| AppError::Internal(e.to_string()))?;
            }
        }

        zip.finish().map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(buf)
}

fn generate_cuesheet(game_name: &str, files: &[File]) -> String {
    let mut cue = String::new();
    for (i, file) in files.iter().enumerate() {
        let track_num = file.track_number.as_deref().unwrap_or("1");
        let track_idx: u32 = track_num.parse().unwrap_or((i + 1) as u32);
        let ext = "bin";
        let track_suffix = if files.len() > 1 {
            format!(" (Track {track_num})")
        } else {
            String::new()
        };
        let bin_name = format!("{game_name}{track_suffix}.{ext}");

        // Determine mode from file size heuristics
        let mode = if file.size % 2352 == 0 {
            "MODE1/2352"
        } else {
            "MODE1/2048"
        };

        cue.push_str(&format!("FILE \"{bin_name}\" BINARY\n"));
        cue.push_str(&format!("  TRACK {:02} {mode}\n", track_idx));
        cue.push_str("    INDEX 01 00:00:00\n");
    }
    cue
}

async fn get_disc_region_names(pool: &PgPool, disc_id: i32) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT r.name FROM disc_regions dr
         JOIN regions r ON r.code = dr.region_code
         WHERE dr.disc_id = $1 ORDER BY r.sort_order"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

fn build_datfile_name(title: &str, regions: &[String], version: Option<&str>, edition: Option<&str>, suffix: Option<&str>) -> String {
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

#[derive(sqlx::FromRow)]
struct DatfileDisc {
    id: i32,
    title: String,
    version: Option<String>,
    edition: Option<String>,
    filename_suffix: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SbiDisc {
    id: i32,
    title: String,
    protection_sbi: Option<String>,
}
