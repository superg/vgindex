use sqlx::PgPool;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::db::models::*;
use crate::error::{AppError, AppResult};

const ARCHIVE_CACHE_VERSION: &str = "v2";

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
        "{}/archives/{}-{}-{}",
        crate::config::DATA_DIR,
        system,
        archive_type,
        ARCHIVE_CACHE_VERSION
    )
}

fn find_cached_archive(system: &str, archive_type: &str) -> Option<ArchiveResult> {
    let dir = archive_subdir(system, archive_type);
    find_cached_archive_in_dir(Path::new(&dir))
}

fn find_cached_archive_in_dir(dir: &Path) -> Option<ArchiveResult> {
    newest_zip_path(dir).and_then(|path| {
        let filename = path.file_name()?.to_str()?.to_string();
        let data = std::fs::read(&path).ok()?;
        Some(ArchiveResult { data, filename })
    })
}

fn newest_zip_path(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("zip"))
        .max_by_key(|path| {
            path.metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

fn store_archive(system: &str, archive_type: &str, result: &ArchiveResult) -> std::io::Result<()> {
    let dir = archive_subdir(system, archive_type);
    store_archive_in_dir(Path::new(&dir), result)
}

fn store_archive_in_dir(dir: &Path, result: &ArchiveResult) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let final_path = dir.join(&result.filename);
    let tmp_path = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));

    match std::fs::write(&tmp_path, &result.data)
        .and_then(|_| std::fs::rename(&tmp_path, &final_path))
    {
        Ok(()) => {}
        Err(err) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err);
        }
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("zip")
                && path.file_name().and_then(|n| n.to_str()) != Some(result.filename.as_str())
            {
                std::fs::remove_file(&path).ok();
            }
        }
    }
    Ok(())
}

pub fn clear_archives_cache() -> std::io::Result<bool> {
    clear_archives_cache_at(crate::config::DATA_DIR)
}

