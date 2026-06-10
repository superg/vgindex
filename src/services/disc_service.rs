use sqlx::PgPool;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::db::models::*;
use crate::error::{AppError, AppResult};

const EDITION_USAGE_COUNTS_SQL: &str = "\
SELECT d.system_code,
       btrim(e.edition) AS edition,
       COUNT(DISTINCT d.id)::BIGINT AS edition_count
FROM discs d
CROSS JOIN LATERAL unnest(d.edition) AS e(edition)
WHERE d.status <> 'Disabled'
  AND btrim(e.edition) <> ''
GROUP BY d.system_code, btrim(e.edition)";

fn to_lf_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn to_crlf_newlines(s: &str) -> String {
    to_lf_newlines(s).replace('\n', "\r\n")
}

fn normalize_newlines(s: &str) -> String {
    to_lf_newlines(s)
}

pub(crate) fn parse_binary_hex_input(text: &str) -> Result<Vec<u8>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }

    let has_offset_prefixes = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .any(|line| line.contains(':'));

    if has_offset_prefixes {
        parse_addressed_hex_dump(text)
    } else {
        parse_raw_hex_input(text)
    }
}

fn parse_addressed_hex_dump(text: &str) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    for (line_num, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row = line_num + 1;
        let colon_pos = match line.find(':') {
            Some(pos) => pos,
            None => return Err(format!("line {row}: missing offset:colon prefix")),
        };
        let offset_part = line[..colon_pos].trim();
        if offset_part.is_empty() || !offset_part.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("line {row}: invalid hex offset '{offset_part}'"));
        }
        let after_colon = &line[colon_pos + 1..];
        let trimmed = after_colon.trim_start();
        let hex_part = match trimmed.find("   ") {
            Some(pos) => &trimmed[..pos],
            None => trimmed,
        };
        let mut line_bytes = 0usize;
        for token in hex_part.split_whitespace() {
            if token.len() != 2 {
                return Err(format!("line {row}: invalid hex token '{token}'"));
            }
            let byte = u8::from_str_radix(token, 16)
                .map_err(|_| format!("line {row}: invalid hex byte '{token}'"))?;
            result.push(byte);
            line_bytes += 1;
        }
        if line_bytes == 0 {
            return Err(format!("line {row}: no hex bytes found"));
        }
    }
    if result.is_empty() {
        return Err("no hex data found".into());
    }
    Ok(result)
}

fn parse_raw_hex_input(text: &str) -> Result<Vec<u8>, String> {
    let mut hex = String::new();
    for (line_num, line) in text.lines().enumerate() {
        let row = line_num + 1;
        for ch in line.chars() {
            if ch.is_ascii_whitespace() {
                continue;
            }
            if !ch.is_ascii_hexdigit() {
                return Err(format!("line {row}: invalid raw hex character '{ch}'"));
            }
            hex.push(ch);
        }
    }

    if hex.is_empty() {
        return Err("no hex data found".into());
    }
    if hex.len() % 2 != 0 {
        return Err("raw hex data must contain an even number of hexadecimal digits".into());
    }

    let mut result = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let token = std::str::from_utf8(pair)
            .map_err(|_| "raw hex data contains invalid UTF-8".to_string())?;
        let byte =
            u8::from_str_radix(token, 16).map_err(|_| format!("invalid hex byte '{token}'"))?;
        result.push(byte);
    }
    Ok(result)
}

fn parse_pvd_hex_dump(text: &str) -> Result<Vec<u8>, String> {
    let mut result = parse_binary_hex_input(text)?;
    result.truncate(82);
    Ok(result)
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

fn parse_hex_text_bytes(val: Option<&str>) -> AppResult<Option<Vec<u8>>> {
    let Some(raw) = val else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(
            "disc_key must contain only hexadecimal characters".into(),
        ));
    }
    if trimmed.len() % 2 != 0 {
        return Err(AppError::BadRequest(
            "disc_key must contain an even number of hexadecimal characters".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(trimmed.len() / 2);
    for pair in trimmed.as_bytes().chunks_exact(2) {
        let token = std::str::from_utf8(pair)
            .map_err(|_| AppError::BadRequest("disc_key contains invalid UTF-8".into()))?;
        let byte = u8::from_str_radix(token, 16)
            .map_err(|_| AppError::BadRequest("disc_key contains invalid hex bytes".into()))?;
        bytes.push(byte);
    }
    Ok(Some(bytes))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct EditionUsageCount {
    pub system_code: String,
    pub edition: String,
    pub edition_count: i64,
}

#[derive(Clone)]
pub struct EditionSuggestionsCache {
    inner: Arc<RwLock<CachedEditionSuggestions>>,
    ttl: Duration,
}

#[derive(Default)]
struct CachedEditionSuggestions {
    loaded_at: Option<Instant>,
    suggestions: BTreeMap<String, Vec<String>>,
}

impl EditionSuggestionsCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(CachedEditionSuggestions::default())),
            ttl,
        }
    }

    pub async fn get(&self, pool: &PgPool) -> AppResult<BTreeMap<String, Vec<String>>> {
        let now = Instant::now();
        {
            let cached = self.inner.read().await;
            if cached.is_fresh(now, self.ttl) {
                return Ok(cached.suggestions.clone());
            }
        }

        let stale = {
            let cached = self.inner.read().await;
            if cached.loaded_at.is_some() {
                Some(cached.suggestions.clone())
            } else {
                None
            }
        };

        let suggestions = match fetch_edition_suggestion_map(pool).await {
            Ok(suggestions) => suggestions,
            Err(err) => {
                if let Some(stale) = stale {
                    tracing::warn!(
                        "Failed to refresh edition suggestions; using stale cache: {err}"
                    );
                    return Ok(stale);
                }
                return Err(err);
            }
        };

        let mut cached = self.inner.write().await;
        cached.loaded_at = Some(Instant::now());
        cached.suggestions = suggestions.clone();
        Ok(suggestions)
    }
}

