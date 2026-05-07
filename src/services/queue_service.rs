use sqlx::PgPool;

use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::disc_service;

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
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
            Some(RomEntry {
                size: extract_xml_attr(line, "size")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                crc32: extract_xml_attr(line, "crc").unwrap_or_default(),
                md5: extract_xml_attr(line, "md5").unwrap_or_default(),
                sha1: extract_xml_attr(line, "sha1").unwrap_or_default(),
            })
        })
        .collect()
}

pub async fn find_matching_disc(pool: &PgPool, files_xml: &str) -> Option<i32> {
    let submitted = parse_rom_entries(files_xml);
    if submitted.is_empty() {
        return None;
    }

    let candidates: Vec<i32> = sqlx::query_scalar(
        "SELECT DISTINCT disc_id FROM files WHERE sha1 = $1",
    )
    .bind(&submitted[0].sha1)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for disc_id in candidates {
        let disc_files: Vec<crate::db::models::File> = sqlx::query_as(
            "SELECT * FROM files WHERE disc_id = $1 AND track_number IS NOT NULL ORDER BY track_number",
        )
        .bind(disc_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        if disc_files.len() != submitted.len() {
            continue;
        }

        let all_match = disc_files.iter().zip(&submitted).all(|(df, sf)| {
            df.size == sf.size && df.crc32 == sf.crc32 && df.md5 == sf.md5 && df.sha1 == sf.sha1
        });

        if all_match {
            return Some(disc_id);
        }
    }

    None
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


fn history_new_value(node: &serde_json::Value) -> Option<&serde_json::Value> {
    node.as_object().and_then(|obj| obj.get("new"))
}

fn apply_string_list_history(
    old_values: &[String],
    changes: &serde_json::Value,
    allow_removal: bool,
) -> Vec<String> {
    let mut out = old_values.to_vec();
    let Some(items) = changes.as_array() else {
        return out;
    };

    for item in items {
        let idx = item.get("index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let new_value = item.get("new");
        match (idx, new_value) {
            (Some(i), Some(serde_json::Value::Null)) => {
                if allow_removal && i < out.len() {
                    out.remove(i);
                }
            }
            (Some(_), None) => {}
            (Some(i), Some(v)) => {
                let text = v.as_str().unwrap_or("").to_string();
                if i < out.len() {
                    out[i] = text;
                } else {
                    while out.len() < i {
                        out.push(String::new());
                    }
                    out.push(text);
                }
            }
            (None, Some(serde_json::Value::Null)) | (None, None) => {}
            (None, Some(v)) => {
                let text = v.as_str().unwrap_or("").to_string();
                if !text.is_empty() {
                    out.push(text);
                }
            }
        }
    }

    out.retain(|s| !s.trim().is_empty());
    out
}

fn apply_i32_list_history(
    old_values: &[i32],
    changes: &serde_json::Value,
    allow_removal: bool,
) -> Vec<i32> {
    let old_as_str: Vec<String> = old_values.iter().map(|v| v.to_string()).collect();
    apply_string_list_history(&old_as_str, changes, allow_removal)
        .into_iter()
        .filter_map(|s| s.parse::<i32>().ok())
        .collect()
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


fn apply_regions_languages_change(
    change_node: &serde_json::Value,
) -> serde_json::Value {
    let new_csv = history_new_value(change_node).and_then(|v| v.as_str()).unwrap_or("");
    let new_values = parse_csv_ids(new_csv);
    serde_json::json!(new_values)
}

fn ring_set_scalar_field(
    entry: &mut serde_json::Value,
    key: &str,
    change_node: &serde_json::Value,
) {
    let Some(new_value) = history_new_value(change_node) else {
        return;
    };
    entry[key] = match new_value {
        serde_json::Value::Null => serde_json::json!(""),
        serde_json::Value::String(s) => serde_json::json!(s),
        serde_json::Value::Number(n) => serde_json::json!(n.to_string()),
        serde_json::Value::Bool(b) => serde_json::json!(b.to_string()),
        _ => serde_json::json!(new_value.to_string()),
    };
}

fn ring_get_or_create_layer(entry: &mut serde_json::Value, layer_index: usize) -> &mut serde_json::Value {
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

fn resolve_change_new_str(change: &serde_json::Value, key: &str) -> String {
    change
        .get(key)
        .and_then(|node| history_new_value(node))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn offsets_match(existing_val: &str, change_val: &str) -> bool {
    existing_val.is_empty() || change_val.is_empty() || existing_val == change_val
}

fn find_matching_ring_entry(
    rings: &[serde_json::Value],
    change: &serde_json::Value,
) -> Option<usize> {
    let change_layers = change.get("layers").and_then(|v| v.as_array())?;

    let change_offset = resolve_change_new_str(change, "offset_value");
    let change_offset_extra = resolve_change_new_str(change, "offset_extra_value");

    'outer: for (ring_idx, ring) in rings.iter().enumerate() {
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
                let change_val = cl
                    .get(field)
                    .and_then(|node| history_new_value(node))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

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
    let allow_removal = true;
    let mut removals: Vec<usize> = Vec::new();
    let mut additions: Vec<serde_json::Value> = Vec::new();
    let ring_index_by_id = |rings: &[serde_json::Value], id: i32| -> Option<usize> {
        rings.iter().position(|entry| entry.get("id").and_then(|v| v.as_i64()).map(|v| v as i32) == Some(id))
    };

    for change in changes {
        let id = change.get("id").and_then(|v| v.as_i64()).map(|v| v as i32);
        let resolved_idx = if let Some(id) = id {
            ring_index_by_id(&rings, id).ok_or_else(|| {
                AppError::BadRequest(format!("ring_codes id {} not found", id))
            })?
        } else {
            usize::MAX
        };
        if change.get("removed").and_then(|v| v.as_bool()).unwrap_or(false) {
            if allow_removal {
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
            }
            continue;
        }

        let merge_idx = if resolved_idx == usize::MAX {
            find_matching_ring_entry(&rings, change)
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
            additions
                .last_mut()
                .expect("addition just pushed")
        };

        for (history_key, target_key) in [
            ("offset_value", "offset_value"),
            ("offset_extra_value", "offset_extra_value"),
            ("sample_data_start", "sample_start"),
            ("comment", "comment"),
        ] {
            if let Some(node) = change.get(history_key) {
                ring_set_scalar_field(entry, target_key, node);
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
                        ring_set_scalar_field(layer, field, node);
                    }
                }
                for field in ["toolstamps", "mould_sids", "additional_moulds"] {
                    if let Some(node) = layer_change.get(field) {
                        let new_csv = history_new_value(node)
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
            "regions" | "languages" => {
                let updated = apply_regions_languages_change(value);
                resolved_obj.insert(key.clone(), updated);
            }
            "serial" | "edition" | "barcode" => {
                let old = json_str_vec(resolved_obj.get(key).unwrap_or(&serde_json::Value::Null));
                let mut updated = apply_string_list_history(&old, value, true);
                if matches!(key.as_str(), "serial" | "edition" | "barcode") {
                    dedup_case_insensitive(&mut updated);
                }
                resolved_obj.insert(key.clone(), serde_json::json!(updated));
            }
            "layerbreaks" => {
                let old: Vec<i32> = resolved_obj
                    .get(key)
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_i64().map(|x| x as i32)).collect())
                    .unwrap_or_default();
                let updated = apply_i32_list_history(&old, value, true);
                resolved_obj.insert(key.clone(), serde_json::json!(updated));
            }
            "ring_codes" => {
                let old = resolved_obj.get(key).cloned().unwrap_or_else(|| serde_json::json!([]));
                let updated = apply_ring_codes_history(&old, value)?;
                resolved_obj.insert(key.clone(), updated);
            }
            _ => {
                let Some(new_value) = history_new_value(value).cloned() else {
                    continue;
                };
                resolved_obj.insert(key.clone(), new_value);
            }
        }
    }

    Ok(resolved)
}

fn dedup_case_insensitive(values: &mut Vec<String>) {
    let mut seen = Vec::new();
    values.retain(|s| {
        if seen.iter().any(|existing: &String| existing.eq_ignore_ascii_case(s)) {
            false
        } else {
            seen.push(s.clone());
            true
        }
    });
}

async fn resolve_submission_data(
    pool: &PgPool,
    sub: &DiscSubmission,
    changes: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    if let Some(disc_id) = sub.target_disc_id {
        let detail = disc_service::get_disc_detail(pool, disc_id).await?;
        let db_snapshot = disc_service::build_snapshot_from_disc(&detail);
        resolve_submission_snapshot(&db_snapshot, changes)
    } else {
        resolve_submission_snapshot(&serde_json::json!({}), changes)
    }
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
/// submission as Approved, and return the resulting disc id.
///
/// Returns `None` if the submission was already processed by another
/// moderator (race condition).  The status is claimed atomically before
/// any disc mutations are performed.
pub async fn approve_submission(
    pool: &PgPool,
    sub: &DiscSubmission,
    changes: &serde_json::Value,
    is_sparse_changes: bool,
    reviewer_id: i32,
    review_comment: Option<&str>,
    archive_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> AppResult<Option<i32>> {
    let mut effective_data = if is_sparse_changes {
        resolve_submission_data(pool, sub, changes).await?
    } else {
        changes.clone()
    };

    if let Some(obj) = effective_data.as_object_mut() {
        if sub.target_disc_id.is_none() {
            obj.insert("status".to_string(), serde_json::json!("Unverified"));
        } else if sub.submission_type == SubmissionType::Disc {
            obj.insert("status".to_string(), serde_json::json!("Verified"));
        }
    }
    let stored_data = if is_sparse_changes {
        changes.clone()
    } else {
        effective_data.clone()
    };

    // Atomically claim the submission by setting status = 'Approved'
    // only when it is still 'Pending'.  If another moderator already
    // processed it, rows_affected will be 0.
    let normalized_review_comment: Option<String> = review_comment
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let claim = sqlx::query(
        "UPDATE disc_submissions SET status = 'Approved', reviewer_id = $1,
         review_comment = $2, reviewed_at = NOW(), changes = $3
         WHERE id = $4 AND status = 'Pending'",
    )
    .bind(reviewer_id)
    .bind(normalized_review_comment.as_deref())
    .bind(&stored_data)
    .bind(sub.id)
    .execute(pool)
    .await?;

    if claim.rows_affected() == 0 {
        return Ok(None);
    }

    let disc_id = if let Some(existing_id) = sub.target_disc_id {
        disc_service::update_disc(pool, existing_id, &effective_data).await?;

        if sub.submission_type == SubmissionType::Disc {
            sqlx::query(
                "INSERT INTO disc_dumpers (disc_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            )
            .bind(existing_id)
            .bind(sub.submitter_id)
            .execute(pool)
            .await?;
        }

        existing_id
    } else {
        let new_id = disc_service::create_disc_from_submission(
            pool,
            &effective_data,
            sub.submitter_id,
        )
        .await?;

        sqlx::query(
            "UPDATE disc_submissions SET target_disc_id = $1 WHERE id = $2",
        )
        .bind(new_id)
        .bind(sub.id)
        .execute(pool)
        .await?;

        new_id
    };

    let system_code: Option<String> = sqlx::query_scalar(
        "SELECT system_code FROM discs WHERE id = $1",
    )
    .bind(disc_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if let Some(code) = system_code {
        let _ = archive_tx.send(code);
    }

    Ok(Some(disc_id))
}

pub async fn get_submission(pool: &PgPool, id: i32) -> AppResult<DiscSubmission> {
    sqlx::query_as("SELECT * FROM disc_submissions WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn list_submissions(
    pool: &PgPool,
    user_id_filter: Option<i32>,
    disc_id_filter: Option<i32>,
    restrict_to_public_statuses: bool,
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
    if status_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("ds.status::text = ${idx}"));
    }
    if type_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("ds.submission_type::text = ${idx}"));
    }
    if system_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("COALESCE(d.system_code, ds.changes->'system_code'->>'new') = ${idx}"));
    }
    if submitter_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("u.username = ${idx}"));
    }

    let sort_col = match sort_column {
        "date"      => "ds.created_at",
        "title"     => "LOWER(COALESCE(d.title, ds.changes->'title'->>'new', 'Untitled'))",
        "system"    => "LOWER(COALESCE(s.manufacturer, '')), COALESCE(s.manufacturer, ''), \
                        LOWER(COALESCE(s.name, COALESCE(d.system_code, ds.changes->'system_code'->>'new', ''))), \
                        COALESCE(s.name, COALESCE(d.system_code, ds.changes->'system_code'->>'new', ''))",
        "submitter" => "LOWER(u.username)",
        "reviewer"  => "LOWER(COALESCE(ur.username, ''))",
        "type"      => "ds.submission_type",
        "status"    => "ds.status",
        _           => "ds.created_at",
    };
    let sort_dir = if sort_order == "asc" { "ASC" } else { "DESC" };

    let sql = format!(
        "SELECT ds.id, ds.submission_type,
                COALESCE(d.title, ds.changes->'title'->>'new', 'Untitled') AS title,
                COALESCE(d.system_code, ds.changes->'system_code'->>'new', '') AS system_code,
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
             ON s.code = COALESCE(d.system_code, ds.changes->'system_code'->>'new')
         WHERE {}
         ORDER BY {sort_col} {sort_dir}
         LIMIT {page_size} OFFSET {offset}",
        conditions.join(" AND ")
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
    if let Some(sub_type) = type_filter {
        if !sub_type.is_empty() {
            query = query.bind(sub_type.to_string());
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
    if status_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("ds.status::text = ${idx}"));
    }
    if type_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("ds.submission_type::text = ${idx}"));
    }
    if system_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("COALESCE(d.system_code, ds.changes->'system_code'->>'new') = ${idx}"));
    }
    if submitter_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("u.username = ${idx}"));
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
    if let Some(sub_type) = type_filter {
        if !sub_type.is_empty() {
            query = query.bind(sub_type.to_string());
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
                "comment": { "new": "updated" }
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
                "removed": true,
                "comment": { "new": "nope" }
            }
        ]);

        let err = apply_ring_codes_history(&old, &changes).unwrap_err();
        match err {
            AppError::BadRequest(msg) => assert!(msg.contains("requires entry id")),
            other => panic!("unexpected error: {other:?}"),
        }
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
                "mastering_code": { "new": "ABCD" },
                "mastering_sid": { "new": "SID-1" },
                "toolstamps": { "new": "TS-B" },
                "mould_sids": { "new": "MS-2" },
                "additional_moulds": { "new": "AM-Y" }
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
                "mastering_code": { "new": "DIFFERENT" },
                "mastering_sid": { "new": "SID-1" },
                "toolstamps": { "new": "TS-B" }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 2, "should add new entry when mastering_code differs");
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
                "mastering_code": { "new": "X" },
                "mastering_sid": { "new": "" },
                "toolstamps": { "new": "B, C" },
                "mould_sids": { "new": "m1, M2" }
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
            "offset_value": { "new": "" },
            "layers": [{
                "index": 0,
                "mastering_code": { "new": "Z" },
                "mastering_sid": { "new": "" },
                "toolstamps": { "new": "T2" }
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
            "offset_value": { "new": "99" },
            "layers": [{
                "index": 0,
                "mastering_code": { "new": "Z" },
                "mastering_sid": { "new": "" },
                "toolstamps": { "new": "T1" }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 2, "different non-empty offsets should not merge");
    }
}