pub(crate) fn clear_archives_cache_at(data_dir: impl AsRef<Path>) -> std::io::Result<bool> {
    let archives_dir = data_dir.as_ref().join("archives");
    match std::fs::remove_dir_all(&archives_dir) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub async fn get_cached_archive(
    pool: &PgPool,
    system: &str,
    archive_type: &str,
) -> AppResult<ArchiveResult> {
    if !archive_type_is_available(pool, system, archive_type).await? {
        return Err(AppError::NotFound);
    }

    if let Some(result) = find_cached_archive(system, archive_type) {
        return Ok(result);
    }

    Err(AppError::NotFound)
}

pub async fn mark_system_archives_dirty(pool: &PgPool, system: &str) -> AppResult<()> {
    sqlx::query("UPDATE systems SET archives_dirty = TRUE WHERE code = $1")
        .bind(system)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_all_system_archives_dirty(pool: &PgPool) -> AppResult<u64> {
    let result = sqlx::query("UPDATE systems SET archives_dirty = TRUE")
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

pub async fn claim_dirty_archive_systems(pool: &PgPool) -> AppResult<Vec<String>> {
    sqlx::query_scalar(
        "WITH dirty AS (
             SELECT code FROM systems WHERE archives_dirty = TRUE ORDER BY code
         )
         UPDATE systems s
         SET archives_dirty = FALSE
         FROM dirty
         WHERE s.code = dirty.code
         RETURNING s.code",
    )
    .fetch_all(pool)
    .await
    .map_err(AppError::from)
}

pub async fn regenerate_system_archives(
    pool: &PgPool,
    metadata: &ArchiveMetadata,
    system: &str,
) -> AppResult<()> {
    let sys: Option<System> = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?;
    let Some(sys) = sys else { return Ok(()) };

    let result = generate_datfile_archive(pool, metadata, &sys.code).await?;
    store_archive(&sys.code, "dat", &result).map_err(|e| AppError::Internal(e.to_string()))?;

    let has_cue_media = system_has_bin_media(pool, &sys.media_types).await;
    if archive_type_supported_by_system(&sys, "cue", has_cue_media).unwrap_or(false) {
        let result = generate_cuesheet_archive(pool, &sys.code).await?;
        store_archive(&sys.code, "cue", &result).map_err(|e| AppError::Internal(e.to_string()))?;
    }

    if archive_type_supported_by_system(&sys, "key", false).unwrap_or(false) {
        let result = generate_key_archive(pool, &sys.code).await?;
        store_archive(&sys.code, "key", &result).map_err(|e| AppError::Internal(e.to_string()))?;
    }

    if archive_type_supported_by_system(&sys, "sbi", false).unwrap_or(false) {
        let result = generate_sbi_archive(pool, &sys.code).await?;
        store_archive(&sys.code, "sbi", &result).map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(())
}

pub async fn process_dirty_archive_systems(
    pool: PgPool,
    metadata: ArchiveMetadata,
) -> AppResult<Vec<String>> {
    let systems = claim_dirty_archive_systems(&pool).await?;

    for code in &systems {
        tracing::info!("Regenerating archives for system {code}");
        if let Err(err) = regenerate_system_archives(&pool, &metadata, code).await {
            tracing::error!("Failed to regenerate archives for system {code}: {err}");
            let _ = mark_system_archives_dirty(&pool, code).await;
        }
    }

    Ok(systems)
}

pub async fn run_archive_worker(pool: PgPool, metadata: ArchiveMetadata) {
    let mut interval = tokio::time::interval(Duration::from_secs(10 * 60));
    loop {
        interval.tick().await;
        if let Err(err) = process_dirty_archive_systems(pool.clone(), metadata.clone()).await {
            tracing::error!("Failed to process dirty archive systems: {err}");
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn archive_type_supported_by_system(
    sys: &System,
    archive_type: &str,
    has_cue_media: bool,
) -> AppResult<bool> {
    match archive_type {
        "dat" => Ok(true),
        "cue" => Ok(has_cue_media),
        "key" => Ok(sys.has_key),
        "sbi" => Ok(sys.has_sbi),
        _ => Err(AppError::NotFound),
    }
}

async fn archive_type_is_available(
    pool: &PgPool,
    system: &str,
    archive_type: &str,
) -> AppResult<bool> {
    if !matches!(archive_type, "dat" | "cue" | "key" | "sbi") {
        return Err(AppError::NotFound);
    }

    let sys: System = sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(system)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let has_cue_media = if archive_type == "cue" {
        system_has_bin_media(pool, &sys.media_types).await
    } else {
        false
    };

    archive_type_supported_by_system(&sys, archive_type, has_cue_media)
}

async fn system_has_bin_media(pool: &PgPool, media_type_codes: &[String]) -> bool {
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
    media_type_code: String,
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
                d.media_type_code,
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
        let cue_active = dat_disc_has_active_cue(disc, &sys);
        let mut roms: Vec<RomEntry> = files
            .iter()
            .filter(|file| dat_file_is_active(file, cue_active))
            .map(|file| {
                let is_cue = file.track_number.is_none();
                let ext = if is_cue {
                    "cue"
                } else {
                    disc.rom_extension.as_str()
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
                    is_cue,
                }
            })
            .collect();
        sort_dat_rom_entries(&mut roms);

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
    is_cue: bool,
}

fn sort_dat_rom_entries(roms: &mut [RomEntry]) {
    roms.sort_by(|a, b| {
        b.is_cue
            .cmp(&a.is_cue)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

fn dat_disc_has_active_cue(disc: &DatfileDisc, sys: &System) -> bool {
    is_cd_rom_extension(&disc.rom_extension)
        && sys
            .media_types
            .iter()
            .any(|code| code.eq_ignore_ascii_case(&disc.media_type_code))
}

fn dat_file_is_active(file: &File, cue_active: bool) -> bool {
    file.track_number.is_some() || cue_active
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(has_key: bool, has_sbi: bool) -> System {
        System {
            code: "SYS".to_string(),
            system_type: "Console".to_string(),
            manufacturer: "Example".to_string(),
            name: "System".to_string(),
            short_name: String::new(),
            media_types: vec!["cd".to_string(), "gdrom".to_string(), "dvd5".to_string()],
            has_exe_date: false,
            has_sbi,
            has_pvd: false,
            has_edc: false,
            has_disc_id: false,
            has_key,
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

    fn dat_disc(media_type_code: &str, rom_extension: &str) -> DatfileDisc {
        DatfileDisc {
            id: 1,
            title: "Game".to_string(),
            filename_suffix: None,
            disc_number: None,
            disc_title: None,
            category_name: "Games".to_string(),
            media_type_code: media_type_code.to_string(),
            rom_extension: rom_extension.to_string(),
        }
    }

    fn file(track_number: Option<&str>) -> File {
        File {
            id: 1,
            disc_id: 1,
            track_number: track_number.map(str::to_string),
            size: 0,
            crc32: String::new(),
            md5: String::new(),
            sha1: String::new(),
        }
    }

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

    #[test]
    fn dat_rom_entries_put_cue_first_then_sort_alphabetically() {
        let mut roms = vec![
            rom_entry("Game (Track 2).bin", false),
            rom_entry("Game (Track 1).bin", false),
            rom_entry("Game.cue", true),
        ];

        sort_dat_rom_entries(&mut roms);

        let names: Vec<&str> = roms.iter().map(|rom| rom.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Game.cue", "Game (Track 1).bin", "Game (Track 2).bin"]
        );
    }

    #[test]
    fn dat_cue_entries_are_active_only_for_supported_bin_media() {
        let sys = system(false, false);

        assert!(dat_disc_has_active_cue(&dat_disc("cd", "bin"), &sys));
        assert!(dat_disc_has_active_cue(&dat_disc("gdrom", "BIN"), &sys));
        assert!(!dat_disc_has_active_cue(&dat_disc("dvd5", "iso"), &sys));
        assert!(!dat_disc_has_active_cue(&dat_disc("other", "bin"), &sys));
    }

    #[test]
    fn dat_file_filter_keeps_non_cue_rows_and_gates_cue_rows() {
        let cue_file = file(None);
        let track_file = file(Some("1"));

        assert!(dat_file_is_active(&track_file, false));
        assert!(!dat_file_is_active(&cue_file, false));
        assert!(dat_file_is_active(&cue_file, true));
    }

    #[test]
    fn archive_availability_uses_system_flags_for_key_and_sbi() {
        let key_system = system(true, false);
        let sbi_system = system(false, true);
        let plain_system = system(false, false);

        assert!(archive_type_supported_by_system(&key_system, "key", false).unwrap());
        assert!(!archive_type_supported_by_system(&plain_system, "key", false).unwrap());
        assert!(archive_type_supported_by_system(&sbi_system, "sbi", false).unwrap());
        assert!(!archive_type_supported_by_system(&plain_system, "sbi", false).unwrap());
    }

    #[test]
    fn archive_availability_uses_bin_media_for_cues() {
        let sys = system(false, false);

        assert!(archive_type_supported_by_system(&sys, "cue", true).unwrap());
        assert!(!archive_type_supported_by_system(&sys, "cue", false).unwrap());
    }

    #[test]
    fn store_archive_replaces_old_zip_only_after_new_zip_is_published() {
        let root = std::env::temp_dir().join(format!(
            "vgindex-archive-store-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("old.zip"), b"old").unwrap();

        let result = ArchiveResult {
            filename: "new.zip".to_string(),
            data: b"new".to_vec(),
        };

        store_archive_in_dir(&root, &result).unwrap();

        assert_eq!(std::fs::read(root.join("new.zip")).unwrap(), b"new");
        assert!(!root.join("old.zip").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn store_archive_keeps_old_zip_when_publish_fails() {
        let root = std::env::temp_dir().join(format!(
            "vgindex-archive-store-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("old.zip"), b"old").unwrap();
        std::fs::create_dir(root.join("new.zip")).unwrap();

        let result = ArchiveResult {
            filename: "new.zip".to_string(),
            data: b"new".to_vec(),
        };

        store_archive_in_dir(&root, &result).unwrap_err();

        assert_eq!(std::fs::read(root.join("old.zip")).unwrap(), b"old");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn clear_archives_cache_removes_only_archives_directory() {
        let root = std::env::temp_dir().join(format!(
            "vgindex-archive-cache-test-{}",
            uuid::Uuid::new_v4()
        ));
        let archives = root.join("archives").join("SYS-dat-v2");
        std::fs::create_dir_all(&archives).unwrap();
        std::fs::write(archives.join("cached.zip"), b"zip").unwrap();
        std::fs::write(root.join("keep.txt"), b"keep").unwrap();

        let removed = clear_archives_cache_at(&root).unwrap();

        assert!(removed);
        assert!(!root.join("archives").exists());
        assert_eq!(std::fs::read(root.join("keep.txt")).unwrap(), b"keep");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn clear_archives_cache_is_ok_when_cache_is_absent() {
        let root = std::env::temp_dir().join(format!(
            "vgindex-archive-cache-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let removed = clear_archives_cache_at(&root).unwrap();

        assert!(!removed);

        std::fs::remove_dir_all(&root).unwrap();
    }

    fn rom_entry(name: &str, is_cue: bool) -> RomEntry {
        RomEntry {
            name: name.to_string(),
            size: 0,
            crc: String::new(),
            md5: String::new(),
            sha1: String::new(),
            is_cue,
        }
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

    if !system_has_bin_media(pool, &sys.media_types).await {
        return Err(AppError::NotFound);
    }

    let discs: Vec<CueDisc> = sqlx::query_as(
        "SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix, d.cue
         FROM discs d
         JOIN media_types mt ON mt.code = d.media_type_code
         WHERE d.system_code = $1 AND d.status NOT IN ('Disabled', 'Questionable')
               AND d.media_type_code = ANY($2)
               AND LOWER(mt.rom_extension) = 'bin'
               AND d.cue IS NOT NULL AND d.cue != ''
         ORDER BY d.title",
    )
    .bind(&sys.code)
    .bind(&sys.media_types)
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
