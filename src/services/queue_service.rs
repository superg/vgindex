use sqlx::PgPool;
use std::cmp::Ordering;
use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::{archive_service, disc_service};

const APPROVAL_CONFLICT_LOCK_KEY: i64 = 0x7667_696e_6465_7801;

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalConflict {
    pub text: String,
    pub disc_id: i32,
    pub disc_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved(i32),
    AlreadyProcessed,
    StaleDiscState,
    Conflicts(Vec<ApprovalConflict>),
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<serde_json::Value>>(),
        ),
        serde_json::Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));

            let mut sorted = serde_json::Map::new();
            for (key, item) in entries {
                sorted.insert(key.clone(), canonical_json(item));
            }
            serde_json::Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

pub fn disc_snapshot_hash(snapshot: &serde_json::Value) -> String {
    let canonical = canonical_json(snapshot);
    let bytes = serde_json::to_vec(&canonical)
        .expect("serializing a serde_json::Value snapshot should not fail");
    hex::encode(Sha256::digest(bytes))
}

pub async fn current_disc_snapshot_hash(pool: &PgPool, disc_id: i32) -> AppResult<String> {
    let detail = disc_service::get_disc_detail(pool, disc_id).await?;
    let snapshot = disc_service::build_snapshot_from_disc(&detail);
    Ok(disc_snapshot_hash(&snapshot))
}

pub fn review_base_hash_is_stale(expected_hash: Option<&str>, current_hash: &str) -> bool {
    expected_hash
        .map(str::trim)
        .is_some_and(|expected_hash| expected_hash != current_hash)
}

fn stale_review_approval_outcome(
    expected_hash: Option<&str>,
    current_hash: &str,
) -> Option<ApprovalOutcome> {
    if review_base_hash_is_stale(expected_hash, current_hash) {
        Some(ApprovalOutcome::StaleDiscState)
    } else {
        None
    }
}

pub async fn create_submission(
    pool: &PgPool,
    sub_type: SubmissionType,
    submitter_id: i32,
    target_disc_id: Option<i32>,
    changes: serde_json::Value,
    submission_comment: Option<&str>,
    dump_log: Option<&str>,
    extra_upload_url: Option<&str>,
) -> AppResult<DiscSubmission> {
    let normalized_submission_comment: Option<String> = submission_comment
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let sub: DiscSubmission = sqlx::query_as(
        "INSERT INTO disc_submissions (submission_type, submitter_id, submission_comment, target_disc_id, changes, dump_log, extra_upload_url)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *"
    )
    .bind(sub_type)
    .bind(submitter_id)
    .bind(normalized_submission_comment.as_deref())
    .bind(target_disc_id)
    .bind(&changes)
    .bind(dump_log)
    .bind(extra_upload_url)
    .fetch_one(pool)
    .await?;

    Ok(sub)
}

struct RomEntry {
    track_number: Option<String>,
    size: i64,
    crc32: String,
    md5: String,
    sha1: String,
}

fn parse_rom_entries(files_xml: &str) -> Vec<RomEntry> {
    files_xml
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("<rom ") {
                return None;
            }
            let name = extract_xml_attr(line, "name").unwrap_or_default();
            Some(RomEntry {
                track_number: extract_track_from_filename(&name),
                size: extract_xml_attr(line, "size")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                crc32: normalize_hash_attr(extract_xml_attr(line, "crc")),
                md5: normalize_hash_attr(extract_xml_attr(line, "md5")),
                sha1: normalize_hash_attr(extract_xml_attr(line, "sha1")),
            })
        })
        .collect()
}

pub async fn find_matching_disc(
    pool: &PgPool,
    files_xml: &str,
    include_disabled_discs: bool,
) -> AppResult<Option<i32>> {
    find_matching_disc_excluding(pool, files_xml, include_disabled_discs, None).await
}

async fn find_matching_disc_excluding(
    pool: &PgPool,
    files_xml: &str,
    include_disabled_discs: bool,
    exclude_disc_id: Option<i32>,
) -> AppResult<Option<i32>> {
    let submitted = parse_rom_entries(files_xml);
    if submitted.is_empty() {
        return Ok(None);
    }

    let candidate_sql = if include_disabled_discs {
        "SELECT DISTINCT disc_id
         FROM files
         WHERE LOWER(sha1) = LOWER($1)
           AND ($2::INT IS NULL OR disc_id <> $2)"
    } else {
        "SELECT DISTINCT f.disc_id
         FROM files f
         JOIN discs d ON d.id = f.disc_id
         WHERE LOWER(f.sha1) = LOWER($1)
           AND d.status <> 'Disabled'
           AND ($2::INT IS NULL OR f.disc_id <> $2)"
    };
    let candidates: Vec<i32> = sqlx::query_scalar(candidate_sql)
        .bind(&submitted[0].sha1)
        .bind(exclude_disc_id)
        .fetch_all(pool)
        .await?;

    for disc_id in candidates {
        let disc_files: Vec<crate::db::models::File> = sqlx::query_as(
            "SELECT * FROM files WHERE disc_id = $1 AND track_number IS NOT NULL ORDER BY track_number",
        )
        .bind(disc_id)
        .fetch_all(pool)
        .await?;

        if files_match_submission(&disc_files, &submitted) {
            return Ok(Some(disc_id));
        }
    }

    Ok(None)
}

pub(crate) fn universal_hash_bytes_for_matching(universal_hash: Option<&str>) -> Option<Vec<u8>> {
    let text = universal_hash?.trim();
    if text.len() != 40 || !text.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    hex::decode(text).ok()
}

pub async fn find_matching_disc_by_universal_hash(
    pool: &PgPool,
    universal_hash: &str,
    include_disabled_discs: bool,
) -> AppResult<Option<i32>> {
    find_matching_disc_by_universal_hash_excluding(
        pool,
        universal_hash,
        include_disabled_discs,
        None,
    )
    .await
}

async fn find_matching_disc_by_universal_hash_excluding(
    pool: &PgPool,
    universal_hash: &str,
    include_disabled_discs: bool,
    exclude_disc_id: Option<i32>,
) -> AppResult<Option<i32>> {
    let Some(hash_bytes) = universal_hash_bytes_for_matching(Some(universal_hash)) else {
        return Ok(None);
    };
    let sql = if include_disabled_discs {
        "SELECT id
         FROM discs
         WHERE universal_hash = $1
           AND ($2::INT IS NULL OR id <> $2)
         ORDER BY id
         LIMIT 1"
    } else {
        "SELECT id
         FROM discs
         WHERE universal_hash = $1
           AND status <> 'Disabled'
           AND ($2::INT IS NULL OR id <> $2)
         ORDER BY id
         LIMIT 1"
    };

    Ok(sqlx::query_scalar(sql)
        .bind(hash_bytes)
        .bind(exclude_disc_id)
        .fetch_optional(pool)
        .await?)
}

fn normalize_hash_attr(value: Option<String>) -> String {
    value
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default()
}

fn track_number_value(track_number: Option<&str>) -> Option<i32> {
    track_number?.trim().parse().ok()
}

fn submitted_by_track(submitted: &[RomEntry]) -> Option<BTreeMap<i32, &RomEntry>> {
    let mut tracks = BTreeMap::new();
    for entry in submitted {
        let track = track_number_value(entry.track_number.as_deref())?;
        if tracks.insert(track, entry).is_some() {
            return None;
        }
    }
    Some(tracks)
}

fn files_by_track(
    files: &[crate::db::models::File],
) -> Option<BTreeMap<i32, &crate::db::models::File>> {
    let mut tracks = BTreeMap::new();
    for file in files {
        let track = track_number_value(file.track_number.as_deref())?;
        if tracks.insert(track, file).is_some() {
            return None;
        }
    }
    Some(tracks)
}

fn hashes_match(file: &crate::db::models::File, submitted: &RomEntry) -> bool {
    file.size == submitted.size
        && file.crc32.eq_ignore_ascii_case(&submitted.crc32)
        && file.md5.eq_ignore_ascii_case(&submitted.md5)
        && file.sha1.eq_ignore_ascii_case(&submitted.sha1)
}

fn compare_track_numbers(a: Option<&str>, b: Option<&str>) -> Ordering {
    track_number_value(a)
        .cmp(&track_number_value(b))
        .then_with(|| a.unwrap_or_default().cmp(b.unwrap_or_default()))
}

fn files_match_submission(files: &[crate::db::models::File], submitted: &[RomEntry]) -> bool {
    if files.len() != submitted.len() {
        return false;
    }

    if let (Some(submitted_tracks), Some(file_tracks)) =
        (submitted_by_track(submitted), files_by_track(files))
    {
        return submitted_tracks.iter().all(|(track, submitted_file)| {
            file_tracks
                .get(track)
                .map(|disc_file| hashes_match(disc_file, submitted_file))
                .unwrap_or(false)
        });
    }

    let mut sorted_files: Vec<&crate::db::models::File> = files.iter().collect();
    sorted_files.sort_by(|a, b| {
        compare_track_numbers(a.track_number.as_deref(), b.track_number.as_deref())
    });

    let mut sorted_submitted: Vec<&RomEntry> = submitted.iter().collect();
    sorted_submitted.sort_by(|a, b| {
        compare_track_numbers(a.track_number.as_deref(), b.track_number.as_deref())
    });

    sorted_files
        .iter()
        .zip(sorted_submitted)
        .all(|(disc_file, submitted_file)| hashes_match(disc_file, submitted_file))
}

