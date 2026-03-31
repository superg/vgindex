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

fn parse_comma_separated(s: &str) -> Vec<String> {
    s.split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

pub async fn get_all_systems(pool: &PgPool) -> AppResult<Vec<System>> {
    Ok(sqlx::query_as("SELECT * FROM systems ORDER BY LOWER(name), name")
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

async fn enrich_media_type(pool: &PgPool, disc: &mut Disc) -> AppResult<()> {
    let row: MediaTypeRow = sqlx::query_as(
        "SELECT code, name, layer_count, rom_extension FROM media_types WHERE code = $1",
    )
    .bind(disc.media_type.code())
    .fetch_one(pool)
    .await?;
    disc.media_type = row.into();
    Ok(())
}

pub async fn get_disc_detail(pool: &PgPool, disc_id: i32) -> AppResult<DiscDetail> {
    let mut disc: Disc = sqlx::query_as("SELECT * FROM discs WHERE id = $1")
        .bind(disc_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;
    enrich_media_type(pool, &mut disc).await?;

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
            offset_value: entry.offset_value,
            offset_extra_value: entry.offset_extra_value,
            sample_data_start: entry.sample_data_start,
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

    let sector_ranges: Vec<ProtectionRange> = sqlx::query_as(
        "SELECT lower(r)::INT AS range_start, upper(r)::INT AS range_end \
         FROM discs, unnest(sector_ranges) AS r WHERE id = $1 ORDER BY lower(r)"
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
        sector_ranges,
        added_at,
        modified_at,
    })
}

pub async fn update_disc(pool: &PgPool, disc_id: i32, data: &serde_json::Value) -> AppResult<()> {
    let title = data["title"].as_str().unwrap_or_default();
    let system_code = data["system_code"].as_str();
    let media_type = data["media_type"].as_str();
    let title_foreign = data["title_foreign"].as_str().filter(|s| !s.is_empty());
    let disc_title = data["disc_title"].as_str().filter(|s| !s.is_empty());
    let disc_number = data["disc_number"].as_str().filter(|s| !s.is_empty());
    let filename_suffix = data["filename_suffix"].as_str().filter(|s| !s.is_empty());
    let serial = parse_text_array(&data["serial"]);
    let category = data["category"].as_str().unwrap_or("Games");
    let version = data["version"].as_str().filter(|s| !s.is_empty());
    let edition = parse_text_array(&data["edition"]);
    let barcode = parse_text_array(&data["barcode"]);
    let comments = data["comments"].as_str().filter(|s| !s.is_empty());
    let contents = data["contents"].as_str().filter(|s| !s.is_empty());
    let protection = data["protection"].as_str().filter(|s| !s.is_empty());
    let sbi = data["sbi"].as_str().filter(|s| !s.is_empty());
    let keys = parse_text_array(&data["keys"]);
    let keys_opt: Option<Vec<String>> = if keys.is_empty() {
        None
    } else {
        Some(keys)
    };
    let error_count = data["error_count"].as_i64().map(|v| v as i32);
    let exe_date = data["exe_date"].as_str().filter(|s| !s.is_empty());
    let edc = if data["edc"].is_boolean() {
        data["edc"].as_bool()
    } else {
        None
    };
    let pvd = data["pvd"].as_str()
        .filter(|s| !s.is_empty())
        .map(parse_pvd_hex_dump);
    let pic = data["pic"].as_str()
        .filter(|s| !s.is_empty())
        .map(|s| parse_hex_dump(s));
    let bca = data["bca"].as_str()
        .filter(|s| !s.is_empty())
        .map(|s| parse_hex_dump(s));
    let header = data["header"].as_str()
        .filter(|s| !s.is_empty())
        .map(|s| parse_hex_dump(s));
    let cue = data["cue"].as_str().filter(|s| !s.is_empty());
    let layerbreaks: Option<Vec<i32>> = if let Some(arr) = data["layerbreaks"].as_array() {
        let v: Vec<i32> = arr.iter().filter_map(|x| x.as_i64().map(|n| n as i32)).collect();
        if v.is_empty() { None } else { Some(v) }
    } else {
        None
    };
    let questionable = data["questionable"].as_bool().unwrap_or(false);
    let enabled = data["enabled"].as_bool().unwrap_or(true);

    sqlx::query(
        "UPDATE discs SET title = $1,
         system_code = COALESCE($2, system_code),
         media_type_code = COALESCE($3, media_type_code),
         category_id = (SELECT id FROM categories WHERE name = $4),
         title_foreign = $5, disc_title = $6, disc_number = $7,
         filename_suffix = $8,
         serial = $9, version = $10, edition = $11, barcode = $12,
         comments = $13, contents = $14,
         error_count = $15, exe_date = $16, edc = $17,
         layerbreaks = $18,
         pvd = $19, pic = $20, bca = $21, header = $22,
         protection = $23, sbi = $24, keys = $25,
         cue = $26,
         questionable = $27, enabled = $28
         WHERE id = $29"
    )
    .bind(title)               // $1
    .bind(system_code)         // $2
    .bind(media_type)          // $3
    .bind(category)            // $4
    .bind(title_foreign)       // $5
    .bind(disc_title)          // $6
    .bind(disc_number)         // $7
    .bind(filename_suffix)     // $8
    .bind(&serial)             // $9
    .bind(version)             // $10
    .bind(&edition)            // $11
    .bind(&barcode)            // $12
    .bind(comments)            // $13
    .bind(contents)            // $14
    .bind(error_count)         // $15
    .bind(exe_date)            // $16
    .bind(edc)                 // $17
    .bind(&layerbreaks)        // $18
    .bind(&pvd)                // $19
    .bind(&pic)                // $20
    .bind(&bca)                // $21
    .bind(&header)             // $22
    .bind(protection)          // $23
    .bind(sbi)                 // $24
    .bind(&keys_opt)            // $25
    .bind(cue)                 // $26
    .bind(questionable)        // $27
    .bind(enabled)             // $28
    .bind(disc_id)             // $29
    .execute(pool)
    .await?;

    // Sector ranges (INT4RANGE[] needs special handling)
    if let Some(ranges) = data["sector_ranges"].as_array() {
        if ranges.is_empty() {
            sqlx::query("UPDATE discs SET sector_ranges = NULL WHERE id = $1")
                .bind(disc_id)
                .execute(pool)
                .await?;
        } else {
            let range_strs: Vec<String> = ranges
                .iter()
                .filter_map(|r| {
                    let start = r["start"].as_i64()?;
                    let end = r["end"].as_i64()?;
                    Some(format!("\"[{},{})\"", start, end))
                })
                .collect();
            let array_literal = format!("{{{}}}", range_strs.join(","));
            sqlx::query("UPDATE discs SET sector_ranges = $1::INT4RANGE[] WHERE id = $2")
                .bind(&array_literal)
                .bind(disc_id)
                .execute(pool)
                .await?;
        }
    }

    // Regions
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

    // Languages
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

    // Ring codes
    if let Some(ring_codes) = data["ring_codes"].as_array() {
        sqlx::query("DELETE FROM disc_ring_code_entries WHERE disc_id = $1")
            .bind(disc_id)
            .execute(pool)
            .await?;

        for entry_data in ring_codes {
            let offset_value = entry_data["offset_value"]
                .as_str()
                .and_then(|s| s.trim().parse::<i32>().ok());
            let offset_extra_value = entry_data["offset_extra_value"]
                .as_str()
                .and_then(|s| s.trim().parse::<i32>().ok());
            let sample_start = entry_data["sample_start"]
                .as_str()
                .and_then(|s| s.trim().parse::<i32>().ok());
            let comment = entry_data["comment"].as_str().filter(|s| !s.is_empty());

            let entry_id: i32 = sqlx::query_scalar(
                "INSERT INTO disc_ring_code_entries (disc_id, offset_value, offset_extra_value, sample_data_start, comment)
                 VALUES ($1, $2, $3, $4, $5) RETURNING id"
            )
            .bind(disc_id)
            .bind(offset_value)
            .bind(offset_extra_value)
            .bind(sample_start)
            .bind(comment)
            .fetch_one(pool)
            .await?;

            if let Some(layers) = entry_data["layers"].as_array() {
                for (li, layer_data) in layers.iter().enumerate() {
                    let mc = layer_data["mastering_code"].as_str().filter(|s| !s.is_empty());
                    let ms = layer_data["mastering_sid"].as_str().filter(|s| !s.is_empty());
                    let mould_sids = parse_comma_separated(
                        layer_data["mould_sids"].as_str().unwrap_or(""),
                    );
                    let toolstamps = parse_comma_separated(
                        layer_data["toolstamps"].as_str().unwrap_or(""),
                    );
                    let additional_moulds = parse_comma_separated(
                        layer_data["additional_moulds"].as_str().unwrap_or(""),
                    );

                    let has_data = mc.is_some()
                        || ms.is_some()
                        || !mould_sids.is_empty()
                        || !toolstamps.is_empty()
                        || !additional_moulds.is_empty();
                    if has_data {
                        sqlx::query(
                            "INSERT INTO disc_ring_code_layers
                             (entry_id, layer, mastering_code, mastering_sid, mould_sids, toolstamps, additional_moulds)
                             VALUES ($1, $2, $3, $4, $5, $6, $7)"
                        )
                        .bind(entry_id)
                        .bind(li as i32)
                        .bind(mc)
                        .bind(ms)
                        .bind(&mould_sids)
                        .bind(&toolstamps)
                        .bind(&additional_moulds)
                        .execute(pool)
                        .await?;
                    }
                }
            }
        }
    }

    // Files (non-cue) from XML
    if let Some(files_xml) = data["files_xml"].as_str() {
        sqlx::query("DELETE FROM files WHERE disc_id = $1 AND track_number IS NOT NULL")
            .bind(disc_id)
            .execute(pool)
            .await?;
        if !files_xml.is_empty() {
            parse_and_insert_files(pool, disc_id, files_xml).await?;
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

    sqlx::query("INSERT INTO disc_dumpers (disc_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(disc_id)
        .bind(submitter_id)
        .execute(pool)
        .await?;

    Ok(disc_id)
}

pub async fn regenerate_cue_entry(pool: &PgPool, disc_id: i32) -> AppResult<()> {
    let mut disc: Disc = sqlx::query_as("SELECT * FROM discs WHERE id = $1")
        .bind(disc_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;
    enrich_media_type(pool, &mut disc).await?;

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
        disc.disc_number.as_deref(),
        disc.disc_title.as_deref(),
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
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (disc_id, track_number) DO UPDATE
             SET size = $3, crc32 = $4, md5 = $5, sha1 = $6"
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
    if filename.ends_with(".iso") {
        return Some("1".to_string());
    }
    let lower = filename.to_lowercase();
    if lower.starts_with("track.") {
        return Some("1".to_string());
    }
    if let Some(pos) = lower.find("track ") {
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
