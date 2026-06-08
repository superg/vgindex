use sqlx::PgPool;
use std::io::Write;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::db::models::*;
use crate::error::{AppError, AppResult};

pub struct ArchiveResult {
    pub data: Vec<u8>,
    pub filename: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveMetadata {
    author: String,
    homepage: String,
    url: String,
}

impl ArchiveMetadata {
    pub fn from_site_url(site_url: &str) -> Self {
        let trimmed = site_url.trim().trim_end_matches('/');
        let url = if trimmed.is_empty() {
            "http://localhost/".to_string()
        } else {
            format!("{trimmed}/")
        };
        let host = crate::config::host_from_url(trimmed);
        let site_host = if host.is_empty() {
            "localhost".to_string()
        } else {
            host
        };

        Self {
            author: site_host.clone(),
            homepage: site_host,
            url,
        }
    }
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

fn archive_subdir(system: &str, archive_type: &str) -> String {
    format!(
        "{}/archives/{}-{}",
        crate::config::DATA_DIR,
        system,
        archive_type
    )
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
    metadata: &ArchiveMetadata,
    system: &str,
    archive_type: &str,
) -> AppResult<ArchiveResult> {
    if let Some(result) = find_cached_archive(system, archive_type) {
        return Ok(result);
    }

    let result = match archive_type {
        "dat" => generate_datfile_archive(pool, metadata, system).await?,
        "cue" => generate_cuesheet_archive(pool, system).await?,
        "key" => generate_key_archive(pool, system).await?,
        "sbi" => generate_sbi_archive(pool, system).await?,
        _ => return Err(AppError::NotFound),
    };

    store_archive(system, archive_type, &result);
    Ok(result)
}

pub async fn regenerate_system_archives(pool: &PgPool, metadata: &ArchiveMetadata, system: &str) {
    let sys: Option<System> = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(sys) = sys else { return };

    if let Ok(result) = generate_datfile_archive(pool, metadata, &sys.code).await {
        store_archive(&sys.code, "dat", &result);
    }

    if system_has_cd_media(pool, &sys.media_types).await {
        if let Ok(result) = generate_cuesheet_archive(pool, &sys.code).await {
            store_archive(&sys.code, "cue", &result);
        }
    }

    if sys.has_key {
        if let Ok(result) = generate_key_archive(pool, &sys.code).await {
            store_archive(&sys.code, "key", &result);
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
    metadata: ArchiveMetadata,
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
            regenerate_system_archives(&pool, &metadata, &code).await;
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
    let extensions: Vec<String> =
        sqlx::query_scalar("SELECT rom_extension FROM media_types WHERE code = ANY($1)")
            .bind(media_type_codes)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    extensions.iter().any(|ext| is_cd_rom_extension(ext))
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

// ---------------------------------------------------------------------------
// DAT generation
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct DatfileDisc {
    id: i32,
    title: String,
    filename_suffix: Option<String>,
    disc_number: Option<String>,
    disc_title: Option<String>,
    category_name: String,
    rom_extension: String,
}

async fn generate_datfile_archive(
    pool: &PgPool,
    metadata: &ArchiveMetadata,
    system: &str,
) -> AppResult<ArchiveResult> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let discs: Vec<DatfileDisc> = sqlx::query_as(
        "SELECT d.id, d.title,
                d.filename_suffix, d.disc_number, d.disc_title,
                c.name AS category_name,
                mt.rom_extension
         FROM discs d
         JOIN categories c ON c.id = d.category_id
         JOIN media_types mt ON mt.code = d.media_type_code
         WHERE d.system_code = $1 AND d.status NOT IN ('Disabled', 'Questionable')
         ORDER BY d.title",
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let ts = timestamp_now();
    let disc_count = discs.len();
    let dat_name = sys.dat_system_name();
    let description = format!("{} - Datfile ({}) ({})", dat_name, disc_count, ts);

    let mut xml = format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE datafile PUBLIC "-//Logiqx//DTD ROM Management Datafile//EN" "http://www.logiqx.com/Dats/datafile.dtd">
<datafile>
	<header>
		<name>{name}</name>
		<description>{desc}</description>
		<version>{ts}</version>
		<date>{ts}</date>
		<author>{author}</author>
		<homepage>{homepage}</homepage>
		<url>{url}</url>
	</header>
"#,
        name = html_escape(&dat_name),
        desc = html_escape(&description),
        ts = html_escape(&ts),
        author = html_escape(&metadata.author),
        homepage = html_escape(&metadata.homepage),
        url = html_escape(&metadata.url),
    );

    struct GameEntry {
        id: i32,
        name: String,
        category: String,
        roms: Vec<RomEntry>,
    }
    struct RomEntry {
        name: String,
        size: i64,
        crc: String,
        md5: String,
        sha1: String,
    }

    let mut games: Vec<GameEntry> = Vec::with_capacity(discs.len());
    for disc in &discs {
        let files: Vec<File> = sqlx::query_as("SELECT * FROM files WHERE disc_id = $1")
            .bind(disc.id)
            .fetch_all(pool)
            .await?;

        let region_names = get_disc_region_names(pool, disc.id).await;
        let language_codes = get_disc_language_codes(pool, disc.id).await;
        let game_name = build_rom_base_name(
            &disc.title,
            &region_names,
            &language_codes,
            disc.disc_number.as_deref(),
            disc.disc_title.as_deref(),
            disc.filename_suffix.as_deref(),
        );

        let total_tracks = files.iter().filter(|f| f.track_number.is_some()).count();
        let mut roms: Vec<RomEntry> = files
            .iter()
            .map(|file| {
                let ext = if file.track_number.is_some() {
                    disc.rom_extension.as_str()
                } else {
                    "cue"
                };
                RomEntry {
                    name: build_rom_name(
                        &game_name,
                        file.track_number.as_deref(),
                        total_tracks,
                        ext,
                    ),
                    size: file.size,
                    crc: file.crc32.clone(),
                    md5: file.md5.clone(),
                    sha1: file.sha1.clone(),
                }
            })
            .collect();
        roms.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        games.push(GameEntry {
            id: disc.id,
            name: game_name,
            category: disc.category_name.clone(),
            roms,
        });
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    for game in &games {
        xml.push_str(&format!(
            "\t<game name=\"{name}\" id=\"{id}\">\n\t\t<category>{cat}</category>\n\t\t<description>{name}</description>\n",
            name = html_escape(&game.name),
            id = game.id,
            cat = html_escape(&game.category),
        ));

        for rom in &game.roms {
            xml.push_str(&format!(
                "\t\t<rom name=\"{name}\" size=\"{size}\" crc=\"{crc}\" md5=\"{md5}\" sha1=\"{sha1}\"/>\n",
                name = html_escape(&rom.name),
                size = rom.size,
                crc = rom.crc,
                md5 = rom.md5,
                sha1 = rom.sha1,
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

    let zip_filename = format!("{}.zip", description);
    Ok(ArchiveResult {
        data: buf,
        filename: zip_filename,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_metadata_uses_site_host_and_url() {
        let metadata = ArchiveMetadata::from_site_url("https://redump.info");

        assert_eq!(metadata.author, "redump.info");
        assert_eq!(metadata.homepage, "redump.info");
        assert_eq!(metadata.url, "https://redump.info/");
    }

    #[test]
    fn archive_metadata_strips_www_host_prefix() {
        let metadata = ArchiveMetadata::from_site_url("https://www.redump.info/");

        assert_eq!(metadata.author, "redump.info");
        assert_eq!(metadata.homepage, "redump.info");
        assert_eq!(metadata.url, "https://www.redump.info/");
    }
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
         WHERE d.system_code = $1 AND d.status NOT IN ('Disabled', 'Questionable')
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

    let zip_filename = format!(
        "{} - Cuesheets ({}) ({}).zip",
        sys.dat_system_name(),
        cue_count,
        ts
    );
    Ok(ArchiveResult {
        data: buf,
        filename: zip_filename,
    })
}

// ---------------------------------------------------------------------------
// Keys generation
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct KeyArchiveDisc {
    id: i32,
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    disc_key: Vec<u8>,
}

async fn generate_key_archive(pool: &PgPool, system: &str) -> AppResult<ArchiveResult> {
    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    if !sys.has_key {
        return Err(AppError::NotFound);
    }

    let discs: Vec<KeyArchiveDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix, d.disc_key
         FROM discs d
         WHERE d.system_code = $1 AND d.disc_key IS NOT NULL
               AND d.status NOT IN ('Disabled', 'Questionable')
         ORDER BY d.title",
    )
    .bind(&sys.code)
    .fetch_all(pool)
    .await?;

    let ts = timestamp_now();
    let key_count = discs.len();

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
            let filename = format!("{base_name}.key");
            zip.start_file(&filename, options)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            zip.write_all(&disc.disc_key)
                .map_err(|e| AppError::Internal(e.to_string()))?;
        }

        zip.finish()
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    let zip_filename = format!(
        "{} - Keys ({}) ({}).zip",
        sys.dat_system_name(),
        key_count,
        ts
    );
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
               AND d.status NOT IN ('Disabled', 'Questionable')
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
        sys.dat_system_name(),
        sbi_count,
        ts
    );
    Ok(ArchiveResult {
        data: buf,
        filename: zip_filename,
    })
}