fn extract_xml_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn json_str_vec(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn scalar_operation_new_value(node: &serde_json::Value) -> AppResult<Option<&serde_json::Value>> {
    let Some(obj) = node.as_object() else {
        return Err(AppError::BadRequest(
            "scalar change must be an object".to_string(),
        ));
    };

    if obj
        .keys()
        .any(|key| !matches!(key.as_str(), "add" | "modify" | "remove"))
    {
        return Err(AppError::BadRequest(
            "scalar change contains unknown operation".to_string(),
        ));
    }

    let operation_count = ["add", "modify", "remove"]
        .iter()
        .filter(|op| obj.contains_key(**op))
        .count();
    if operation_count != 1 {
        return Err(AppError::BadRequest(
            "scalar change must contain exactly one operation".to_string(),
        ));
    }

    if let Some(add) = obj.get("add") {
        let Some(add_obj) = add.as_object() else {
            return Err(AppError::BadRequest(
                "scalar add operation must be an object".to_string(),
            ));
        };
        if add_obj.len() != 1 || !add_obj.contains_key("new") {
            return Err(AppError::BadRequest(
                "scalar add operation requires only new value".to_string(),
            ));
        }
        return add.get("new").map(Some).ok_or_else(|| {
            AppError::BadRequest("scalar add operation requires new value".to_string())
        });
    }
    if let Some(modify) = obj.get("modify") {
        let Some(modify_obj) = modify.as_object() else {
            return Err(AppError::BadRequest(
                "scalar modify operation must be an object".to_string(),
            ));
        };
        if modify_obj.len() != 2
            || !modify_obj.contains_key("old")
            || !modify_obj.contains_key("new")
        {
            return Err(AppError::BadRequest(
                "scalar modify operation requires old and new values".to_string(),
            ));
        }
        return Ok(Some(&modify["new"]));
    }
    if let Some(remove) = obj.get("remove") {
        let Some(remove_obj) = remove.as_object() else {
            return Err(AppError::BadRequest(
                "scalar remove operation must be an object".to_string(),
            ));
        };
        if remove_obj.len() != 1 || !remove_obj.contains_key("old") {
            return Err(AppError::BadRequest(
                "scalar remove operation requires only old value".to_string(),
            ));
        }
        return Ok(None);
    }

    unreachable!("operation_count already checked")
}

fn is_nullable_scalar_field(key: &str) -> bool {
    matches!(
        key,
        "title_foreign"
            | "disc_number"
            | "disc_title"
            | "filename_suffix"
            | "version"
            | "error_count"
            | "exe_date"
            | "comments"
            | "contents"
            | "protection"
            | "sector_ranges"
            | "sbi"
            | "disc_id"
            | "disc_key"
            | "universal_hash"
            | "pvd"
            | "header"
            | "bca"
            | "pic"
            | "cuesheet"
            | "dat"
            | "layerbreaks"
    )
}

fn apply_scalar_operation(
    key: &str,
    change_node: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    match scalar_operation_new_value(change_node)? {
        Some(new_value) => Ok(new_value.clone()),
        None if is_nullable_scalar_field(key) => Ok(serde_json::Value::Null),
        None => Err(AppError::BadRequest(format!("{key} cannot be removed"))),
    }
}

fn parse_csv_ids(value: &str) -> Vec<String> {
    let mut out: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    out.sort_by_key(|s| s.to_lowercase());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

fn normalize_csv_ids(value: &str) -> String {
    parse_csv_ids(value).join(", ")
}

fn append_unique_case_insensitive(
    values: &mut Vec<String>,
    incoming: impl IntoIterator<Item = String>,
) {
    for value in incoming {
        if !values
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&value))
        {
            values.push(value);
        }
    }
}

fn append_unique(values: &mut Vec<String>, incoming: impl IntoIterator<Item = String>) {
    for value in incoming {
        if !value.trim().is_empty() && !values.iter().any(|existing| existing == &value) {
            values.push(value);
        }
    }
}

fn apply_set_operation(
    old_values: &[String],
    change_node: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    let Some(obj) = change_node.as_object() else {
        return Err(AppError::BadRequest(
            "set change must be an object".to_string(),
        ));
    };

    if obj.is_empty()
        || obj
            .keys()
            .any(|key| !matches!(key.as_str(), "add" | "remove"))
    {
        return Err(AppError::BadRequest(
            "set change must contain add and/or remove arrays".to_string(),
        ));
    }

    let read_set_values = |key: &str| -> AppResult<Vec<String>> {
        let Some(value) = obj.get(key) else {
            return Ok(Vec::new());
        };
        let Some(arr) = value.as_array() else {
            return Err(AppError::BadRequest(format!(
                "set {key} operation must be an array"
            )));
        };
        arr.iter()
            .map(|v| {
                v.as_str().map(|s| s.to_string()).ok_or_else(|| {
                    AppError::BadRequest(format!("set {key} operation values must be strings"))
                })
            })
            .collect()
    };

    let remove_values = read_set_values("remove")?;
    let add_values = read_set_values("add")?;

    let mut out: Vec<String> = old_values
        .iter()
        .filter(|value| !remove_values.iter().any(|remove| remove == *value))
        .cloned()
        .collect();
    append_unique(&mut out, add_values);
    Ok(serde_json::json!(out))
}

fn apply_ring_scalar_field(
    entry: &mut serde_json::Value,
    key: &str,
    change_node: &serde_json::Value,
) -> AppResult<()> {
    let value = match scalar_operation_new_value(change_node)? {
        Some(serde_json::Value::Null) | None => serde_json::json!(""),
        Some(serde_json::Value::String(s)) => serde_json::json!(s),
        Some(serde_json::Value::Number(n)) => serde_json::json!(n.to_string()),
        Some(serde_json::Value::Bool(b)) => serde_json::json!(b.to_string()),
        Some(value) => serde_json::json!(value.to_string()),
    };
    entry[key] = value;
    Ok(())
}

fn operation_new_str(change: &serde_json::Value, key: &str) -> String {
    change
        .get(key)
        .and_then(|node| scalar_operation_new_value(node).ok().flatten())
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn ring_get_or_create_layer(
    entry: &mut serde_json::Value,
    layer_index: usize,
) -> &mut serde_json::Value {
    if !entry["layers"].is_array() {
        entry["layers"] = serde_json::json!([]);
    }
    while entry["layers"].as_array().map(|a| a.len()).unwrap_or(0) <= layer_index {
        let arr = entry["layers"].as_array_mut().expect("layers array exists");
        arr.push(serde_json::json!({
            "mastering_code": "",
            "mastering_sid": "",
            "toolstamps": "",
            "mould_sids": "",
            "additional_moulds": ""
        }));
    }
    &mut entry["layers"][layer_index]
}

fn ring_layers_max(entries: &[serde_json::Value]) -> usize {
    entries
        .iter()
        .map(|e| e["layers"].as_array().map(|a| a.len()).unwrap_or(0))
        .max()
        .unwrap_or(0)
}

fn merge_csv_values(existing: &str, incoming: &str) -> String {
    let mut combined: Vec<String> = parse_csv_ids(existing);
    for val in parse_csv_ids(incoming) {
        if !combined.iter().any(|v| v.eq_ignore_ascii_case(&val)) {
            combined.push(val);
        }
    }
    combined.sort_by_key(|s| s.to_lowercase());
    combined.join(", ")
}

fn offsets_match(existing_val: &str, change_val: &str) -> bool {
    existing_val.is_empty() || change_val.is_empty() || existing_val == change_val
}

fn find_matching_ring_entry(
    rings: &[serde_json::Value],
    change: &serde_json::Value,
    removed_ring_ids: &std::collections::HashSet<i32>,
) -> Option<usize> {
    let change_layers = change.get("layers").and_then(|v| v.as_array())?;

    let change_offset = operation_new_str(change, "offset_value");
    let change_offset_extra = operation_new_str(change, "offset_extra_value");

    'outer: for (ring_idx, ring) in rings.iter().enumerate() {
        if ring
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|v| removed_ring_ids.contains(&(v as i32)))
            .unwrap_or(false)
        {
            continue;
        }

        let ring_offset = ring["offset_value"].as_str().unwrap_or("");
        let ring_offset_extra = ring["offset_extra_value"].as_str().unwrap_or("");

        if !offsets_match(ring_offset, &change_offset)
            || !offsets_match(ring_offset_extra, &change_offset_extra)
        {
            continue;
        }

        let ring_layers = ring["layers"].as_array();
        for cl in change_layers {
            let Some(layer_idx) = cl.get("index").and_then(|v| v.as_u64()) else {
                continue;
            };
            let layer_idx = layer_idx as usize;

            for field in ["mastering_code", "mastering_sid"] {
                let change_val = operation_new_str(cl, field);

                let ring_val = ring_layers
                    .and_then(|layers| layers.get(layer_idx))
                    .and_then(|l| l[field].as_str())
                    .unwrap_or("");

                if change_val != ring_val {
                    continue 'outer;
                }
            }
        }

        return Some(ring_idx);
    }
    None
}

