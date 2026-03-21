use sqlx::PgPool;

use crate::db::models::*;
use crate::error::{AppError, AppResult};

pub async fn get_all_systems(pool: &PgPool) -> AppResult<Vec<System>> {
    Ok(sqlx::query_as("SELECT * FROM systems ORDER BY sort_order, full_name")
        .fetch_all(pool)
        .await?)
}

pub async fn get_system(pool: &PgPool, code: &str) -> AppResult<System> {
    sqlx::query_as("SELECT * FROM systems WHERE code = $1")
        .bind(code)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn get_disc_detail(pool: &PgPool, disc_id: i32) -> AppResult<DiscDetail> {
    let disc: Disc = sqlx::query_as("SELECT * FROM discs WHERE id = $1")
        .bind(disc_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let system = get_system(pool, &disc.system_code).await?;

    let regions: Vec<Region> = sqlx::query_as(
        "SELECT r.* FROM regions r
         JOIN disc_regions dr ON dr.region_code = r.code
         WHERE dr.disc_id = $1 ORDER BY r.sort_order"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let languages: Vec<Language> = sqlx::query_as(
        "SELECT l.* FROM languages l
         JOIN disc_languages dl ON dl.language_id = l.id
         WHERE dl.disc_id = $1 ORDER BY l.sort_order"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let ring_entries: Vec<DiscRingCodeEntry> = sqlx::query_as(
        "SELECT * FROM disc_ring_code_entries WHERE disc_id = $1 ORDER BY id"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let mut ring_views = Vec::new();
    for entry in &ring_entries {
        let layers: Vec<DiscRingCodeLayer> = sqlx::query_as(
            "SELECT * FROM disc_ring_code_layers WHERE entry_id = $1 ORDER BY layer"
        )
        .bind(entry.id)
        .fetch_all(pool)
        .await?;
        ring_views.push(RingEntryView { layers });
    }

    let files: Vec<File> = sqlx::query_as(
        "SELECT * FROM files WHERE disc_id = $1 ORDER BY track_number"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let dumpers: Vec<DumperInfo> = sqlx::query_as(
        "SELECT u.id AS user_id, u.username FROM disc_dumpers dd
         JOIN users u ON u.id = dd.user_id
         WHERE dd.disc_id = $1"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    Ok(DiscDetail {
        disc,
        system,
        regions,
        languages,
        ring_entries: ring_views,
        files,
        dumpers,
    })
}

pub async fn update_disc(pool: &PgPool, disc_id: i32, data: &serde_json::Value) -> AppResult<()> {
    let title = data["title"].as_str().unwrap_or_default();
    let title_foreign = data["title_foreign"].as_str().filter(|s| !s.is_empty());
    let title_disc = data["title_disc"].as_str().filter(|s| !s.is_empty());
    let title_disc_number = data["title_disc_number"].as_str().filter(|s| !s.is_empty());
    let serial = data["serial"].as_str().filter(|s| !s.is_empty());
    let category = data["category"].as_str().unwrap_or("Games");
    let version = data["version"].as_str().filter(|s| !s.is_empty());
    let edition = data["edition"].as_str().filter(|s| !s.is_empty());
    let barcode = data["barcode"].as_str().filter(|s| !s.is_empty());
    let comments = data["comments"].as_str().filter(|s| !s.is_empty());
    let protection = data["protection"].as_str().filter(|s| !s.is_empty());
    let error_count = data["error_count"].as_i64().map(|v| v as i32);

    sqlx::query(
        "UPDATE discs SET title = $1,
         title_foreign = $2, title_disc = $3, title_disc_number = $4,
         serial = $5,
         category_id = (SELECT id FROM categories WHERE name = $6),
         version = $7, edition = $8, barcode = $9,
         comments = $10, protection = $11, error_count = $12, updated_at = NOW()
         WHERE id = $13"
    )
    .bind(title)
    .bind(title_foreign)
    .bind(title_disc)
    .bind(title_disc_number)
    .bind(serial)
    .bind(category)
    .bind(version)
    .bind(edition)
    .bind(barcode)
    .bind(comments)
    .bind(protection)
    .bind(error_count)
    .bind(disc_id)
    .execute(pool)
    .await?;

    // Update regions
    sqlx::query("DELETE FROM disc_regions WHERE disc_id = $1")
        .bind(disc_id)
        .execute(pool)
        .await?;
    if let Some(regions) = data["regions"].as_array() {
        for r in regions {
            if let Some(rcode) = r.as_str() {
                sqlx::query(
                    "INSERT INTO disc_regions (disc_id, region_code) VALUES ($1, $2)
                     ON CONFLICT DO NOTHING"
                )
                .bind(disc_id)
                .bind(rcode)
                .execute(pool)
                .await?;
            }
        }
    }

    // Update languages
    sqlx::query("DELETE FROM disc_languages WHERE disc_id = $1")
        .bind(disc_id)
        .execute(pool)
        .await?;
    if let Some(langs) = data["languages"].as_array() {
        for l in langs {
            if let Some(lid) = l.as_i64() {
                sqlx::query(
                    "INSERT INTO disc_languages (disc_id, language_id) VALUES ($1, $2)
                     ON CONFLICT DO NOTHING"
                )
                .bind(disc_id)
                .bind(lid as i32)
                .execute(pool)
                .await?;
            }
        }
    }

    Ok(())
}

pub async fn create_disc_from_submission(
    pool: &PgPool,
    data: &serde_json::Value,
    submitter_id: i32,
) -> AppResult<i32> {
    let system_code = data["system_code"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(AppError::BadRequest("system_code required".into()))?;
    let media_type = data["media_type"].as_str().unwrap_or("cd");
    let title = data["title"].as_str().unwrap_or_default();
    let category = data["category"].as_str().unwrap_or("Games");

    let disc_id: i32 = sqlx::query_scalar(
        "INSERT INTO discs (system_code, media_type_code, title, category_id, status)
         VALUES ($1, $2, $3,
                 (SELECT id FROM categories WHERE name = $4), 'Good')
         RETURNING id"
    )
    .bind(system_code)
    .bind(media_type)
    .bind(title)
    .bind(category)
    .fetch_one(pool)
    .await?;

    update_disc(pool, disc_id, data).await?;

    // Add submitter as dumper
    sqlx::query("INSERT INTO disc_dumpers (disc_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(disc_id)
        .bind(submitter_id)
        .execute(pool)
        .await?;

    // Parse and insert files from files_xml
    if let Some(files_xml) = data["files_xml"].as_str() {
        parse_and_insert_files(pool, disc_id, files_xml).await?;
    }

    Ok(disc_id)
}

async fn parse_and_insert_files(pool: &PgPool, disc_id: i32, files_xml: &str) -> AppResult<()> {
    for line in files_xml.lines() {
        let line = line.trim();
        if !line.starts_with("<rom ") {
            continue;
        }
        let name = extract_attr(line, "name").unwrap_or_default();
        let size: i64 = extract_attr(line, "size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let crc = extract_attr(line, "crc").unwrap_or_default();
        let md5 = extract_attr(line, "md5").unwrap_or_default();
        let sha1 = extract_attr(line, "sha1").unwrap_or_default();

        let track_number = extract_track_number(&name);

        sqlx::query(
            "INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1)
             VALUES ($1, $2, $3, $4, $5, $6)"
        )
        .bind(disc_id)
        .bind(&track_number)
        .bind(size)
        .bind(&crc)
        .bind(&md5)
        .bind(&sha1)
        .execute(pool)
        .await?;
    }
    Ok(())
}

fn extract_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn extract_track_number(filename: &str) -> Option<String> {
    // e.g. "Title (USA) (Track 1).bin" -> "1"
    // e.g. "Title (USA).iso" -> "1"
    if filename.ends_with(".iso") {
        return Some("1".to_string());
    }
    if let Some(pos) = filename.to_lowercase().find("track ") {
        let rest = &filename[pos + 6..];
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if !num.is_empty() {
            return Some(num);
        }
    }
    None
}

// DumperInfo needs FromRow
impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for DumperInfo {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            user_id: row.try_get("user_id")?,
            username: row.try_get("username")?,
        })
    }
}