impl CachedEditionSuggestions {
    fn is_fresh(&self, now: Instant, ttl: Duration) -> bool {
        self.loaded_at
            .map(|loaded_at| now.duration_since(loaded_at) < ttl)
            .unwrap_or(false)
    }
}

pub(crate) async fn fetch_edition_suggestion_map(
    pool: &PgPool,
) -> AppResult<BTreeMap<String, Vec<String>>> {
    let usage_counts: Vec<EditionUsageCount> = sqlx::query_as(EDITION_USAGE_COUNTS_SQL)
        .fetch_all(pool)
        .await?;

    Ok(build_edition_suggestion_map(&usage_counts))
}

pub(crate) fn build_edition_suggestion_map(
    usage_counts: &[EditionUsageCount],
) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<&str, Vec<&EditionUsageCount>> = BTreeMap::new();

    for usage in usage_counts {
        let edition = usage.edition.trim();
        if edition.is_empty() || usage.edition_count <= 0 {
            continue;
        }
        grouped
            .entry(usage.system_code.as_str())
            .or_default()
            .push(usage);
    }

    grouped
        .into_iter()
        .filter_map(|(system_code, mut usages)| {
            usages.sort_by(|a, b| {
                (!a.edition.trim().eq_ignore_ascii_case("Original"))
                    .cmp(&!b.edition.trim().eq_ignore_ascii_case("Original"))
                    .then_with(|| a.edition.to_lowercase().cmp(&b.edition.to_lowercase()))
                    .then_with(|| a.edition.cmp(&b.edition))
            });
            let suggestions: Vec<String> = usages
                .into_iter()
                .map(|usage| usage.edition.trim().to_string())
                .collect();

            if suggestions.is_empty() {
                None
            } else {
                Some((system_code.to_string(), suggestions))
            }
        })
        .collect()
}

pub fn can_view_disc_status(status: DiscStatus, can_view_disabled_discs: bool) -> bool {
    can_view_disabled_discs || status != DiscStatus::Disabled
}