fn apply_ring_codes_history(
    old_value: &serde_json::Value,
    change_node: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    let mut rings = old_value.as_array().cloned().unwrap_or_default();
    let before_layers = ring_layers_max(&rings);
    disc_service::sort_ring_codes_json(&mut rings, before_layers);
    let Some(changes) = change_node.as_array() else {
        return Ok(serde_json::json!(rings));
    };
    let removed_ring_ids: std::collections::HashSet<i32> = changes
        .iter()
        .filter(|change| {
            change
                .get("remove")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .filter_map(|change| change.get("id").and_then(|v| v.as_i64()).map(|v| v as i32))
        .collect();
    let mut removals: Vec<usize> = Vec::new();
    let mut additions: Vec<serde_json::Value> = Vec::new();
    let ring_index_by_id = |rings: &[serde_json::Value], id: i32| -> Option<usize> {
        rings.iter().position(|entry| {
            entry.get("id").and_then(|v| v.as_i64()).map(|v| v as i32) == Some(id)
        })
    };

    for change in changes {
        let is_removed = change
            .get("remove")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let id = change.get("id").and_then(|v| v.as_i64()).map(|v| v as i32);
        let resolved_idx = if let Some(id) = id {
            ring_index_by_id(&rings, id)
                .ok_or_else(|| AppError::BadRequest(format!("ring_codes id {} not found", id)))?
        } else {
            usize::MAX
        };
        if is_removed {
            if resolved_idx == usize::MAX {
                return Err(AppError::BadRequest(
                    "ring_codes removal requires entry id".to_string(),
                ));
            }
            if resolved_idx >= rings.len() {
                return Err(AppError::BadRequest(format!(
                    "ring_codes index {} out of range (len {})",
                    resolved_idx,
                    rings.len()
                )));
            }
            removals.push(resolved_idx);
            continue;
        }

        let merge_idx = if resolved_idx == usize::MAX {
            find_matching_ring_entry(&rings, change, &removed_ring_ids)
        } else {
            None
        };
        let is_merge = merge_idx.is_some();

        let entry = if resolved_idx != usize::MAX {
            if resolved_idx >= rings.len() {
                return Err(AppError::BadRequest(format!(
                    "ring_codes index {} out of range (len {})",
                    resolved_idx,
                    rings.len()
                )));
            }
            &mut rings[resolved_idx]
        } else if let Some(mi) = merge_idx {
            &mut rings[mi]
        } else {
            additions.push(serde_json::json!({
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "",
                "layers": []
            }));
            additions.last_mut().expect("addition just pushed")
        };

        for (history_key, target_key) in [
            ("offset_value", "offset_value"),
            ("offset_extra_value", "offset_extra_value"),
            ("sample_data_start", "sample_start"),
            ("comment", "comment"),
        ] {
            if let Some(node) = change.get(history_key) {
                apply_ring_scalar_field(entry, target_key, node)?;
            }
        }

        if let Some(layer_changes) = change.get("layers").and_then(|v| v.as_array()) {
            for layer_change in layer_changes {
                let Some(layer_idx) = layer_change.get("index").and_then(|v| v.as_u64()) else {
                    continue;
                };
                let layer = ring_get_or_create_layer(entry, layer_idx as usize);
                for field in ["mastering_code", "mastering_sid"] {
                    if let Some(node) = layer_change.get(field) {
                        apply_ring_scalar_field(layer, field, node)?;
                    }
                }
                for field in ["toolstamps", "mould_sids", "additional_moulds"] {
                    if let Some(node) = layer_change.get(field) {
                        let new_csv = scalar_operation_new_value(node)?
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if is_merge {
                            let existing = layer[field].as_str().unwrap_or("");
                            layer[field] = serde_json::json!(merge_csv_values(existing, new_csv));
                        } else {
                            layer[field] = serde_json::json!(normalize_csv_ids(new_csv));
                        }
                    }
                }
            }
        }
    }

    removals.sort_unstable();
    removals.dedup();
    for idx in removals.into_iter().rev() {
        rings.remove(idx);
    }
    rings.extend(additions);
    let after_layers = ring_layers_max(&rings);
    disc_service::sort_ring_codes_json(&mut rings, after_layers);

    Ok(serde_json::json!(rings))
}

pub fn resolve_submission_snapshot(
    db_snapshot: &serde_json::Value,
    changes: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    let mut resolved = db_snapshot.clone();
    let Some(change_obj) = changes.as_object() else {
        return Ok(resolved);
    };
    let Some(resolved_obj) = resolved.as_object_mut() else {
        return Ok(resolved);
    };

    for (key, value) in change_obj {
        match key.as_str() {
            "regions" | "languages" | "serial" | "edition" | "barcode" => {
                let old = json_str_vec(resolved_obj.get(key).unwrap_or(&serde_json::Value::Null));
                let updated = apply_set_operation(&old, value)?;
                resolved_obj.insert(key.clone(), updated);
            }
            "ring_codes" => {
                let old = resolved_obj
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]));
                let updated = apply_ring_codes_history(&old, value)?;
                resolved_obj.insert(key.clone(), updated);
            }
            _ => {
                let new_value = apply_scalar_operation(key, value)?;
                resolved_obj.insert(key.clone(), new_value);
            }
        }
    }

    Ok(resolved)
}

pub fn resolve_submission_snapshot_for_submission(
    db_snapshot: &serde_json::Value,
    sub: &DiscSubmission,
) -> AppResult<serde_json::Value> {
    resolve_submission_snapshot(db_snapshot, &sub.changes)
}

async fn resolve_submission_data(
    pool: &PgPool,
    sub: &DiscSubmission,
    changes: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    resolve_submission_data_for_target(pool, sub.target_disc_id, changes).await
}

async fn resolve_submission_data_for_target(
    pool: &PgPool,
    target_disc_id: Option<i32>,
    changes: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    let db_snapshot = if let Some(disc_id) = target_disc_id {
        let detail = disc_service::get_disc_detail(pool, disc_id).await?;
        disc_service::build_snapshot_from_disc(&detail)
    } else {
        serde_json::json!({})
    };
    resolve_submission_snapshot(&db_snapshot, changes)
}

fn apply_approval_status(
    data: &mut serde_json::Value,
    submission_type: SubmissionType,
    target_disc_id: Option<i32>,
) {
    let Some(obj) = data.as_object_mut() else {
        return;
    };

    if target_disc_id.is_none() {
        obj.insert("status".to_string(), serde_json::json!("Unverified"));
    } else if submission_type == SubmissionType::Disc {
        obj.insert("status".to_string(), serde_json::json!("Verified"));
    }
}

async fn approval_effective_data(
    pool: &PgPool,
    submission_type: SubmissionType,
    target_disc_id: Option<i32>,
    changes: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    let mut effective_data =
        resolve_submission_data_for_target(pool, target_disc_id, changes).await?;
    apply_approval_status(&mut effective_data, submission_type, target_disc_id);
    Ok(effective_data)
}

pub async fn find_approval_conflicts(
    pool: &PgPool,
    submission_type: SubmissionType,
    target_disc_id: Option<i32>,
    changes: &serde_json::Value,
) -> AppResult<Vec<ApprovalConflict>> {
    let effective_data =
        approval_effective_data(pool, submission_type, target_disc_id, changes).await?;
    find_approval_conflicts_for_effective_data(pool, target_disc_id, changes, &effective_data).await
}

async fn find_approval_conflicts_for_effective_data(
    pool: &PgPool,
    target_disc_id: Option<i32>,
    changes: &serde_json::Value,
    effective_data: &serde_json::Value,
) -> AppResult<Vec<ApprovalConflict>> {
    if !effective_disc_is_active(effective_data) {
        return Ok(Vec::new());
    }

    let mut conflicts = Vec::new();

    if let Some(conflict) =
        find_generated_name_conflict(pool, effective_data, target_disc_id).await?
    {
        conflicts.push(conflict);
    }

    if let Some(files_xml) = dat_hash_conflict_input(changes, effective_data) {
        if let Some(disc_id) =
            find_matching_disc_excluding(pool, files_xml, false, target_disc_id).await?
        {
            conflicts.push(ApprovalConflict {
                text: "DAT hashes already exist:".to_string(),
                disc_id,
                disc_title: fetch_approval_conflict_disc_title(pool, disc_id).await?,
            });
        }
    }

    if let Some(universal_hash) = universal_hash_conflict_input(changes, effective_data) {
        if let Some(disc_id) = find_matching_disc_by_universal_hash_excluding(
            pool,
            universal_hash,
            false,
            target_disc_id,
        )
        .await?
        {
            conflicts.push(ApprovalConflict {
                text: "Universal hash already exists:".to_string(),
                disc_id,
                disc_title: fetch_approval_conflict_disc_title(pool, disc_id).await?,
            });
        }
    }

    Ok(conflicts)
}

fn effective_disc_is_active(effective_data: &serde_json::Value) -> bool {
    effective_data["status"].as_str() != Some("Disabled")
}

fn change_set_contains(changes: &serde_json::Value, key: &str) -> bool {
    changes.as_object().is_some_and(|obj| obj.contains_key(key))
}

