use sqlx::PgPool;

use crate::db::models::*;
use crate::error::{AppError, AppResult};

fn parse_hex_dump(text: &str) -> Vec<u8> {
    let mut result = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let after_colon = match line.find(':') {
            Some(pos) => &line[pos + 1..],
            None => continue,
        };
        let trimmed = after_colon.trim_start();
        let hex_part = match trimmed.find("   ") {
            Some(pos) => &trimmed[..pos],
            None => trimmed,
        };
        for token in hex_part.split_whitespace() {
            if token.len() == 2 {
                if let Ok(b) = u8::from_str_radix(token, 16) {
                    result.push(b);
                }
            }
        }
    }
    result
}

fn parse_pvd_hex_dump(text: &str) -> Vec<u8> {
    let mut result = parse_hex_dump(text);
    result.truncate(82);
    result
}

fn parse_text_array(val: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = val.as_array() {
        arr.iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if let Some(s) = val.as_str() {
        s.split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect()
    } else {
        Vec::new()
    }
}

pub async fn get_all_systems(pool: &PgPool) -> AppResult<Vec<System>> {
    Ok(sqlx::query_as("SELECT * FROM systems ORDER BY sort_order, name")
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
         JOIN disc_languages dl ON dl.language_code = l.code
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
        ring_views.push(RingEntryView {
            offset_value: entry.offset_value.clone(),

            sample_data_start: entry.sample_data_start.clone(),
            comment: entry.comment.clone(),
            layers,
        });
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

    let added_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT MIN(created_at) FROM disc_submissions WHERE target_disc_id = $1"
    )
    .bind(disc_id)
    .fetch_one(pool)
    .await?;

    let modified_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM disc_submissions WHERE target_disc_id = $1"
    )
    .bind(disc_id)
    .fetch_one(pool)
    .await?;

    let protection_ranges: Vec<ProtectionRange> = sqlx::query_as(
        "SELECT lower(r)::INT AS range_start, upper(r)::INT AS range_end \
         FROM discs, unnest(protection_ranges) AS r WHERE id = $1 ORDER BY lower(r)"
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
        protection_ranges,
        added_at,
        modified_at,
    })
}

pub async fn update_disc(pool: &PgPool, disc_id: i32, data: &serde_json::Value) -> AppResult<()> {
    let title = data["title"].as_str().unwrap_or_default();
    let title_foreign = data["title_foreign"].as_str().filter(|s| !s.is_empty());
    let title_disc = data["title_disc"].as_str().filter(|s| !s.is_empty());
    let title_disc_number = data["title_disc_number"].as_str().filter(|s| !s.is_empty());
    let serial = parse_text_array(&data["serial"]);
    let category = data["category"].as_str().unwrap_or("Games");
    let version = data["version"].as_str().filter(|s| !s.is_empty());
    let edition = parse_text_array(&data["edition"]);
    let barcode = parse_text_array(&data["barcode"]);
    let comments = data["comments"].as_str().filter(|s| !s.is_empty());
    let protection = data["protection"].as_str().filter(|s| !s.is_empty());
    let error_count = data["error_count"].as_i64().map(|v| v as i32);
    let pvd = data["pvd"].as_str()
        .filter(|s| !s.is_empty())
        .map(parse_pvd_hex_dump);
    let header = data["header"].as_str()
        .filter(|s| !s.is_empty())
        .map(|s| parse_hex_dump(s));

    sqlx::query(
        "UPDATE discs SET title = $1,
         title_foreign = $2, title_disc = $3, title_disc_number = $4,
         serial = $5,
         category_id = (SELECT id FROM categories WHERE name = $6),
         version = $7, edition = $8, barcode = $9,
         comments = $10, protection = $11, error_count = $12,
         pvd = $13, header = $14
         WHERE id = $15"
    )
    .bind(title)
    .bind(title_foreign)
    .bind(title_disc)
    .bind(title_disc_number)
    .bind(&serial)
    .bind(category)
    .bind(version)
    .bind(&edition)
    .bind(&barcode)
    .bind(comments)
    .bind(protection)
    .bind(error_count)
    .bind(&pvd)
    .bind(&header)
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
            if let Some(lcode) = l.as_str() {
                sqlx::query(
                    "INSERT INTO disc_languages (disc_id, language_code) VALUES ($1, $2)
                     ON CONFLICT DO NOTHING"
                )
                .bind(disc_id)
                .bind(lcode)
                .execute(pool)
                .await?;
            }
        }
    }

    regenerate_cue_entry(pool, disc_id).await?;

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
        "INSERT INTO discs (system_code, media_type_code, title, category_id)
         VALUES ($1, $2, $3,
                 (SELECT id FROM categories WHERE name = $4))
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

    regenerate_cue_entry(pool, disc_id).await?;

    Ok(disc_id)
}

pub async fn regenerate_cue_entry(pool: &PgPool, disc_id: i32) -> AppResult<()> {
    let disc: Disc = sqlx::query_as("SELECT * FROM discs WHERE id = $1")
        .bind(disc_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let raw_cue = match &disc.cue {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(()),
    };

    let region_names: Vec<String> = sqlx::query_scalar(
        "SELECT r.name FROM regions r
         JOIN disc_regions dr ON dr.region_code = r.code
         WHERE dr.disc_id = $1 ORDER BY r.sort_order"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let language_codes: Vec<String> = sqlx::query_scalar(
        "SELECT l.code FROM languages l
         JOIN disc_languages dl ON dl.language_code = l.code
         WHERE dl.disc_id = $1 ORDER BY l.sort_order"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let base_name = build_rom_base_name(
        &disc.title,
        &region_names,
        &language_codes,
        disc.title_disc_number.as_deref(),
        disc.title_disc.as_deref(),
        disc.filename_suffix.as_deref(),
    );
    let ext = disc.media_type.rom_extension();

    let finalized = finalize_cue(raw_cue, &base_name, ext);

    sqlx::query("UPDATE discs SET cue = $1 WHERE id = $2")
        .bind(&finalized)
        .bind(disc_id)
        .execute(pool)
        .await?;

    let (size, crc32, md5, sha1) = compute_file_hashes(finalized.as_bytes());

    sqlx::query(
        "INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1)
         VALUES ($1, NULL, $2, $3, $4, $5)
         ON CONFLICT (disc_id) WHERE track_number IS NULL
         DO UPDATE SET size = $2, crc32 = $3, md5 = $4, sha1 = $5"
    )
    .bind(disc_id)
    .bind(size)
    .bind(&crc32)
    .bind(&md5)
    .bind(&sha1)
    .execute(pool)
    .await?;

    Ok(())
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