pub fn ensure_disc_status_visible(
    status: DiscStatus,
    can_view_disabled_discs: bool,
) -> AppResult<()> {
    if can_view_disc_status(status, can_view_disabled_discs) {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

pub async fn get_disc_status(pool: &PgPool, disc_id: i32) -> AppResult<DiscStatus> {
    sqlx::query_scalar("SELECT status FROM discs WHERE id = $1")
        .bind(disc_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn ensure_disc_id_visible(
    pool: &PgPool,
    disc_id: i32,
    can_view_disabled_discs: bool,
) -> AppResult<()> {
    let status = get_disc_status(pool, disc_id).await?;
    ensure_disc_status_visible(status, can_view_disabled_discs)
}

fn parse_comma_separated(s: &str) -> Vec<String> {
    let mut out: Vec<String> = s
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect();
    out.sort_by_key(|v| v.to_lowercase());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

fn normalize_comma_separated(s: &str) -> String {
    parse_comma_separated(s).join(", ")
}

fn norm_cmp_str(s: &str) -> String {
    s.trim().to_lowercase()
}

fn json_field_norm(entry: &serde_json::Value, key: &str) -> String {
    entry
        .get(key)
        .and_then(|v| v.as_str())
        .map(norm_cmp_str)
        .unwrap_or_default()
}

fn json_layer_field_norm(layer: &serde_json::Value, key: &str) -> String {
    layer
        .get(key)
        .and_then(|v| v.as_str())
        .map(norm_cmp_str)
        .unwrap_or_default()
}

fn cmp_ring_entry_layers_json(
    a: &serde_json::Value,
    b: &serde_json::Value,
    max_layers: usize,
) -> Ordering {
    let a_layers = a["layers"].as_array().cloned().unwrap_or_default();
    let b_layers = b["layers"].as_array().cloned().unwrap_or_default();
    for idx in 0..max_layers {
        let a_layer = a_layers
            .get(idx)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let b_layer = b_layers
            .get(idx)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let by_mc = json_layer_field_norm(&a_layer, "mastering_code")
            .cmp(&json_layer_field_norm(&b_layer, "mastering_code"));
        if by_mc != Ordering::Equal {
            return by_mc;
        }

        let by_ms = json_layer_field_norm(&a_layer, "mastering_sid")
            .cmp(&json_layer_field_norm(&b_layer, "mastering_sid"));
        if by_ms != Ordering::Equal {
            return by_ms;
        }
    }

    let by_offset = json_field_norm(a, "offset_value").cmp(&json_field_norm(b, "offset_value"));
    if by_offset != Ordering::Equal {
        return by_offset;
    }
    let by_offset_extra =
        json_field_norm(a, "offset_extra_value").cmp(&json_field_norm(b, "offset_extra_value"));
    if by_offset_extra != Ordering::Equal {
        return by_offset_extra;
    }
    let by_sample = json_field_norm(a, "sample_start").cmp(&json_field_norm(b, "sample_start"));
    if by_sample != Ordering::Equal {
        return by_sample;
    }
    let by_comment = json_field_norm(a, "comment").cmp(&json_field_norm(b, "comment"));
    if by_comment != Ordering::Equal {
        return by_comment;
    }

    let a_id = a.get("id").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
    let b_id = b.get("id").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
    a_id.cmp(&b_id)
}

fn layer_field_norm(layer: Option<&DiscRingCodeLayer>, key: &str) -> String {
    let raw = match (layer, key) {
        (Some(l), "mastering_code") => l.mastering_code.as_deref().unwrap_or(""),
        (Some(l), "mastering_sid") => l.mastering_sid.as_deref().unwrap_or(""),
        _ => "",
    };
    norm_cmp_str(raw)
}

pub fn sort_ring_entry_views(entries: &mut [RingEntryView], max_layers: usize) {
    entries.sort_by(|a, b| {
        for idx in 0..max_layers {
            let a_layer = a.layers.iter().find(|l| l.layer == idx as i32);
            let b_layer = b.layers.iter().find(|l| l.layer == idx as i32);

            let by_mc = layer_field_norm(a_layer, "mastering_code")
                .cmp(&layer_field_norm(b_layer, "mastering_code"));
            if by_mc != Ordering::Equal {
                return by_mc;
            }

            let by_ms = layer_field_norm(a_layer, "mastering_sid")
                .cmp(&layer_field_norm(b_layer, "mastering_sid"));
            if by_ms != Ordering::Equal {
                return by_ms;
            }
        }

        let by_offset = a
            .offset_value
            .map(|v| v.to_string())
            .unwrap_or_default()
            .cmp(&b.offset_value.map(|v| v.to_string()).unwrap_or_default());
        if by_offset != Ordering::Equal {
            return by_offset;
        }
        let by_offset_extra = a
            .offset_extra_value
            .map(|v| v.to_string())
            .unwrap_or_default()
            .cmp(
                &b.offset_extra_value
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            );
        if by_offset_extra != Ordering::Equal {
            return by_offset_extra;
        }
        let by_sample = a
            .sample_data_start
            .map(|v| v.to_string())
            .unwrap_or_default()
            .cmp(
                &b.sample_data_start
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            );
        if by_sample != Ordering::Equal {
            return by_sample;
        }
        let by_comment = a
            .comment
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_lowercase()
            .cmp(&b.comment.as_deref().unwrap_or("").trim().to_lowercase());
        if by_comment != Ordering::Equal {
            return by_comment;
        }
        a.id.cmp(&b.id)
    });
}

pub fn sort_ring_codes_json(entries: &mut [serde_json::Value], max_layers: usize) {
    entries.sort_by(|a, b| cmp_ring_entry_layers_json(a, b, max_layers));
}

pub async fn get_all_systems(pool: &PgPool) -> AppResult<Vec<System>> {
    Ok(sqlx::query_as(
        "SELECT * FROM systems
         ORDER BY LOWER(manufacturer), manufacturer, LOWER(name), name",
    )
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
        "SELECT code, name, layer_count, pic, rom_extension FROM media_types WHERE code = $1",
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
         WHERE dr.disc_id = $1 ORDER BY r.sort_order",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let languages: Vec<Language> = sqlx::query_as(
        "SELECT l.* FROM languages l
         JOIN disc_languages dl ON dl.language_code = l.code
         WHERE dl.disc_id = $1 ORDER BY l.sort_order",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let ring_entries: Vec<DiscRingCodeEntry> =
        sqlx::query_as("SELECT * FROM disc_ring_code_entries WHERE disc_id = $1 ORDER BY id")
            .bind(disc_id)
            .fetch_all(pool)
            .await?;

    let mut ring_views = Vec::new();
    for entry in &ring_entries {
        let layers: Vec<DiscRingCodeLayer> = sqlx::query_as(
            "SELECT * FROM disc_ring_code_layers WHERE entry_id = $1 ORDER BY layer",
        )
        .bind(entry.id)
        .fetch_all(pool)
        .await?;
        ring_views.push(RingEntryView {
            id: entry.id,
            offset_value: entry.offset_value,
            offset_extra_value: entry.offset_extra_value,
            sample_data_start: entry.sample_data_start,
            comment: entry.comment.clone(),
            layers,
        });
    }

    let files: Vec<File> = sqlx::query_as(
        "SELECT * FROM files WHERE disc_id = $1 ORDER BY CAST(track_number AS INTEGER) NULLS LAST, track_number NULLS LAST"
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let dumpers: Vec<DumperInfo> = sqlx::query_as(
        "SELECT u.id AS user_id,
                u.username
         FROM disc_dumpers dd
         JOIN users u ON u.id = dd.user_id
         WHERE dd.disc_id = $1
         ORDER BY dd.position",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let disc_submission_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM disc_submissions
         WHERE target_disc_id = $1
           AND submission_type = 'Disc'",
    )
    .bind(disc_id)
    .fetch_one(pool)
    .await?;

    let added_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT MIN(created_at) FROM disc_submissions
         WHERE target_disc_id = $1",
    )
    .bind(disc_id)
    .fetch_one(pool)
    .await?;

    let modified_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM disc_submissions
         WHERE target_disc_id = $1",
    )
    .bind(disc_id)
    .fetch_one(pool)
    .await?;

    let sector_ranges: Vec<ProtectionRange> = sqlx::query_as(
        "SELECT lower(r)::INT AS range_start, upper(r)::INT AS range_end \
         FROM discs, unnest(sector_ranges) AS r WHERE id = $1 ORDER BY lower(r)",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    sort_ring_entry_views(&mut ring_views, disc.media_type.max_layers() as usize);

    Ok(DiscDetail {
        disc,
        system,
        regions,
        languages,
        ring_entries: ring_views,
        files,
        dumpers,
        disc_submission_count,
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
    let comments = data["comments"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty());
    let contents = data["contents"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty());
    let protection = data["protection"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty());
    let sbi = data["sbi"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty());
    let disc_id_text = data["disc_id"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let disc_key = parse_hex_text_bytes(data["disc_key"].as_str())?;
    let error_count = data["error_count"].as_i64().map(|v| v as i32);
    let exe_date = data["exe_date"].as_str().filter(|s| !s.is_empty());
    let edc = data["edc"].as_bool().unwrap_or(false);
    let pvd = data["pvd"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty())
        .map(|s| parse_pvd_hex_dump(&s).map_err(|e| AppError::BadRequest(format!("PVD: {e}"))))
        .transpose()?;
    let pic = data["pic"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty())
        .map(|s| parse_binary_hex_input(&s).map_err(|e| AppError::BadRequest(format!("PIC: {e}"))))
        .transpose()?;
    let bca = data["bca"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty())
        .map(|s| parse_binary_hex_input(&s).map_err(|e| AppError::BadRequest(format!("BCA: {e}"))))
        .transpose()?;
    let header = data["header"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty())
        .map(|s| {
            parse_binary_hex_input(&s).map_err(|e| AppError::BadRequest(format!("Header: {e}")))
        })
        .transpose()?;
    let cue = data["cuesheet"]
        .as_str()
        .map(normalize_newlines)
        .filter(|s| !s.is_empty());
    let layerbreaks: Option<Vec<i32>> = if let Some(arr) = data["layerbreaks"].as_array() {
        let v: Vec<i32> = arr
            .iter()
            .filter_map(|x| x.as_i64().map(|n| n as i32))
            .collect();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    } else {
        None
    };
    let status = match data["status"].as_str().unwrap_or("Unverified") {
        "Disabled" => "Disabled",
        "Questionable" => "Questionable",
        "Verified" => "Verified",
        _ => "Unverified",
    };

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
         protection = $23, sbi = $24, disc_id = $25, disc_key = $26,
         cue = $27,
         status = $28::disc_status_enum
         WHERE id = $29",
    )
    .bind(title) // $1
    .bind(system_code) // $2
    .bind(media_type) // $3
    .bind(category) // $4
    .bind(title_foreign) // $5
    .bind(disc_title) // $6
    .bind(disc_number) // $7
    .bind(filename_suffix) // $8
    .bind(&serial) // $9
    .bind(version) // $10
    .bind(&edition) // $11
    .bind(&barcode) // $12
    .bind(comments) // $13
    .bind(contents) // $14
    .bind(error_count) // $15
    .bind(exe_date) // $16
    .bind(edc) // $17
    .bind(&layerbreaks) // $18
    .bind(&pvd) // $19
    .bind(&pic) // $20
    .bind(&bca) // $21
    .bind(&header) // $22
    .bind(protection) // $23
    .bind(sbi) // $24
    .bind(disc_id_text) // $25
    .bind(&disc_key) // $26
    .bind(cue) // $27
    .bind(status) // $28
    .bind(disc_id) // $29
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
                     ON CONFLICT DO NOTHING",
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
                     ON CONFLICT DO NOTHING",
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
        let mut keep_entry_ids: Vec<i32> = Vec::new();

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

            let entry_id: i32 = if let Some(existing_id) =
                entry_data["id"].as_i64().map(|v| v as i32)
            {
                let updated = sqlx::query(
                    "UPDATE disc_ring_code_entries
                     SET offset_value = $1, offset_extra_value = $2, sample_data_start = $3, comment = $4
                     WHERE id = $5 AND disc_id = $6"
                )
                .bind(offset_value)
                .bind(offset_extra_value)
                .bind(sample_start)
                .bind(comment)
                .bind(existing_id)
                .bind(disc_id)
                .execute(pool)
                .await?;
                if updated.rows_affected() == 0 {
                    return Err(AppError::BadRequest(format!(
                        "ring code entry id {} does not belong to disc {}",
                        existing_id, disc_id
                    )));
                }
                existing_id
            } else {
                sqlx::query_scalar(
                    "INSERT INTO disc_ring_code_entries (disc_id, offset_value, offset_extra_value, sample_data_start, comment)
                     VALUES ($1, $2, $3, $4, $5) RETURNING id"
                )
                .bind(disc_id)
                .bind(offset_value)
                .bind(offset_extra_value)
                .bind(sample_start)
                .bind(comment)
                .fetch_one(pool)
                .await?
            };

            keep_entry_ids.push(entry_id);

            sqlx::query("DELETE FROM disc_ring_code_layers WHERE entry_id = $1")
                .bind(entry_id)
                .execute(pool)
                .await?;

            if let Some(layers) = entry_data["layers"].as_array() {
                for (li, layer_data) in layers.iter().enumerate() {
                    let mc = layer_data["mastering_code"]
                        .as_str()
                        .filter(|s| !s.is_empty());
                    let ms = layer_data["mastering_sid"]
                        .as_str()
                        .filter(|s| !s.is_empty());
                    let mould_sids =
                        normalize_comma_separated(layer_data["mould_sids"].as_str().unwrap_or(""));
                    let toolstamps =
                        normalize_comma_separated(layer_data["toolstamps"].as_str().unwrap_or(""));
                    let additional_moulds = normalize_comma_separated(
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

        if keep_entry_ids.is_empty() {
            sqlx::query("DELETE FROM disc_ring_code_entries WHERE disc_id = $1")
                .bind(disc_id)
                .execute(pool)
                .await?;
        } else {
            sqlx::query(
                "DELETE FROM disc_ring_code_entries
                 WHERE disc_id = $1 AND NOT (id = ANY($2::INT[]))",
            )
            .bind(disc_id)
            .bind(&keep_entry_ids)
            .execute(pool)
            .await?;
        }
    }

    // Files (non-cue) from XML
    if let Some(files_xml) = data["dat"].as_str().map(normalize_newlines) {
        sqlx::query("DELETE FROM files WHERE disc_id = $1 AND track_number IS NOT NULL")
            .bind(disc_id)
            .execute(pool)
            .await?;
        if !files_xml.is_empty() {
            parse_and_insert_files(pool, disc_id, &files_xml).await?;
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
         RETURNING id",
    )
    .bind(system_code)
    .bind(media_type)
    .bind(title)
    .bind(category)
    .fetch_one(pool)
    .await?;

    update_disc(pool, disc_id, data).await?;

    sqlx::query("INSERT INTO disc_dumpers (disc_id, user_id, position) VALUES ($1, $2, 0) ON CONFLICT DO NOTHING")
        .bind(disc_id)
        .bind(submitter_id)
        .execute(pool)
        .await?;

    Ok(disc_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(system_code: &str, edition: &str, edition_count: i64) -> EditionUsageCount {
        EditionUsageCount {
            system_code: system_code.to_string(),
            edition: edition.to_string(),
            edition_count,
        }
    }

    #[test]
    fn disabled_disc_status_requires_visibility_permission() {
        assert!(can_view_disc_status(DiscStatus::Verified, false));
        assert!(can_view_disc_status(DiscStatus::Questionable, false));
        assert!(can_view_disc_status(DiscStatus::Unverified, false));
        assert!(!can_view_disc_status(DiscStatus::Disabled, false));
        assert!(can_view_disc_status(DiscStatus::Disabled, true));
        assert!(ensure_disc_status_visible(DiscStatus::Disabled, false).is_err());
        assert!(ensure_disc_status_visible(DiscStatus::Disabled, true).is_ok());
    }

    #[test]
    fn binary_hex_parser_accepts_addressed_spaced_and_compact_hex() {
        let addressed =
            "0320 : 20 21 22 23 24 25 26 27  28 29 2A 2B 2C 2D 2E 2F    !\"#$%&'()*+,-./\n\
                         0330 : 30 31 32 33                                      0123";
        let spaced = "20 21 22 23 24 25 26 27  28 29 2A 2B 2C 2D 2E 2F\n30 31 32 33";
        let compact = "202122232425262728292A2B2C2D2E2F\n30313233";
        let expected: Vec<u8> = (0x20..=0x33).collect();

        assert_eq!(parse_binary_hex_input(addressed).unwrap(), expected);
        assert_eq!(parse_binary_hex_input(spaced).unwrap(), expected);
        assert_eq!(parse_binary_hex_input(compact).unwrap(), expected);
    }

    #[test]
    fn binary_hex_parser_rejects_invalid_raw_and_mixed_inputs() {
        assert!(parse_binary_hex_input("ABC").is_err());
        assert!(parse_binary_hex_input("01 02 XX").is_err());
        assert!(parse_binary_hex_input("0000 : 01 02\n03 04").is_err());
    }

    #[test]
    fn pvd_hex_parser_keeps_existing_stored_length_limit() {
        let bytes: Vec<u8> = (0u8..96).collect();
        let compact = bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join("");

        let parsed = parse_pvd_hex_dump(&compact).unwrap();

        assert_eq!(parsed.len(), 82);
        assert_eq!(parsed, bytes[..82]);
    }

    #[test]
    fn sort_ring_codes_json_uses_id_as_last_tiebreaker() {
        let mut entries = vec![
            serde_json::json!({
                "id": 9,
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "",
                "layers": [{"mastering_code": "AAA", "mastering_sid": "ZZZ"}]
            }),
            serde_json::json!({
                "id": 3,
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "",
                "layers": [{"mastering_code": "AAA", "mastering_sid": "ZZZ"}]
            }),
        ];

        sort_ring_codes_json(&mut entries, 1);
        assert_eq!(entries[0]["id"], 3);
        assert_eq!(entries[1]["id"], 9);
    }

    #[test]
    fn build_snapshot_from_disc_includes_label_side_layer() {
        let media_type: MediaType = MediaTypeRow {
            code: "dvd9".to_string(),
            name: "DVD-9".to_string(),
            layer_count: 2,
            pic: false,
            rom_extension: "iso".to_string(),
        }
        .into();
        let detail = DiscDetail {
            disc: Disc {
                id: 1,
                system_code: "SYS".to_string(),
                media_type,
                title: "Game".to_string(),
                title_foreign: None,
                disc_title: None,
                disc_number: None,
                serial: vec![],
                category: Category::Games,
                version: None,
                edition: vec![],
                barcode: vec![],
                comments: None,
                contents: None,
                filename_suffix: None,
                error_count: None,
                exe_date: None,
                edc: false,
                layerbreaks: None,
                protection: None,
                sbi: None,
                disc_id: None,
                disc_key: None,
                cue: None,
                pvd: None,
                pic: None,
                header: None,
                bca: None,
                status: DiscStatus::Verified,
            },
            system: System {
                code: "SYS".to_string(),
                system_type: "Console".to_string(),
                manufacturer: "Maker".to_string(),
                name: "System".to_string(),
                short_name: "SYS".to_string(),
                media_types: vec!["dvd9".to_string()],
                has_exe_date: false,
                has_sbi: false,
                has_pvd: false,
                has_edc: false,
                has_disc_id: false,
                has_key: false,
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
            },
            regions: vec![],
            languages: vec![],
            ring_entries: vec![RingEntryView {
                id: 1,
                offset_value: None,
                offset_extra_value: None,
                sample_data_start: None,
                comment: None,
                layers: vec![
                    DiscRingCodeLayer {
                        id: 1,
                        entry_id: 1,
                        layer: 0,
                        mastering_code: Some("L0-MC".to_string()),
                        mastering_sid: None,
                        mould_sids: String::new(),
                        toolstamps: String::new(),
                        additional_moulds: String::new(),
                    },
                    DiscRingCodeLayer {
                        id: 2,
                        entry_id: 1,
                        layer: 2,
                        mastering_code: Some("LS-MC".to_string()),
                        mastering_sid: None,
                        mould_sids: String::new(),
                        toolstamps: String::new(),
                        additional_moulds: String::new(),
                    },
                ],
            }],
            files: vec![],
            dumpers: vec![],
            disc_submission_count: 0,
            sector_ranges: vec![],
            added_at: None,
            modified_at: None,
        };

        let snapshot = build_snapshot_from_disc(&detail);
        let layers = snapshot["ring_codes"][0]["layers"].as_array().unwrap();

        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0]["mastering_code"], "L0-MC");
        assert_eq!(layers[1]["mastering_code"], "");
        assert_eq!(layers[2]["mastering_code"], "LS-MC");
    }

    #[test]
    fn newline_helpers_normalize_lf_and_crlf() {
        let mixed = "A\r\nB\rC\nD";
        assert_eq!(to_lf_newlines(mixed), "A\nB\nC\nD");
        assert_eq!(to_crlf_newlines(mixed), "A\r\nB\r\nC\r\nD");
    }

    #[test]
    fn cue_hashes_use_crlf_bytes() {
        let lf = "FILE \"Track 1.bin\" BINARY\n  TRACK 01 AUDIO";
        let crlf = to_crlf_newlines(lf);
        let (lf_size, _, _, _) = compute_file_hashes(lf.as_bytes());
        let (crlf_size, _, _, _) = compute_file_hashes(crlf.as_bytes());

        assert!(crlf.contains("\r\n"));
        assert_eq!(lf_size + 1, crlf_size);
    }

    #[test]
    fn edition_suggestion_query_excludes_disabled_discs() {
        assert!(EDITION_USAGE_COUNTS_SQL.contains("d.status <> 'Disabled'"));
    }

    #[test]
    fn edition_suggestions_trim_blank_and_sort_original_first_then_alphabetic() {
        let suggestions = build_edition_suggestion_map(&[
            usage("SYS", "Beta", 900),
            usage("SYS", "  Original  ", 1),
            usage("SYS", "alpha", 500),
            usage("SYS", "Rare", 4),
            usage("SYS", "   ", 100),
        ]);

        assert_eq!(
            suggestions["SYS"],
            vec![
                "Original".to_string(),
                "alpha".to_string(),
                "Beta".to_string(),
                "Rare".to_string()
            ]
        );
    }

    #[test]
    fn edition_suggestions_keep_full_alphabetic_list_for_selector() {
        let usages: Vec<EditionUsageCount> = (0..25)
            .rev()
            .map(|idx| usage("SYS", &format!("Edition {idx:02}"), 25 - idx))
            .collect();

        let suggestions = build_edition_suggestion_map(&usages);

        assert_eq!(suggestions["SYS"].len(), 25);
        assert_eq!(suggestions["SYS"].first().unwrap(), "Edition 00");
        assert_eq!(suggestions["SYS"].last().unwrap(), "Edition 24");
    }
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
         WHERE dr.disc_id = $1 ORDER BY r.sort_order",
    )
    .bind(disc_id)
    .fetch_all(pool)
    .await?;

    let language_codes: Vec<String> = sqlx::query_scalar(
        "SELECT l.code FROM languages l
         JOIN disc_languages dl ON dl.language_code = l.code
         WHERE dl.disc_id = $1 ORDER BY l.sort_order",
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
    let finalized_crlf = to_crlf_newlines(&finalized);

    sqlx::query("UPDATE discs SET cue = $1 WHERE id = $2")
        .bind(&finalized_crlf)
        .bind(disc_id)
        .execute(pool)
        .await?;

    let (size, crc32, md5, sha1) = compute_file_hashes(finalized_crlf.as_bytes());

    sqlx::query(
        "INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1)
         VALUES ($1, NULL, $2, $3, $4, $5)
         ON CONFLICT (disc_id) WHERE track_number IS NULL
         DO UPDATE SET size = $2, crc32 = $3, md5 = $4, sha1 = $5",
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
             SET size = $3, crc32 = $4, md5 = $5, sha1 = $6",
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
    extract_track_from_filename(filename)
}

fn format_hex_dump_snapshot(data: &[u8], base_addr: usize) -> String {
    let mut out = String::new();
    let total_chunks = data.chunks(16).len();
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = base_addr + i * 16;
        out.push_str(&format!("{:04X} : ", offset));
        for (j, byte) in chunk.iter().enumerate() {
            out.push_str(&format!("{:02X} ", byte));
            if j == 7 {
                out.push(' ');
            }
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
        }
        out.push_str("  ");
        for byte in chunk {
            if byte.is_ascii_graphic() || *byte == b' ' {
                out.push(*byte as char);
            } else {
                out.push(' ');
            }
        }
        if i + 1 < total_chunks {
            out.push('\n');
        }
    }
    out
}

fn format_pvd_hex_snapshot(data: &[u8]) -> String {
    const PVD_FULL_SIZE: usize = 96;
    const PVD_STORED_SIZE: usize = 82;
    let mut buf = [0u8; PVD_FULL_SIZE];
    let copy_len = data.len().min(PVD_STORED_SIZE);
    buf[..copy_len].copy_from_slice(&data[..copy_len]);
    format_hex_dump_snapshot(&buf, 0x0320)
}

/// Convert a DiscDetail into the flat JSON snapshot format that `update_disc` expects.
pub fn build_snapshot_from_disc(detail: &DiscDetail) -> serde_json::Value {
    let rom_extension = detail.disc.media_type.rom_extension();
    let max_layers = detail.disc.media_type.max_layers() + 1;
    let mut sorted_ring_entries = detail.ring_entries.clone();
    sort_ring_entry_views(&mut sorted_ring_entries, max_layers as usize);

    let ring_codes: Vec<serde_json::Value> = sorted_ring_entries.iter().map(|e| {
        let layers: Vec<serde_json::Value> = (0..max_layers).map(|li| {
            let layer = e.layers.iter().find(|l| l.layer == li as i32);
            serde_json::json!({
                "mastering_code": layer.and_then(|l| l.mastering_code.as_deref()).unwrap_or(""),
                "mastering_sid": layer.and_then(|l| l.mastering_sid.as_deref()).unwrap_or(""),
                "mould_sids": layer.map(|l| normalize_comma_separated(&l.mould_sids)).unwrap_or_default(),
                "toolstamps": layer.map(|l| normalize_comma_separated(&l.toolstamps)).unwrap_or_default(),
                "additional_moulds": layer.map(|l| normalize_comma_separated(&l.additional_moulds)).unwrap_or_default(),
            })
        }).collect();
        serde_json::json!({
            "id": e.id,
            "offset_value": e.offset_value.map(|v| v.to_string()).unwrap_or_default(),
            "offset_extra_value": e.offset_extra_value.map(|v| v.to_string()).unwrap_or_default(),
            "sample_start": e.sample_data_start.map(|v| v.to_string()).unwrap_or_default(),
            "comment": e.comment.clone().unwrap_or_default(),
            "layers": layers,
        })
    }).collect();

    let total_tracks = detail
        .files
        .iter()
        .filter(|f| f.track_number.is_some())
        .count();
    let files_xml: String = detail
        .files
        .iter()
        .filter(|f| f.track_number.is_some())
        .map(|f| {
            let name =
                build_simple_track_name(f.track_number.as_deref(), total_tracks, rom_extension);
            format!(
                r#"<rom name="{}" size="{}" crc="{}" md5="{}" sha1="{}" />"#,
                name, f.size, f.crc32, f.md5, f.sha1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let sector_ranges: Vec<serde_json::Value> = detail
        .sector_ranges
        .iter()
        .map(|r| serde_json::json!({"start": r.range_start, "end": r.range_end}))
        .collect();

    let region_codes: Vec<String> = detail
        .regions
        .iter()
        .map(|r| r.code.trim().to_string())
        .collect();
    let lang_codes: Vec<String> = detail
        .languages
        .iter()
        .map(|l| l.code.trim().to_string())
        .collect();

    let edc_value = detail.disc.edc;

    let cue = detail
        .disc
        .cue
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|c| simplify_cue(c, rom_extension));

    serde_json::json!({
        "system_code": detail.disc.system_code,
        "media_type": detail.disc.media_type.code(),
        "title": detail.disc.title,
        "category": detail.disc.category.to_string(),
        "title_foreign": detail.disc.title_foreign,
        "disc_number": detail.disc.disc_number,
        "disc_title": detail.disc.disc_title,
        "filename_suffix": detail.disc.filename_suffix,
        "serial": detail.disc.serial,
        "version": detail.disc.version,
        "edition": detail.disc.edition,
        "barcode": detail.disc.barcode,
        "comments": detail.disc.comments,
        "contents": detail.disc.contents,
        "error_count": detail.disc.error_count,
        "exe_date": detail.disc.exe_date,
        "edc": edc_value,
        "layerbreaks": detail.disc.layerbreaks.clone().unwrap_or_default(),
        "pvd": detail.disc.pvd.as_ref().map(|d| format_pvd_hex_snapshot(d)),
        "pic": detail.disc.pic.as_ref().map(|d| format_hex_dump_snapshot(d, 0x0000)),
        "bca": detail.disc.bca.as_ref().map(|d| format_hex_dump_snapshot(d, 0x0000)),
        "header": detail.disc.header.as_ref().map(|d| format_hex_dump_snapshot(d, 0x0000)),
        "protection": detail.disc.protection,
        "sbi": detail.disc.sbi,
        "disc_id": detail.disc.disc_id,
        "disc_key": detail.disc.disc_key.as_ref().map(|bytes| bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>()),
        "cuesheet": cue,
        "status": detail.disc.status.to_string(),
        "regions": region_codes,
        "languages": lang_codes,
        "ring_codes": ring_codes,
        "sector_ranges": sector_ranges,
        "dat": if files_xml.is_empty() { serde_json::Value::Null } else { serde_json::json!(files_xml) },
    })
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