fn dat_hash_conflict_input<'a>(
    changes: &serde_json::Value,
    effective_data: &'a serde_json::Value,
) -> Option<&'a str> {
    if !change_set_contains(changes, "dat") {
        return None;
    }
    effective_data["dat"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn universal_hash_conflict_input<'a>(
    changes: &serde_json::Value,
    effective_data: &'a serde_json::Value,
) -> Option<&'a str> {
    if !change_set_contains(changes, "universal_hash") {
        return None;
    }
    effective_data["universal_hash"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn json_nonempty_strings(value: &serde_json::Value) -> Vec<String> {
    json_str_vec(value)
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

async fn selected_region_names(pool: &PgPool, region_codes: &[String]) -> AppResult<Vec<String>> {
    if region_codes.is_empty() {
        return Ok(Vec::new());
    }

    Ok(sqlx::query_scalar::<_, String>(
        "SELECT name FROM regions WHERE code = ANY($1) ORDER BY sort_order",
    )
    .bind(region_codes)
    .fetch_all(pool)
    .await?)
}

async fn selected_language_codes(
    pool: &PgPool,
    language_codes: &[String],
) -> AppResult<Vec<String>> {
    if language_codes.is_empty() {
        return Ok(Vec::new());
    }

    Ok(sqlx::query_scalar::<_, String>(
        "SELECT code FROM languages WHERE code = ANY($1) ORDER BY sort_order",
    )
    .bind(language_codes)
    .fetch_all(pool)
    .await?)
}

fn generated_name_key(name: &str) -> String {
    name.to_lowercase()
}

fn generated_disc_name(
    title: &str,
    region_names: &[String],
    language_codes: &[String],
    disc_number: Option<&str>,
    disc_title: Option<&str>,
    filename_suffix: Option<&str>,
) -> String {
    build_rom_base_name(
        title.trim(),
        region_names,
        language_codes,
        disc_number.map(str::trim).filter(|s| !s.is_empty()),
        disc_title.map(str::trim).filter(|s| !s.is_empty()),
        filename_suffix.map(str::trim).filter(|s| !s.is_empty()),
    )
}

async fn find_generated_name_conflict(
    pool: &PgPool,
    effective_data: &serde_json::Value,
    exclude_disc_id: Option<i32>,
) -> AppResult<Option<ApprovalConflict>> {
    #[derive(sqlx::FromRow)]
    struct DuplicateNameDiscRow {
        id: i32,
        title: String,
        disc_number: Option<String>,
        disc_title: Option<String>,
        filename_suffix: Option<String>,
        region_names: Vec<String>,
        language_codes: Vec<String>,
    }

    let system_code = effective_data["system_code"].as_str().unwrap_or("").trim();
    let title = effective_data["title"].as_str().unwrap_or("").trim();
    if system_code.is_empty() || title.is_empty() {
        return Ok(None);
    }

    let regions = json_nonempty_strings(&effective_data["regions"]);
    let languages = json_nonempty_strings(&effective_data["languages"]);
    let region_names = selected_region_names(pool, &regions).await?;
    let language_codes = selected_language_codes(pool, &languages).await?;
    let proposed_name = generated_disc_name(
        title,
        &region_names,
        &language_codes,
        effective_data["disc_number"].as_str(),
        effective_data["disc_title"].as_str(),
        effective_data["filename_suffix"].as_str(),
    );
    let proposed_key = generated_name_key(&proposed_name);

    let candidates: Vec<DuplicateNameDiscRow> = sqlx::query_as(
        "SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix,
                COALESCE((
                    SELECT array_agg(r.name ORDER BY r.sort_order)
                    FROM disc_regions dr
                    JOIN regions r ON r.code = dr.region_code
                    WHERE dr.disc_id = d.id
                ), ARRAY[]::TEXT[]) AS region_names,
                COALESCE((
                    SELECT array_agg(l.code ORDER BY l.sort_order)
                    FROM disc_languages dl
                    JOIN languages l ON l.code = dl.language_code
                    WHERE dl.disc_id = d.id
                ), ARRAY[]::TEXT[]) AS language_codes
         FROM discs d
         WHERE d.system_code = $1
           AND d.status <> 'Disabled'
           AND ($2::INT IS NULL OR d.id <> $2)
         ORDER BY d.id",
    )
    .bind(system_code)
    .bind(exclude_disc_id)
    .fetch_all(pool)
    .await?;

    for candidate in candidates {
        let candidate_name = generated_disc_name(
            &candidate.title,
            &candidate.region_names,
            &candidate.language_codes,
            candidate.disc_number.as_deref(),
            candidate.disc_title.as_deref(),
            candidate.filename_suffix.as_deref(),
        );
        if generated_name_key(&candidate_name) == proposed_key {
            return Ok(Some(ApprovalConflict {
                text: "Generated name already exists:".to_string(),
                disc_id: candidate.id,
                disc_title: candidate_name,
            }));
        }
    }

    Ok(None)
}

async fn fetch_approval_conflict_disc_title(pool: &PgPool, disc_id: i32) -> AppResult<String> {
    #[derive(sqlx::FromRow)]
    struct Row {
        title: String,
        disc_number: Option<String>,
        disc_title: Option<String>,
        filename_suffix: Option<String>,
        has_disc_number: bool,
        has_disc_title: bool,
    }

    let Some(row) = sqlx::query_as::<_, Row>(
        "SELECT d.title, d.disc_number, d.disc_title, d.filename_suffix,
                s.has_disc_number, s.has_disc_title
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         WHERE d.id = $1",
    )
    .bind(disc_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(format!("disc #{}", disc_id));
    };

    Ok(format_display_title(
        &row.title,
        if row.has_disc_number {
            row.disc_number.as_deref()
        } else {
            None
        },
        if row.has_disc_title {
            row.disc_title.as_deref()
        } else {
            None
        },
        row.filename_suffix.as_deref(),
    ))
}

/// Atomically reject a submission.  Returns `true` if the rejection was
/// applied, `false` if the submission was already processed by another
/// moderator (race condition).
pub async fn reject_submission(
    pool: &PgPool,
    id: i32,
    reviewer_id: i32,
    review_comment: Option<&str>,
) -> AppResult<bool> {
    let normalized_review_comment: Option<String> = review_comment
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let result = sqlx::query(
        "UPDATE disc_submissions SET status = 'Rejected', reviewer_id = $1,
         review_comment = $2, reviewed_at = NOW()
         WHERE id = $3 AND status = 'Pending'",
    )
    .bind(reviewer_id)
    .bind(normalized_review_comment.as_deref())
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Apply approval to a submission: update/create the disc, mark the
/// submission as Approved, and return the approval outcome.
///
/// The status is claimed atomically before any disc mutations are performed.
pub async fn approve_submission(
    pool: &PgPool,
    sub: &DiscSubmission,
    changes: &serde_json::Value,
    reviewer_id: i32,
    review_comment: Option<&str>,
    expected_review_base_hash: Option<&str>,
) -> AppResult<ApprovalOutcome> {
    let mut approval_lock = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(APPROVAL_CONFLICT_LOCK_KEY)
        .execute(&mut *approval_lock)
        .await?;

    if let Some(disc_id) = sub.target_disc_id {
        let current_hash = current_disc_snapshot_hash(pool, disc_id).await?;
        if let Some(outcome) =
            stale_review_approval_outcome(expected_review_base_hash, &current_hash)
        {
            approval_lock.rollback().await?;
            return Ok(outcome);
        }
    }

    let effective_data =
        approval_effective_data(pool, sub.submission_type, sub.target_disc_id, changes).await?;
    let conflicts = find_approval_conflicts_for_effective_data(
        pool,
        sub.target_disc_id,
        changes,
        &effective_data,
    )
    .await?;
    if !conflicts.is_empty() {
        approval_lock.rollback().await?;
        return Ok(ApprovalOutcome::Conflicts(conflicts));
    }

    let previous_system_code: Option<String> = if let Some(existing_id) = sub.target_disc_id {
        sqlx::query_scalar("SELECT system_code FROM discs WHERE id = $1")
            .bind(existing_id)
            .fetch_optional(pool)
            .await?
    } else {
        None
    };
    let stored_data = changes.clone();

    // Atomically claim the submission by setting status = 'Approved'
    // only when it is still 'Pending'.  If another moderator already
    // processed it, rows_affected will be 0.
    let normalized_review_comment: Option<String> = review_comment
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let claim = sqlx::query(
        "UPDATE disc_submissions SET status = 'Approved', reviewer_id = $1,
         review_comment = $2, reviewed_at = NOW(), changes_original = changes, changes = $3
         WHERE id = $4 AND status = 'Pending'",
    )
    .bind(reviewer_id)
    .bind(normalized_review_comment.as_deref())
    .bind(&stored_data)
    .bind(sub.id)
    .execute(pool)
    .await?;

    if claim.rows_affected() == 0 {
        approval_lock.rollback().await?;
        return Ok(ApprovalOutcome::AlreadyProcessed);
    }

    let disc_id = if let Some(existing_id) = sub.target_disc_id {
        disc_service::update_disc(pool, existing_id, &effective_data).await?;

        if sub.submission_type == SubmissionType::Disc {
            sqlx::query(
                "INSERT INTO disc_dumpers (disc_id, user_id, position)
                 VALUES ($1, $2, COALESCE((SELECT MAX(position) + 1 FROM disc_dumpers WHERE disc_id = $1), 0))
                 ON CONFLICT DO NOTHING",
            )
            .bind(existing_id)
            .bind(sub.submitter_id)
            .execute(pool)
            .await?;
        }

        existing_id
    } else {
        let new_id =
            disc_service::create_disc_from_submission(pool, &effective_data, sub.submitter_id)
                .await?;

        sqlx::query("UPDATE disc_submissions SET target_disc_id = $1 WHERE id = $2")
            .bind(new_id)
            .bind(sub.id)
            .execute(pool)
            .await?;

        new_id
    };

    let system_code: Option<String> =
        sqlx::query_scalar("SELECT system_code FROM discs WHERE id = $1")
            .bind(disc_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    let mut dirty_systems = Vec::new();
    if let Some(code) = previous_system_code {
        dirty_systems.push(code);
    }
    if let Some(code) = system_code {
        if !dirty_systems.iter().any(|dirty| dirty == &code) {
            dirty_systems.push(code);
        }
    }

    for code in dirty_systems {
        archive_service::mark_system_archives_dirty(pool, &code).await?;
    }

    approval_lock.commit().await?;

    Ok(ApprovalOutcome::Approved(disc_id))
}

pub async fn get_submission(pool: &PgPool, id: i32) -> AppResult<DiscSubmission> {
    sqlx::query_as("SELECT * FROM disc_submissions WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

const DISC_SUBMISSION_HAS_DAT_ADD_SQL: &str = "COALESCE((ds.changes->'dat') ? 'add', false)";

fn submission_display_kind_sql() -> String {
    format!(
        "CASE \
         WHEN ds.submission_type = 'Edit' THEN 'Edit' \
         WHEN {DISC_SUBMISSION_HAS_DAT_ADD_SQL} THEN 'New Disc' \
         ELSE 'Verification' \
         END"
    )
}

fn submission_type_filter_condition(type_filter: Option<&str>) -> Option<String> {
    match type_filter.unwrap_or_default() {
        "Edit" => Some("ds.submission_type = 'Edit'".to_string()),
        "New Disc" => Some(format!(
            "ds.submission_type = 'Disc' AND {DISC_SUBMISSION_HAS_DAT_ADD_SQL}"
        )),
        "Verification" => Some(format!(
            "ds.submission_type = 'Disc' AND NOT ({DISC_SUBMISSION_HAS_DAT_ADD_SQL})"
        )),
        _ => None,
    }
}

pub async fn list_submissions(
    pool: &PgPool,
    user_id_filter: Option<i32>,
    disc_id_filter: Option<i32>,
    restrict_to_public_statuses: bool,
    hide_disabled_disc_targets: bool,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
    system_filter: Option<&str>,
    submitter_filter: Option<&str>,
    sort_column: &str,
    sort_order: &str,
    page: i64,
    page_size: i64,
) -> AppResult<Vec<SubmissionListRow>> {
    let offset = (page - 1) * page_size;
    let mut conditions = vec!["1=1".to_string()];
    let mut idx = 0u32;

    if user_id_filter.is_some() {
        idx += 1;
        conditions.push(format!("ds.submitter_id = ${idx}"));
    }
    if disc_id_filter.is_some() {
        idx += 1;
        conditions.push(format!("ds.target_disc_id = ${idx}"));
    }
    if restrict_to_public_statuses {
        conditions.push("ds.status IN ('Approved', 'Legacy')".to_string());
    }
    if hide_disabled_disc_targets {
        conditions.push("(d.id IS NULL OR d.status <> 'Disabled')".to_string());
    }
    if status_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("ds.status::text = ${idx}"));
    }
    if let Some(condition) = submission_type_filter_condition(type_filter) {
        conditions.push(condition);
    }
    if system_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("COALESCE(d.system_code, ds.changes->'system_code'->'add'->>'new', ds.changes->'system_code'->'modify'->>'new') = ${idx}"));
    }
    if submitter_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("LOWER(u.username) = LOWER(${idx})"));
    }

    let title_expr = "COALESCE(NULLIF(ds.changes->'title'->'add'->>'new', ''), \
                      NULLIF(ds.changes->'title'->'modify'->>'new', ''), \
                      NULLIF(d.title, ''), 'Untitled')";
    let system_expr = "CONCAT_WS(' ', NULLIF(s.manufacturer, ''), \
                       COALESCE(s.name, d.system_code, ds.changes->'system_code'->'add'->>'new', ds.changes->'system_code'->'modify'->>'new', ''))";
    let type_expr = submission_display_kind_sql();
    let changes_summary_expr = if disc_id_filter.is_some() {
        "ds.changes"
    } else {
        "'{}'::jsonb"
    };
    let sort_col = match sort_column {
        "date" => "ds.created_at".to_string(),
        "title" => format!("LOWER({title_expr})"),
        "disc_id" => "ds.target_disc_id".to_string(),
        "system" => format!("LOWER({system_expr})"),
        "submitter" => "LOWER(u.username)".to_string(),
        "reviewer" => "LOWER(COALESCE(ur.username, ''))".to_string(),
        "type" => type_expr.clone(),
        "status" => "ds.status".to_string(),
        _ => "ds.created_at".to_string(),
    };
    let sort_dir = if sort_order == "asc" { "ASC" } else { "DESC" };
    let nulls_order = if sort_column == "disc_id" {
        " NULLS LAST"
    } else {
        ""
    };

    let sql = format!(
        "SELECT ds.id, ds.submission_type,
                {dat_add_expr} AS submission_has_dat_add,
                {title_expr} AS title,
                COALESCE(ds.submission_comment, '') AS submission_comment,
                {changes_summary_expr} AS changes_summary,
                COALESCE(d.system_code, ds.changes->'system_code'->'add'->>'new', ds.changes->'system_code'->'modify'->>'new', '') AS system_code,
                COALESCE(s.short_name, '') AS system_short_name,
                u.username AS submitter,
                ds.submitter_id,
                ur.username AS reviewer,
                ds.reviewer_id,
                ds.status,
                ds.target_disc_id,
                ds.created_at
         FROM disc_submissions ds
         JOIN users u ON u.id = ds.submitter_id
         LEFT JOIN users ur ON ur.id = ds.reviewer_id
         LEFT JOIN discs d ON d.id = ds.target_disc_id
         LEFT JOIN systems s
             ON s.code = COALESCE(d.system_code, ds.changes->'system_code'->'add'->>'new', ds.changes->'system_code'->'modify'->>'new')
         WHERE {}
         ORDER BY {sort_col} {sort_dir}{nulls_order}
         LIMIT {page_size} OFFSET {offset}",
        conditions.join(" AND "),
        dat_add_expr = DISC_SUBMISSION_HAS_DAT_ADD_SQL
    );

    let mut query = sqlx::query_as::<_, SubmissionListRow>(&sql);
    if let Some(uid) = user_id_filter {
        query = query.bind(uid);
    }
    if let Some(disc_id) = disc_id_filter {
        query = query.bind(disc_id);
    }
    if let Some(status) = status_filter {
        if !status.is_empty() {
            query = query.bind(status.to_string());
        }
    }
    if let Some(system) = system_filter {
        if !system.is_empty() {
            query = query.bind(system.to_string());
        }
    }
    if let Some(submitter) = submitter_filter {
        if !submitter.is_empty() {
            query = query.bind(submitter.to_string());
        }
    }

    Ok(query.fetch_all(pool).await?)
}

pub async fn count_submissions(
    pool: &PgPool,
    user_id_filter: Option<i32>,
    disc_id_filter: Option<i32>,
    restrict_to_public_statuses: bool,
    hide_disabled_disc_targets: bool,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
    system_filter: Option<&str>,
    submitter_filter: Option<&str>,
) -> AppResult<i64> {
    let mut conditions = vec!["1=1".to_string()];
    let mut idx = 0u32;

    if user_id_filter.is_some() {
        idx += 1;
        conditions.push(format!("ds.submitter_id = ${idx}"));
    }
    if disc_id_filter.is_some() {
        idx += 1;
        conditions.push(format!("ds.target_disc_id = ${idx}"));
    }
    if restrict_to_public_statuses {
        conditions.push("ds.status IN ('Approved', 'Legacy')".to_string());
    }
    if hide_disabled_disc_targets {
        conditions.push("(d.id IS NULL OR d.status <> 'Disabled')".to_string());
    }
    if status_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("ds.status::text = ${idx}"));
    }
    if let Some(condition) = submission_type_filter_condition(type_filter) {
        conditions.push(condition);
    }
    if system_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("COALESCE(d.system_code, ds.changes->'system_code'->'add'->>'new', ds.changes->'system_code'->'modify'->>'new') = ${idx}"));
    }
    if submitter_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("LOWER(u.username) = LOWER(${idx})"));
    }

    let sql = format!(
        "SELECT COUNT(*)
         FROM disc_submissions ds
         JOIN users u ON u.id = ds.submitter_id
         LEFT JOIN discs d ON d.id = ds.target_disc_id
         WHERE {}",
        conditions.join(" AND ")
    );

    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    if let Some(uid) = user_id_filter {
        query = query.bind(uid);
    }
    if let Some(disc_id) = disc_id_filter {
        query = query.bind(disc_id);
    }
    if let Some(status) = status_filter {
        if !status.is_empty() {
            query = query.bind(status.to_string());
        }
    }
    if let Some(system) = system_filter {
        if !system.is_empty() {
            query = query.bind(system.to_string());
        }
    }
    if let Some(submitter) = submitter_filter {
        if !submitter.is_empty() {
            query = query.bind(submitter.to_string());
        }
    }

    Ok(query.fetch_one(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::time::Duration;

    fn file(
        track_number: &str,
        size: i64,
        crc32: &str,
        md5: &str,
        sha1: &str,
    ) -> crate::db::models::File {
        crate::db::models::File {
            id: 1,
            disc_id: 1,
            track_number: Some(track_number.to_string()),
            size,
            crc32: crc32.to_string(),
            md5: md5.to_string(),
            sha1: sha1.to_string(),
        }
    }

    fn rom_line(track_number: &str, size: i64, crc32: &str, md5: &str, sha1: &str) -> String {
        format!(
            r#"<rom name="Track {track_number}.bin" size="{size}" crc="{crc32}" md5="{md5}" sha1="{sha1}" />"#
        )
    }

    fn ring_entry(mastering_code: &str, comment: &str) -> serde_json::Value {
        serde_json::json!({
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": comment,
            "layers": [
                {
                    "mastering_code": mastering_code,
                    "mastering_sid": "",
                    "toolstamps": "",
                    "mould_sids": "",
                    "additional_moulds": ""
                }
            ]
        })
    }

    fn unreachable_pool() -> PgPool {
        let options = PgConnectOptions::new()
            .host("127.0.0.1")
            .port(1)
            .username("vgindex")
            .database("vgindex");

        PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy_with(options)
    }

    fn submission(
        submission_type: SubmissionType,
        target_disc_id: Option<i32>,
        changes: serde_json::Value,
    ) -> DiscSubmission {
        DiscSubmission {
            id: 1,
            submission_type,
            submitter_id: 1,
            submission_comment: None,
            target_disc_id,
            changes_original: None,
            changes,
            dump_log: None,
            extra_upload_url: None,
            status: SubmissionStatus::Pending,
            reviewer_id: None,
            review_comment: None,
            created_at: chrono::Utc::now(),
            reviewed_at: None,
        }
    }

    #[tokio::test]
    async fn public_match_helpers_return_database_errors() {
        let pool = unreachable_pool();
        let dat = rom_line(
            "1",
            100,
            "11111111",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );

        let dat_err = find_matching_disc(&pool, &dat, false).await.unwrap_err();
        assert!(matches!(dat_err, AppError::Database(_)));

        let hash_err = find_matching_disc_by_universal_hash(
            &pool,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(hash_err, AppError::Database(_)));
    }

    #[test]
    fn disc_snapshot_hash_is_stable_for_object_key_order_and_changes_on_data_change() {
        let left = serde_json::json!({
            "title": "Game",
            "nested": {
                "z": 2,
                "a": [
                    {
                        "right": true,
                        "left": false
                    }
                ]
            }
        });
        let right = serde_json::json!({
            "nested": {
                "a": [
                    {
                        "left": false,
                        "right": true
                    }
                ],
                "z": 2
            },
            "title": "Game"
        });
        let changed = serde_json::json!({
            "nested": {
                "a": [
                    {
                        "left": false,
                        "right": true
                    }
                ],
                "z": 3
            },
            "title": "Game"
        });

        assert_eq!(disc_snapshot_hash(&left), disc_snapshot_hash(&right));
        assert_ne!(disc_snapshot_hash(&right), disc_snapshot_hash(&changed));
    }

    #[test]
    fn review_base_hash_stale_decision_requires_current_hash_match_when_provided() {
        assert!(!review_base_hash_is_stale(None, "current"));
        assert!(!review_base_hash_is_stale(Some(" current "), "current"));
        assert!(review_base_hash_is_stale(Some("previous"), "current"));
        assert!(review_base_hash_is_stale(Some(""), "current"));
    }

    #[test]
    fn stale_review_hash_maps_to_stale_approval_outcome() {
        assert_eq!(
            stale_review_approval_outcome(Some("previous"), "current"),
            Some(ApprovalOutcome::StaleDiscState)
        );
        assert_eq!(
            stale_review_approval_outcome(Some("current"), "current"),
            None
        );
        assert_eq!(stale_review_approval_outcome(None, "current"), None);
    }

    #[test]
    fn submission_type_filter_conditions_split_disc_submissions_by_dat_add() {
        assert_eq!(submission_type_filter_condition(None), None);
        assert_eq!(submission_type_filter_condition(Some("")), None);
        assert_eq!(
            submission_type_filter_condition(Some("Edit")),
            Some("ds.submission_type = 'Edit'".to_string())
        );
        assert_eq!(
            submission_type_filter_condition(Some("New Disc")),
            Some(format!(
                "ds.submission_type = 'Disc' AND {DISC_SUBMISSION_HAS_DAT_ADD_SQL}"
            ))
        );
        assert_eq!(
            submission_type_filter_condition(Some("Verification")),
            Some(format!(
                "ds.submission_type = 'Disc' AND NOT ({DISC_SUBMISSION_HAS_DAT_ADD_SQL})"
            ))
        );
        assert_eq!(submission_type_filter_condition(Some("Disc")), None);
        assert_eq!(submission_type_filter_condition(Some("Unknown")), None);
    }

    #[test]
    fn dat_match_uses_numeric_track_order() {
        let files = vec![
            file("1", 100, "11111111", &"1".repeat(32), &"1".repeat(40)),
            file("10", 1000, "aaaaaaaa", &"a".repeat(32), &"a".repeat(40)),
            file("2", 200, "22222222", &"2".repeat(32), &"2".repeat(40)),
        ];
        let dat = [
            rom_line("1", 100, "11111111", &"1".repeat(32), &"1".repeat(40)),
            rom_line("2", 200, "22222222", &"2".repeat(32), &"2".repeat(40)),
            rom_line("10", 1000, "aaaaaaaa", &"a".repeat(32), &"a".repeat(40)),
        ]
        .join("\n");
        let submitted = parse_rom_entries(&dat);

        assert!(files_match_submission(&files, &submitted));
    }

    #[test]
    fn dat_match_ignores_hash_case() {
        let files = vec![file(
            "1",
            100,
            "deadbeef",
            "0123456789abcdef0123456789abcdef",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )];
        let dat = rom_line(
            "1",
            100,
            "DEADBEEF",
            "0123456789ABCDEF0123456789ABCDEF",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        );
        let submitted = parse_rom_entries(&dat);

        assert!(files_match_submission(&files, &submitted));
    }

    #[test]
    fn dat_match_accepts_single_iso_file() {
        let files = vec![file("0", 100, "11111111", &"1".repeat(32), &"1".repeat(40))];
        let dat = r#"<rom name="Game.iso" size="100" crc="11111111" md5="11111111111111111111111111111111" sha1="1111111111111111111111111111111111111111" />"#;
        let submitted = parse_rom_entries(dat);

        assert_eq!(submitted[0].track_number.as_deref(), Some("0"));
        assert!(files_match_submission(&files, &submitted));
    }

    #[test]
    fn dat_match_rejects_wrong_track_hash() {
        let files = vec![
            file("1", 100, "11111111", &"1".repeat(32), &"1".repeat(40)),
            file("2", 200, "22222222", &"2".repeat(32), &"2".repeat(40)),
        ];
        let dat = [
            rom_line("1", 100, "11111111", &"1".repeat(32), &"1".repeat(40)),
            rom_line("2", 200, "33333333", &"3".repeat(32), &"3".repeat(40)),
        ]
        .join("\n");
        let submitted = parse_rom_entries(&dat);

        assert!(!files_match_submission(&files, &submitted));
    }

    #[test]
    fn universal_hash_matching_normalizes_valid_sha1_hex() {
        let hash = "AABBCCDDEEFF00112233445566778899AABBCCDD";
        let bytes = universal_hash_bytes_for_matching(Some(hash)).unwrap();

        assert_eq!(
            bytes,
            vec![
                0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
                0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd
            ]
        );
        assert_eq!(
            universal_hash_bytes_for_matching(Some("  aabbccddeeff00112233445566778899aabbccdd  ")),
            Some(bytes)
        );
    }

    #[test]
    fn universal_hash_matching_rejects_invalid_sha1_hex() {
        assert!(universal_hash_bytes_for_matching(None).is_none());
        assert!(universal_hash_bytes_for_matching(Some("")).is_none());
        assert!(universal_hash_bytes_for_matching(Some("abc123")).is_none());
        assert!(universal_hash_bytes_for_matching(Some(&"g".repeat(40))).is_none());
        assert!(universal_hash_bytes_for_matching(Some(&"a".repeat(41))).is_none());
    }

    #[test]
    fn dat_conflict_check_runs_only_when_dat_is_in_changes() {
        let effective = serde_json::json!({
            "dat": r#"<rom name="Game.iso" size="1" crc="11111111" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" />"#
        });

        assert_eq!(
            dat_hash_conflict_input(
                &serde_json::json!({"dat": {"add": {"new": "x"}}}),
                &effective
            ),
            effective["dat"].as_str()
        );
        assert_eq!(
            dat_hash_conflict_input(
                &serde_json::json!({"title": {"modify": {"old": "Old", "new": "New"}}}),
                &effective
            ),
            None
        );
        assert_eq!(
            dat_hash_conflict_input(
                &serde_json::json!({"dat": {"remove": {"old": "x"}}}),
                &serde_json::json!({"dat": null})
            ),
            None
        );
    }

    #[test]
    fn universal_hash_conflict_check_runs_only_when_hash_is_in_changes() {
        let effective = serde_json::json!({
            "universal_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        });

        assert_eq!(
            universal_hash_conflict_input(
                &serde_json::json!({"universal_hash": {"add": {"new": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}),
                &effective
            ),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(
            universal_hash_conflict_input(
                &serde_json::json!({"comments": {"add": {"new": "metadata only"}}}),
                &effective
            ),
            None
        );
        assert_eq!(
            universal_hash_conflict_input(
                &serde_json::json!({"universal_hash": {"remove": {"old": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}),
                &serde_json::json!({"universal_hash": null})
            ),
            None
        );
    }

    #[test]
    fn applies_new_disc_changes_to_empty_snapshot() {
        let changes = serde_json::json!({
            "title": { "add": { "new": "New Game" } },
            "regions": { "add": ["AS", "EU"] },
            "languages": { "add": ["ja"] },
            "serial": { "add": ["ABC-001"] },
            "edition": { "add": ["Limited"] },
            "barcode": { "add": ["1234567890"] },
            "layerbreaks": { "add": { "new": [12345] } },
            "ring_codes": [{
                "offset_value": { "add": { "new": "0" } },
                "comment": { "add": { "new": "new pressing" } },
                "layers": [{
                    "index": 0,
                    "mastering_code": { "add": { "new": "MASTER-A" } },
                    "mastering_sid": { "add": { "new": "SID-A" } },
                    "toolstamps": { "add": { "new": "T1" } }
                }]
            }]
        });
        let sub = submission(SubmissionType::Disc, None, changes);

        let result =
            resolve_submission_snapshot_for_submission(&serde_json::json!({}), &sub).unwrap();

        assert_eq!(result["title"], "New Game");
        assert_eq!(result["regions"], serde_json::json!(["AS", "EU"]));
        assert_eq!(result["languages"], serde_json::json!(["ja"]));
        assert_eq!(result["serial"], serde_json::json!(["ABC-001"]));
        assert_eq!(result["edition"], serde_json::json!(["Limited"]));
        assert_eq!(result["barcode"], serde_json::json!(["1234567890"]));
        assert_eq!(result["layerbreaks"], serde_json::json!([12345]));
        assert_eq!(result["ring_codes"].as_array().unwrap().len(), 1);
        assert_eq!(result["ring_codes"][0]["offset_value"], "0");
        assert_eq!(result["ring_codes"][0]["comment"], "new pressing");
        assert_eq!(
            result["ring_codes"][0]["layers"][0]["mastering_code"],
            "MASTER-A"
        );
        assert_eq!(result["ring_codes"][0]["layers"][0]["toolstamps"], "T1");
    }

    #[test]
    fn applies_add_disc_verification_payload_without_duplicating_existing_values() {
        let db = serde_json::json!({
            "title": "Old Game",
            "regions": ["Europe"],
            "languages": ["en"],
            "serial": ["ABC-001"],
            "edition": ["Original"],
            "barcode": ["1234567890"],
            "dat": r#"<rom name="Existing.iso" size="1" crc="11111111" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" />"#,
            "ring_codes": [{
                "id": 1,
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "",
                "layers": [{
                    "mastering_code": "MASTER-A",
                    "mastering_sid": "SID-A",
                    "toolstamps": "T1",
                    "mould_sids": "",
                    "additional_moulds": ""
                }]
            }]
        });
        let changes = serde_json::json!({
            "title": { "modify": { "old": "Old Game", "new": "Updated Game" } },
            "regions": { "add": ["Asia", "Europe"] },
            "languages": { "add": ["ja"] },
            "serial": { "add": ["ABC-001", "DEF-002"] },
            "edition": { "add": ["Limited"] },
            "barcode": { "add": ["1234567890", "0987654321"] },
            "ring_codes": [
                {
                    "layers": [{
                        "index": 0,
                        "mastering_code": { "add": { "new": "MASTER-A" } },
                        "mastering_sid": { "add": { "new": "SID-A" } },
                        "toolstamps": { "add": { "new": "T2" } }
                    }]
                }
            ]
        });
        let sub = submission(SubmissionType::Disc, Some(1), changes);

        let result = resolve_submission_snapshot_for_submission(&db, &sub).unwrap();

        assert_eq!(result["title"], "Updated Game");
        assert_eq!(result["regions"], serde_json::json!(["Europe", "Asia"]));
        assert_eq!(result["languages"], serde_json::json!(["en", "ja"]));
        assert_eq!(result["serial"], serde_json::json!(["ABC-001", "DEF-002"]));
        assert_eq!(
            result["edition"],
            serde_json::json!(["Original", "Limited"])
        );
        assert_eq!(
            result["barcode"],
            serde_json::json!(["1234567890", "0987654321"])
        );
        assert_eq!(
            result["dat"],
            r#"<rom name="Existing.iso" size="1" crc="11111111" md5="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" />"#
        );
        assert_eq!(result["ring_codes"].as_array().unwrap().len(), 1);
        assert_eq!(result["ring_codes"][0]["id"], 1);
        assert_eq!(result["ring_codes"][0]["layers"][0]["toolstamps"], "T1, T2");
    }

    #[test]
    fn applies_moderator_verification_delta_with_set_adds_and_removes() {
        let db = serde_json::json!({
            "regions": ["Europe"],
            "languages": ["en"],
            "serial": ["ABC-001"],
            "edition": ["Original"],
            "barcode": ["1234567890"],
        });
        let changes = serde_json::json!({
            "regions": { "remove": ["Europe"], "add": ["Asia"] },
            "languages": { "remove": ["en"], "add": ["ja"] },
            "serial": { "remove": ["ABC-001"], "add": ["abc-001"] },
            "edition": { "remove": ["Original"], "add": ["original"] },
            "barcode": { "remove": ["1234567890"], "add": ["0987654321"] },
        });
        let sub = submission(SubmissionType::Disc, Some(1), changes);

        let result = resolve_submission_snapshot_for_submission(&db, &sub).unwrap();

        assert_eq!(result["regions"], serde_json::json!(["Asia"]));
        assert_eq!(result["languages"], serde_json::json!(["ja"]));
        assert_eq!(result["serial"], serde_json::json!(["abc-001"]));
        assert_eq!(result["edition"], serde_json::json!(["original"]));
        assert_eq!(result["barcode"], serde_json::json!(["0987654321"]));
    }

    #[test]
    fn applies_edit_changes_with_scalar_set_and_ring_operations() {
        let db = serde_json::json!({
            "title": "Old Game",
            "comments": "old comment",
            "regions": ["EU"],
            "languages": ["en"],
            "serial": ["ABC-001", "OLD-002"],
            "edition": ["Original"],
            "barcode": ["1234567890"],
            "layerbreaks": [10, 20],
            "ring_codes": [
                {
                    "id": 1,
                    "offset_value": "",
                    "offset_extra_value": "",
                    "sample_start": "",
                    "comment": "old",
                    "layers": [{
                        "mastering_code": "MASTER-A",
                        "mastering_sid": "SID-A",
                        "toolstamps": "T1",
                        "mould_sids": "",
                        "additional_moulds": ""
                    }]
                },
                {
                    "id": 2,
                    "offset_value": "",
                    "offset_extra_value": "",
                    "sample_start": "",
                    "comment": "remove me",
                    "layers": [{
                        "mastering_code": "MASTER-B",
                        "mastering_sid": "SID-B",
                        "toolstamps": "T9",
                        "mould_sids": "",
                        "additional_moulds": ""
                    }]
                }
            ]
        });
        let changes = serde_json::json!({
            "title": { "modify": { "old": "Old Game", "new": "Edited Game" } },
            "comments": { "remove": { "old": "old comment" } },
            "regions": { "remove": ["EU"], "add": ["AS"] },
            "languages": { "remove": ["en"] },
            "serial": { "remove": ["ABC-001", "OLD-002"], "add": ["XYZ-999", "NEW-003"] },
            "edition": { "remove": ["Original"], "add": ["Greatest Hits"] },
            "barcode": { "remove": ["1234567890"], "add": ["5555555555"] },
            "layerbreaks": { "modify": { "old": [10, 20], "new": [15, 30] } },
            "ring_codes": [
                {
                    "id": 1,
                    "comment": { "modify": { "old": "old", "new": "updated" } },
                    "layers": [{
                        "index": 0,
                        "toolstamps": { "modify": { "old": "T1", "new": "T1, T2" } }
                    }]
                },
                { "id": 2, "remove": true }
            ]
        });
        let sub = submission(SubmissionType::Edit, Some(1), changes);

        let result = resolve_submission_snapshot_for_submission(&db, &sub).unwrap();

        assert_eq!(result["title"], "Edited Game");
        assert!(result["comments"].is_null());
        assert_eq!(result["regions"], serde_json::json!(["AS"]));
        assert_eq!(result["languages"], serde_json::json!([]));
        assert_eq!(result["serial"], serde_json::json!(["XYZ-999", "NEW-003"]));
        assert_eq!(result["edition"], serde_json::json!(["Greatest Hits"]));
        assert_eq!(result["barcode"], serde_json::json!(["5555555555"]));
        assert_eq!(result["layerbreaks"], serde_json::json!([15, 30]));
        assert_eq!(result["ring_codes"].as_array().unwrap().len(), 1);
        assert_eq!(result["ring_codes"][0]["id"], 1);
        assert_eq!(result["ring_codes"][0]["comment"], "updated");
        assert_eq!(result["ring_codes"][0]["layers"][0]["toolstamps"], "T1, T2");
    }

    #[test]
    fn scalar_remove_rejects_required_fields() {
        let db = serde_json::json!({
            "title": "Old Game",
        });
        let changes = serde_json::json!({
            "title": { "remove": { "old": "Old Game" } },
        });

        assert!(resolve_submission_snapshot(&db, &changes).is_err());
    }

    #[test]
    fn nullable_scalar_remove_resolves_to_null() {
        let db = serde_json::json!({
            "title": "Old Game",
            "comments": "remove me",
            "layerbreaks": [10, 20],
        });
        let changes = serde_json::json!({
            "comments": { "remove": { "old": "remove me" } },
            "layerbreaks": { "remove": { "old": [10, 20] } },
        });

        let result = resolve_submission_snapshot(&db, &changes).unwrap();

        assert!(result["comments"].is_null());
        assert!(result["layerbreaks"].is_null());
        assert_eq!(result["title"], "Old Game");
    }

    #[test]
    fn scalar_operations_reject_malformed_wrappers() {
        let db = serde_json::json!({ "comments": "old" });
        let cases = [
            (
                serde_json::json!({ "comments": { "add": { "old": "old", "new": "new" } } }),
                "requires only new",
            ),
            (
                serde_json::json!({ "comments": { "modify": { "new": "new" } } }),
                "requires old and new",
            ),
            (
                serde_json::json!({ "comments": { "remove": {} } }),
                "requires only old",
            ),
            (
                serde_json::json!({ "comments": { "replace": { "new": "new" } } }),
                "unknown operation",
            ),
            (
                serde_json::json!({ "comments": { "add": { "new": "new" }, "remove": { "old": "old" } } }),
                "exactly one operation",
            ),
        ];

        for (changes, expected) in cases {
            let err = resolve_submission_snapshot(&db, &changes).unwrap_err();
            match err {
                AppError::BadRequest(msg) => assert!(
                    msg.contains(expected),
                    "expected `{msg}` to contain `{expected}`"
                ),
                other => panic!("unexpected error: {other:?}"),
            }
        }
    }

    #[test]
    fn set_operations_are_case_sensitive_and_deduplicate_on_apply() {
        let db = serde_json::json!({
            "regions": ["Europe"],
            "serial": ["ABC-001"],
        });
        let changes = serde_json::json!({
            "regions": { "add": ["Europe", "europe", "Asia", "Asia"], "remove": ["Missing"] },
            "serial": { "remove": ["ABC-001"], "add": ["abc-001", "ABC-001", "abc-001"] },
        });

        let result = resolve_submission_snapshot(&db, &changes).unwrap();

        assert_eq!(
            result["regions"],
            serde_json::json!(["Europe", "europe", "Asia"])
        );
        assert_eq!(result["serial"], serde_json::json!(["abc-001", "ABC-001"]));
    }

    #[test]
    fn set_operations_reject_malformed_wrappers() {
        let db = serde_json::json!({ "regions": ["Europe"] });
        let cases = [
            (
                serde_json::json!({ "regions": {} }),
                "add and/or remove arrays",
            ),
            (
                serde_json::json!({ "regions": { "add": "Asia" } }),
                "add operation must be an array",
            ),
            (
                serde_json::json!({ "regions": { "remove": [1] } }),
                "remove operation values must be strings",
            ),
            (
                serde_json::json!({ "regions": { "modify": ["Asia"] } }),
                "add and/or remove arrays",
            ),
        ];

        for (changes, expected) in cases {
            let err = resolve_submission_snapshot(&db, &changes).unwrap_err();
            match err {
                AppError::BadRequest(msg) => assert!(
                    msg.contains(expected),
                    "expected `{msg}` to contain `{expected}`"
                ),
                other => panic!("unexpected error: {other:?}"),
            }
        }
    }

    #[test]
    fn resolver_preserves_unchanged_snapshot_fields() {
        let db = serde_json::json!({
            "title": "Old Game",
            "system_code": "SYS",
            "regions": ["Europe"],
            "languages": ["en"],
            "comments": "keep me",
            "ring_codes": []
        });
        let changes = serde_json::json!({
            "regions": { "add": ["Asia"] },
        });

        let result = resolve_submission_snapshot(&db, &changes).unwrap();

        assert_eq!(result["title"], "Old Game");
        assert_eq!(result["system_code"], "SYS");
        assert_eq!(result["comments"], "keep me");
        assert_eq!(result["languages"], serde_json::json!(["en"]));
        assert_eq!(result["regions"], serde_json::json!(["Europe", "Asia"]));
    }

    #[test]
    fn resolver_is_submission_type_agnostic() {
        let db = serde_json::json!({
            "title": "Old Game",
            "regions": ["Europe"],
        });
        let changes = serde_json::json!({
            "title": { "modify": { "old": "Old Game", "new": "New Game" } },
            "regions": { "add": ["Asia"] },
        });
        let disc_sub = submission(SubmissionType::Disc, Some(1), changes.clone());
        let edit_sub = submission(SubmissionType::Edit, Some(1), changes);

        let disc_result = resolve_submission_snapshot_for_submission(&db, &disc_sub).unwrap();
        let edit_result = resolve_submission_snapshot_for_submission(&db, &edit_sub).unwrap();

        assert_eq!(disc_result, edit_result);
        assert_eq!(disc_result["title"], "New Game");
        assert_eq!(
            disc_result["regions"],
            serde_json::json!(["Europe", "Asia"])
        );
    }

    #[test]
    fn new_disc_resolve_still_uses_submitted_values() {
        let changes = serde_json::json!({
            "regions": { "add": ["AS"] },
            "languages": { "add": ["ja"] },
            "serial": { "add": ["DEF"] },
        });
        let sub = submission(SubmissionType::Disc, None, changes);

        let result =
            resolve_submission_snapshot_for_submission(&serde_json::json!({}), &sub).unwrap();

        assert_eq!(result["regions"], serde_json::json!(["AS"]));
        assert_eq!(result["languages"], serde_json::json!(["ja"]));
        assert_eq!(result["serial"], serde_json::json!(["DEF"]));
    }

    #[test]
    fn ring_code_modify_can_create_missing_layer_by_index() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [{
                "mastering_code": "L0",
                "mastering_sid": "",
                "toolstamps": "",
                "mould_sids": "",
                "additional_moulds": ""
            }]
        }]);
        let changes = serde_json::json!([{
            "id": 1,
            "layers": [{
                "index": 1,
                "mastering_code": { "add": { "new": "L1" } },
                "mastering_sid": { "add": { "new": "SID-L1" } },
                "toolstamps": { "add": { "new": "T2, T1, T2" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();

        assert_eq!(result[0]["layers"].as_array().unwrap().len(), 2);
        assert_eq!(result[0]["layers"][1]["mastering_code"], "L1");
        assert_eq!(result[0]["layers"][1]["mastering_sid"], "SID-L1");
        assert_eq!(result[0]["layers"][1]["toolstamps"], "T1, T2");
    }

    #[test]
    fn apply_ring_codes_history_uses_entry_id() {
        let old = serde_json::json!([
            serde_json::json!({
                "id": 20,
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "",
                "layers": [{ "mastering_code": "B", "mastering_sid": "", "toolstamps": "", "mould_sids": "", "additional_moulds": "" }]
            }),
            serde_json::json!({
                "id": 10,
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "",
                "layers": [{ "mastering_code": "A", "mastering_sid": "", "toolstamps": "", "mould_sids": "", "additional_moulds": "" }]
            })
        ]);
        let changes = serde_json::json!([
            {
                "id": 10,
                "comment": { "add": { "new": "updated" } }
            }
        ]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries[0]["id"], 10);
        assert_eq!(entries[0]["layers"][0]["mastering_code"], "A");
        assert_eq!(entries[0]["comment"], "updated");
    }

    #[test]
    fn apply_ring_codes_history_rejects_missing_entry_id() {
        let old = serde_json::json!([ring_entry("A", "")]);
        let changes = serde_json::json!([
            {
                "remove": true
            }
        ]);

        let err = apply_ring_codes_history(&old, &changes).unwrap_err();
        match err {
            AppError::BadRequest(msg) => assert!(msg.contains("requires entry id")),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn apply_ring_codes_history_does_not_merge_addition_into_removed_entry() {
        let old = serde_json::json!([{
            "id": 3116,
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "inherited",
            "layers": [{
                "mastering_code": "",
                "mastering_sid": "",
                "toolstamps": "",
                "mould_sids": "",
                "additional_moulds": ""
            }]
        }]);
        let changes = serde_json::json!([
            {
                "offset_value": { "add": { "new": "2" } },
                "layers": [{
                    "index": 0,
                    "mould_sids": { "add": { "new": "IFPI 947R" } }
                }]
            },
            {
                "id": 3116,
                "remove": true
            }
        ]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries[0].get("id").is_none());
        assert_eq!(entries[0]["offset_value"], "2");
        assert_eq!(entries[0]["comment"], "");
        assert_eq!(entries[0]["layers"][0]["mould_sids"], "IFPI 947R");
    }

    #[test]
    fn apply_ring_codes_history_does_not_merge_multiple_additions_into_removed_entries() {
        let old = serde_json::json!([
            {
                "id": 1,
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "first inherited",
                "layers": [{
                    "mastering_code": "",
                    "mastering_sid": "",
                    "toolstamps": "",
                    "mould_sids": "",
                    "additional_moulds": ""
                }]
            },
            {
                "id": 2,
                "offset_value": "",
                "offset_extra_value": "",
                "sample_start": "",
                "comment": "second inherited",
                "layers": [{
                    "mastering_code": "",
                    "mastering_sid": "",
                    "toolstamps": "",
                    "mould_sids": "",
                    "additional_moulds": ""
                }]
            }
        ]);
        let changes = serde_json::json!([
            {
                "offset_value": { "add": { "new": "11" } },
                "layers": [{
                    "index": 0,
                    "mould_sids": { "add": { "new": "IFPI A111" } }
                }]
            },
            {
                "offset_value": { "add": { "new": "22" } },
                "layers": [{
                    "index": 0,
                    "mould_sids": { "add": { "new": "IFPI B222" } }
                }]
            },
            {
                "id": 2,
                "remove": true
            },
            {
                "id": 1,
                "remove": true
            }
        ]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.get("id").is_none()));
        assert_eq!(entries[0]["offset_value"], "11");
        assert_eq!(entries[0]["layers"][0]["mould_sids"], "IFPI A111");
        assert_eq!(entries[1]["offset_value"], "22");
        assert_eq!(entries[1]["layers"][0]["mould_sids"], "IFPI B222");
    }

    #[test]
    fn merge_ring_entry_when_mastering_matches() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [{
                "mastering_code": "ABCD",
                "mastering_sid": "SID-1",
                "toolstamps": "TS-A",
                "mould_sids": "MS-1",
                "additional_moulds": "AM-X"
            }]
        }]);
        let changes = serde_json::json!([{
            "layers": [{
                "index": 0,
                "mastering_code": { "add": { "new": "ABCD" } },
                "mastering_sid": { "add": { "new": "SID-1" } },
                "toolstamps": { "add": { "new": "TS-B" } },
                "mould_sids": { "add": { "new": "MS-2" } },
                "additional_moulds": { "add": { "new": "AM-Y" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 1, "should merge, not add new entry");
        let layer = &entries[0]["layers"][0];
        assert_eq!(layer["toolstamps"], "TS-A, TS-B");
        assert_eq!(layer["mould_sids"], "MS-1, MS-2");
        assert_eq!(layer["additional_moulds"], "AM-X, AM-Y");
    }

    #[test]
    fn no_merge_when_mastering_code_differs() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [{
                "mastering_code": "ABCD",
                "mastering_sid": "SID-1",
                "toolstamps": "TS-A",
                "mould_sids": "",
                "additional_moulds": ""
            }]
        }]);
        let changes = serde_json::json!([{
            "layers": [{
                "index": 0,
                "mastering_code": { "add": { "new": "DIFFERENT" } },
                "mastering_sid": { "add": { "new": "SID-1" } },
                "toolstamps": { "add": { "new": "TS-B" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(
            entries.len(),
            2,
            "should add new entry when mastering_code differs"
        );
    }

    #[test]
    fn merge_deduplicates_csv_values() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [{
                "mastering_code": "X",
                "mastering_sid": "",
                "toolstamps": "A, B",
                "mould_sids": "M1",
                "additional_moulds": ""
            }]
        }]);
        let changes = serde_json::json!([{
            "layers": [{
                "index": 0,
                "mastering_code": { "add": { "new": "X" } },
                "mastering_sid": { "add": { "new": "" } },
                "toolstamps": { "add": { "new": "B, C" } },
                "mould_sids": { "add": { "new": "m1, M2" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let layer = &entries[0]["layers"][0];
        assert_eq!(layer["toolstamps"], "A, B, C");
        assert_eq!(layer["mould_sids"], "M1, M2");
    }

    #[test]
    fn merge_matches_when_one_offset_empty() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "42",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [{
                "mastering_code": "Z",
                "mastering_sid": "",
                "toolstamps": "T1",
                "mould_sids": "",
                "additional_moulds": ""
            }]
        }]);
        let changes = serde_json::json!([{
            "offset_value": { "add": { "new": "" } },
            "layers": [{
                "index": 0,
                "mastering_code": { "add": { "new": "Z" } },
                "mastering_sid": { "add": { "new": "" } },
                "toolstamps": { "add": { "new": "T2" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 1, "empty offset on change side should match");
        assert_eq!(entries[0]["layers"][0]["toolstamps"], "T1, T2");
    }

    #[test]
    fn no_merge_when_offsets_differ() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "42",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [{
                "mastering_code": "Z",
                "mastering_sid": "",
                "toolstamps": "",
                "mould_sids": "",
                "additional_moulds": ""
            }]
        }]);
        let changes = serde_json::json!([{
            "offset_value": { "add": { "new": "99" } },
            "layers": [{
                "index": 0,
                "mastering_code": { "add": { "new": "Z" } },
                "mastering_sid": { "add": { "new": "" } },
                "toolstamps": { "add": { "new": "T1" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(
            entries.len(),
            2,
            "different non-empty offsets should not merge"
        );
    }
}
