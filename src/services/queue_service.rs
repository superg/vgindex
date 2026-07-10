use sqlx::{PgConnection, PgPool};
use std::cmp::Ordering;
use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::hex_case::canonicalize_disc_snapshot_hex_fields;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManualRetargetOutcome {
    Retargeted,
    TargetNotFound,
    TargetDisabled,
    Unchanged,
    SubmissionChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingTargetUpdateOutcome {
    Updated,
    SubmissionChanged,
}

#[derive(Debug, Clone)]
pub struct SubmissionCreation {
    pub submission: DiscSubmission,
    pub created: bool,
}

#[derive(Debug, Clone)]
pub enum DirectSubmissionOutcome {
    Approved(i32),
    Existing(DiscSubmission),
    Conflicts(Vec<ApprovalConflict>),
}

fn normalize_submission_token(token: Option<&str>) -> AppResult<Option<String>> {
    let token = token.map(str::trim).filter(|token| !token.is_empty());
    let Some(token) = token else {
        return Ok(None);
    };

    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(
            "Submission token is invalid. Reload the form and try again.".into(),
        ));
    }

    Ok(Some(token.to_ascii_lowercase()))
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
    submission_token: Option<&str>,
    submission_fingerprint: Option<&str>,
) -> AppResult<SubmissionCreation> {
    let mut tx = pool.begin().await?;
    let creation = match create_submission_on(
        &mut tx,
        sub_type,
        submitter_id,
        target_disc_id,
        &changes,
        submission_comment,
        dump_log,
        extra_upload_url,
        submission_token,
        submission_fingerprint,
    )
    .await
    {
        Ok(creation) => creation,
        Err(err) => {
            tx.rollback().await?;
            return Err(err);
        }
    };
    tx.commit().await?;
    Ok(creation)
}

#[allow(clippy::too_many_arguments)]
pub async fn submit_draft_submission(
    pool: &PgPool,
    submission_id: i32,
    submitter_id: i32,
    target_disc_id: Option<i32>,
    changes: serde_json::Value,
    submission_comment: Option<&str>,
    dump_log: Option<&str>,
    extra_upload_url: Option<&str>,
    submission_token: Option<&str>,
    submission_fingerprint: Option<&str>,
) -> AppResult<Option<DiscSubmission>> {
    let normalized_submission_comment = submission_comment
        .map(normalize_newlines)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let submission_token = normalize_submission_token(submission_token)?;
    let submission_fingerprint = if submission_token.is_some() {
        Some(
            normalize_submission_token(submission_fingerprint)?.ok_or_else(|| {
                AppError::BadRequest(
                    "Submission fingerprint is missing. Reload the form and try again.".into(),
                )
            })?,
        )
    } else {
        None
    };

    Ok(sqlx::query_as(
        "UPDATE disc_submissions
         SET submission_comment = $1, target_disc_id = $2, changes = $3,
             dump_log = $4, extra_upload_url = $5, status = 'Pending',
             submission_token = $6, submission_fingerprint = $7
         WHERE id = $8 AND submitter_id = $9
           AND submission_type = 'Disc' AND status = 'Draft'
         RETURNING *",
    )
    .bind(normalized_submission_comment.as_deref())
    .bind(target_disc_id)
    .bind(changes)
    .bind(dump_log)
    .bind(extra_upload_url)
    .bind(submission_token.as_deref())
    .bind(submission_fingerprint.as_deref())
    .bind(submission_id)
    .bind(submitter_id)
    .fetch_optional(pool)
    .await?)
}

async fn create_submission_on(
    conn: &mut PgConnection,
    sub_type: SubmissionType,
    submitter_id: i32,
    target_disc_id: Option<i32>,
    changes: &serde_json::Value,
    submission_comment: Option<&str>,
    dump_log: Option<&str>,
    extra_upload_url: Option<&str>,
    submission_token: Option<&str>,
    submission_fingerprint: Option<&str>,
) -> AppResult<SubmissionCreation> {
    let normalized_submission_comment: Option<String> = submission_comment
        .map(normalize_newlines)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let submission_token = normalize_submission_token(submission_token)?;
    let submission_fingerprint = if submission_token.is_some() {
        normalize_submission_token(submission_fingerprint)?.ok_or_else(|| {
            AppError::BadRequest(
                "Submission fingerprint is missing. Reload the form and try again.".into(),
            )
        })?
    } else {
        String::new()
    };
    let inserted: Option<DiscSubmission> = sqlx::query_as(
        "INSERT INTO disc_submissions (submission_type, submitter_id, submission_comment, target_disc_id, changes, dump_log, extra_upload_url, submission_token, submission_fingerprint)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (submission_token) WHERE submission_token IS NOT NULL DO NOTHING
         RETURNING *"
    )
    .bind(sub_type)
    .bind(submitter_id)
    .bind(normalized_submission_comment.as_deref())
    .bind(target_disc_id)
    .bind(changes)
    .bind(dump_log)
    .bind(extra_upload_url)
    .bind(submission_token.as_deref())
    .bind(submission_token.as_ref().map(|_| &submission_fingerprint))
    .fetch_optional(&mut *conn)
    .await?;

    if let Some(submission) = inserted {
        return Ok(SubmissionCreation {
            submission,
            created: true,
        });
    }

    let token = submission_token.as_deref().ok_or_else(|| {
        AppError::Internal("submission insert returned no row without an idempotency token".into())
    })?;
    let submission: DiscSubmission =
        sqlx::query_as("SELECT * FROM disc_submissions WHERE submission_token = $1")
            .bind(token)
            .fetch_one(&mut *conn)
            .await?;

    let same_request = submission.submission_type == sub_type
        && submission.submitter_id == submitter_id
        && submission.submission_fingerprint.as_deref() == Some(&submission_fingerprint);
    if !same_request {
        return Err(AppError::BadRequest(
            "This form was already submitted with different contents. Reload it and try again."
                .into(),
        ));
    }

    Ok(SubmissionCreation {
        submission,
        created: false,
    })
}

fn direct_outcome_for_existing(submission: DiscSubmission) -> DirectSubmissionOutcome {
    if submission.status == SubmissionStatus::Approved {
        if let Some(disc_id) = submission.target_disc_id {
            return DirectSubmissionOutcome::Approved(disc_id);
        }
    }
    DirectSubmissionOutcome::Existing(submission)
}

#[allow(clippy::too_many_arguments)]
pub async fn create_and_approve_submission(
    pool: &PgPool,
    sub_type: SubmissionType,
    submitter_id: i32,
    target_disc_id: Option<i32>,
    changes: serde_json::Value,
    submission_comment: Option<&str>,
    dump_log: Option<&str>,
    extra_upload_url: Option<&str>,
    submission_token: Option<&str>,
    submission_fingerprint: Option<&str>,
    reviewer_id: i32,
) -> AppResult<DirectSubmissionOutcome> {
    let mut tx = pool.begin().await?;
    acquire_approval_lock(&mut tx).await?;

    let creation = match create_submission_on(
        &mut tx,
        sub_type,
        submitter_id,
        target_disc_id,
        &changes,
        submission_comment,
        dump_log,
        extra_upload_url,
        submission_token,
        submission_fingerprint,
    )
    .await
    {
        Ok(creation) => creation,
        Err(err) => {
            tx.rollback().await?;
            return Err(err);
        }
    };
    let created = creation.created;
    let submission = creation.submission;

    if !created && submission.status != SubmissionStatus::Pending {
        tx.rollback().await?;
        return Ok(direct_outcome_for_existing(submission));
    }

    let approval = approve_submission_on(
        pool,
        &mut tx,
        &submission,
        &submission.changes,
        reviewer_id,
        None,
        None,
    )
    .await;

    match approval {
        Ok(ApprovalOutcome::Approved(disc_id)) => {
            tx.commit().await?;
            Ok(DirectSubmissionOutcome::Approved(disc_id))
        }
        Ok(ApprovalOutcome::Conflicts(conflicts)) => {
            tx.rollback().await?;
            Ok(DirectSubmissionOutcome::Conflicts(conflicts))
        }
        Ok(ApprovalOutcome::AlreadyProcessed | ApprovalOutcome::StaleDiscState) => {
            tx.rollback().await?;
            let existing = get_submission(pool, submission.id).await?;
            Ok(direct_outcome_for_existing(existing))
        }
        Err(err) => {
            tx.rollback().await?;
            Err(err)
        }
    }
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

pub async fn find_matching_disc(pool: &PgPool, files_xml: &str) -> AppResult<Option<i32>> {
    find_matching_disc_excluding(pool, files_xml, None).await
}

const DAT_MATCH_CANDIDATES_SQL: &str = "SELECT DISTINCT f.disc_id
         FROM files f
         JOIN discs d ON d.id = f.disc_id
         WHERE LOWER(f.sha1) = LOWER($1)
           AND d.status <> 'Disabled'
           AND ($2::INT IS NULL OR f.disc_id <> $2)";

async fn find_matching_disc_excluding(
    pool: &PgPool,
    files_xml: &str,
    exclude_disc_id: Option<i32>,
) -> AppResult<Option<i32>> {
    let submitted = parse_rom_entries(files_xml);
    if submitted.is_empty() {
        return Ok(None);
    }

    let candidates: Vec<i32> = sqlx::query_scalar(DAT_MATCH_CANDIDATES_SQL)
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
) -> AppResult<Option<i32>> {
    find_matching_disc_by_universal_hash_excluding(pool, universal_hash, None).await
}

const UNIVERSAL_HASH_MATCH_SQL: &str = "SELECT id
         FROM discs
         WHERE universal_hash = $1
           AND status <> 'Disabled'
           AND ($2::INT IS NULL OR id <> $2)
         ORDER BY id
         LIMIT 1";

async fn find_matching_disc_by_universal_hash_excluding(
    pool: &PgPool,
    universal_hash: &str,
    exclude_disc_id: Option<i32>,
) -> AppResult<Option<i32>> {
    let Some(hash_bytes) = universal_hash_bytes_for_matching(Some(universal_hash)) else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar(UNIVERSAL_HASH_MATCH_SQL)
        .bind(hash_bytes)
        .bind(exclude_disc_id)
        .fetch_optional(pool)
        .await?)
}

fn resolve_exact_disc_match(
    dat_match: Option<i32>,
    universal_hash_match: Option<i32>,
) -> Option<i32> {
    match (dat_match, universal_hash_match) {
        (None, None) => None,
        (Some(disc_id), None) | (None, Some(disc_id)) => Some(disc_id),
        (Some(dat_disc_id), Some(universal_hash_disc_id))
            if dat_disc_id == universal_hash_disc_id =>
        {
            Some(dat_disc_id)
        }
        (Some(_), Some(_)) => None,
    }
}

pub(crate) async fn find_unambiguous_exact_disc_match(
    pool: &PgPool,
    files_xml: Option<&str>,
    universal_hash: Option<&str>,
) -> AppResult<Option<i32>> {
    let dat_match = match files_xml.map(str::trim).filter(|value| !value.is_empty()) {
        Some(files_xml) => find_matching_disc(pool, files_xml).await?,
        None => None,
    };
    let universal_hash_match = match universal_hash
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(universal_hash) => find_matching_disc_by_universal_hash(pool, universal_hash).await?,
        None => None,
    };
    Ok(resolve_exact_disc_match(dat_match, universal_hash_match))
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
    out.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    out.dedup();
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
        if !combined.contains(&val) {
            combined.push(val);
        }
    }
    combined.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    combined.join(", ")
}

fn ring_numeric_values_match(existing_val: &str, change_val: &str, require_exact: bool) -> bool {
    existing_val == change_val
        || (!require_exact && (existing_val.is_empty() || change_val.is_empty()))
}

fn ring_layer_has_mastering_identity(layer: &serde_json::Value) -> bool {
    ["mastering_code", "mastering_sid"]
        .iter()
        .any(|field| !layer[*field].as_str().unwrap_or("").is_empty())
}

fn ring_layers_have_mastering_identity(layers: Option<&Vec<serde_json::Value>>) -> bool {
    layers.is_some_and(|layers| layers.iter().any(ring_layer_has_mastering_identity))
}

fn ring_change_layers_have_mastering_identity(layers: &[serde_json::Value]) -> bool {
    layers.iter().any(|layer| {
        ["mastering_code", "mastering_sid"]
            .iter()
            .any(|field| !operation_new_str(layer, field).is_empty())
    })
}

fn find_matching_ring_entry(
    rings: &[serde_json::Value],
    change: &serde_json::Value,
    removed_ring_ids: &std::collections::HashSet<i32>,
) -> Option<usize> {
    let change_layers = change.get("layers").and_then(|v| v.as_array())?;

    let change_offset = operation_new_str(change, "offset_value");
    let change_offset_extra = operation_new_str(change, "offset_extra_value");
    let change_sample_start = operation_new_str(change, "sample_data_start");
    let change_comment = operation_new_str(change, "comment");
    let change_has_mastering_identity = ring_change_layers_have_mastering_identity(change_layers);

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
        let ring_sample_start = ring["sample_start"].as_str().unwrap_or("");
        let ring_comment = ring["comment"].as_str().unwrap_or("");
        let ring_layers = ring["layers"].as_array();
        let require_exact_numeric_match =
            !change_has_mastering_identity && !ring_layers_have_mastering_identity(ring_layers);

        if !ring_numeric_values_match(ring_offset, &change_offset, require_exact_numeric_match)
            || !ring_numeric_values_match(
                ring_offset_extra,
                &change_offset_extra,
                require_exact_numeric_match,
            )
            || !ring_numeric_values_match(
                ring_sample_start,
                &change_sample_start,
                require_exact_numeric_match,
            )
            || ring_comment != change_comment
        {
            continue;
        }

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

        if ring_layers.is_some_and(|layers| {
            layers.iter().enumerate().any(|(layer_idx, layer)| {
                ring_layer_has_mastering_identity(layer)
                    && !change_layers.iter().any(|change_layer| {
                        change_layer.get("index").and_then(|v| v.as_u64()) == Some(layer_idx as u64)
                    })
            })
        }) {
            continue;
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
        canonicalize_disc_snapshot_hex_fields(&mut resolved);
        return Ok(resolved);
    };
    let Some(resolved_obj) = resolved.as_object_mut() else {
        canonicalize_disc_snapshot_hex_fields(&mut resolved);
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

    canonicalize_disc_snapshot_hex_fields(&mut resolved);
    Ok(resolved)
}

pub fn resolve_submission_snapshot_for_submission(
    db_snapshot: &serde_json::Value,
    sub: &DiscSubmission,
) -> AppResult<serde_json::Value> {
    resolve_submission_snapshot(db_snapshot, &sub.changes)
}

struct ApprovalResolution {
    changes: serde_json::Value,
    effective_data: serde_json::Value,
    target_status: Option<DiscStatus>,
}

fn preserve_submitted_status_change(
    submission_type: SubmissionType,
    submitted_changes: &serde_json::Value,
    reviewed_changes: &serde_json::Value,
) -> AppResult<serde_json::Value> {
    let mut approval_changes = reviewed_changes.clone();
    if submission_type != SubmissionType::Disc || change_set_contains(reviewed_changes, "status") {
        return Ok(approval_changes);
    }
    let Some(status_change) = submitted_changes.get("status") else {
        return Ok(approval_changes);
    };
    let changes_obj = approval_changes
        .as_object_mut()
        .ok_or_else(|| AppError::BadRequest("submission changes must be a JSON object".into()))?;
    changes_obj.insert("status".to_string(), status_change.clone());
    Ok(approval_changes)
}

fn automatic_disc_status_change(target_status: Option<DiscStatus>) -> Option<serde_json::Value> {
    match target_status {
        None => Some(serde_json::json!({ "add": { "new": "Unverified" } })),
        Some(status @ (DiscStatus::Unverified | DiscStatus::Questionable)) => {
            Some(serde_json::json!({
                "modify": {
                    "old": status.to_string(),
                    "new": "Verified"
                }
            }))
        }
        Some(DiscStatus::Verified | DiscStatus::Disabled) => None,
    }
}

fn resolve_approval_from_snapshot(
    submission_type: SubmissionType,
    target_status: Option<DiscStatus>,
    db_snapshot: &serde_json::Value,
    changes: &serde_json::Value,
) -> AppResult<ApprovalResolution> {
    let mut approved_changes = changes.clone();
    if submission_type == SubmissionType::Disc && !change_set_contains(changes, "status") {
        if let Some(status_change) = automatic_disc_status_change(target_status) {
            let changes_obj = approved_changes.as_object_mut().ok_or_else(|| {
                AppError::BadRequest("submission changes must be a JSON object".into())
            })?;
            changes_obj.insert("status".to_string(), status_change);
        }
    }
    let effective_data = resolve_submission_snapshot(db_snapshot, &approved_changes)?;
    Ok(ApprovalResolution {
        changes: approved_changes,
        effective_data,
        target_status,
    })
}

async fn approval_resolution(
    pool: &PgPool,
    submission_type: SubmissionType,
    target_disc_id: Option<i32>,
    changes: &serde_json::Value,
) -> AppResult<ApprovalResolution> {
    let (db_snapshot, target_status) = if let Some(disc_id) = target_disc_id {
        let detail = disc_service::get_disc_detail(pool, disc_id).await?;
        let status = detail.disc.status;
        (
            disc_service::build_snapshot_from_disc(&detail),
            Some(status),
        )
    } else {
        (serde_json::json!({}), None)
    };
    resolve_approval_from_snapshot(submission_type, target_status, &db_snapshot, changes)
}

pub async fn find_approval_conflicts(
    pool: &PgPool,
    submission_type: SubmissionType,
    target_disc_id: Option<i32>,
    changes: &serde_json::Value,
) -> AppResult<Vec<ApprovalConflict>> {
    let resolution = approval_resolution(pool, submission_type, target_disc_id, changes).await?;
    find_approval_conflicts_for_effective_data(
        pool,
        submission_type,
        target_disc_id,
        resolution.target_status,
        &resolution.changes,
        &resolution.effective_data,
    )
    .await
}

async fn find_approval_conflicts_for_effective_data(
    pool: &PgPool,
    submission_type: SubmissionType,
    target_disc_id: Option<i32>,
    target_status: Option<DiscStatus>,
    changes: &serde_json::Value,
    effective_data: &serde_json::Value,
) -> AppResult<Vec<ApprovalConflict>> {
    if disabled_verification_target(submission_type, target_status) {
        let disc_id = target_disc_id.ok_or_else(|| {
            AppError::Internal("disabled verification target is missing a disc ID".into())
        })?;
        return Ok(vec![ApprovalConflict {
            text: "Verification target is disabled:".to_string(),
            disc_id,
            disc_title: fetch_approval_conflict_disc_title(pool, disc_id).await?,
        }]);
    }

    if !effective_disc_is_active(effective_data) {
        return Ok(Vec::new());
    }

    let mut conflicts = Vec::new();

    if effective_disc_is_archive_eligible(effective_data) {
        if let Some(conflict) =
            find_generated_name_conflict(pool, effective_data, target_disc_id).await?
        {
            conflicts.push(conflict);
        }
    }

    if let Some(files_xml) = dat_hash_conflict_input(changes, effective_data) {
        if let Some(disc_id) = find_matching_disc_excluding(pool, files_xml, target_disc_id).await?
        {
            conflicts.push(ApprovalConflict {
                text: "DAT hashes already exist:".to_string(),
                disc_id,
                disc_title: fetch_approval_conflict_disc_title(pool, disc_id).await?,
            });
        }
    }

    if let Some(universal_hash) = universal_hash_conflict_input(changes, effective_data) {
        if let Some(disc_id) =
            find_matching_disc_by_universal_hash_excluding(pool, universal_hash, target_disc_id)
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

fn disabled_verification_target(
    submission_type: SubmissionType,
    target_status: Option<DiscStatus>,
) -> bool {
    submission_type == SubmissionType::Disc && target_status == Some(DiscStatus::Disabled)
}

fn effective_disc_is_active(effective_data: &serde_json::Value) -> bool {
    effective_data["status"].as_str() != Some("Disabled")
}

fn effective_disc_is_archive_eligible(effective_data: &serde_json::Value) -> bool {
    !matches!(
        effective_data["status"].as_str(),
        Some("Disabled" | "Questionable")
    )
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

    let candidates: Vec<DuplicateNameDiscRow> =
        sqlx::query_as(GENERATED_NAME_CONFLICT_CANDIDATES_SQL)
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

const GENERATED_NAME_CONFLICT_CANDIDATES_SQL: &str = "\
SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix,
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
           AND d.status NOT IN ('Disabled', 'Questionable')
           AND ($2::INT IS NULL OR d.id <> $2)
         ORDER BY d.id";

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

/// Atomically return an add-disc submission to its submitter. Returns `true`
/// when this reviewer won the Pending-state claim and `false` when another
/// reviewer processed the submission first.
pub async fn draft_submission(
    pool: &PgPool,
    id: i32,
    reviewer_id: i32,
    review_comment: &str,
) -> AppResult<bool> {
    let normalized_review_comment = normalize_newlines(review_comment).trim().to_string();
    if normalized_review_comment.is_empty() {
        return Err(AppError::BadRequest(
            "Review Comment: required when returning a submission to Draft".into(),
        ));
    }

    let result = sqlx::query(
        "UPDATE disc_submissions SET status = 'Draft', reviewer_id = $1,
         review_comment = $2, reviewed_at = NOW()
         WHERE id = $3 AND submission_type = 'Disc' AND status = 'Pending'",
    )
    .bind(reviewer_id)
    .bind(&normalized_review_comment)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

async fn acquire_approval_lock(conn: &mut PgConnection) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(APPROVAL_CONFLICT_LOCK_KEY)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn update_pending_submission_target(
    conn: &mut PgConnection,
    submission_id: i32,
    expected_target_disc_id: Option<i32>,
    target_disc_id: Option<i32>,
) -> AppResult<PendingTargetUpdateOutcome> {
    let result = sqlx::query(
        "UPDATE disc_submissions SET target_disc_id = $1
         WHERE id = $2 AND submission_type = 'Disc' AND status = 'Pending'
           AND target_disc_id IS NOT DISTINCT FROM $3",
    )
    .bind(target_disc_id)
    .bind(submission_id)
    .bind(expected_target_disc_id)
    .execute(&mut *conn)
    .await?;

    Ok(if result.rows_affected() == 1 {
        PendingTargetUpdateOutcome::Updated
    } else {
        PendingTargetUpdateOutcome::SubmissionChanged
    })
}

pub(crate) async fn retarget_pending_submission(
    pool: &PgPool,
    submission_id: i32,
    expected_target_disc_id: i32,
    files_xml: Option<&str>,
    universal_hash: Option<&str>,
) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    acquire_approval_lock(&mut tx).await?;

    if find_unambiguous_exact_disc_match(pool, files_xml, universal_hash).await?
        != Some(expected_target_disc_id)
    {
        tx.rollback().await?;
        return Ok(false);
    }

    if update_pending_submission_target(&mut tx, submission_id, None, Some(expected_target_disc_id))
        .await?
        != PendingTargetUpdateOutcome::Updated
    {
        tx.rollback().await?;
        return Ok(false);
    }

    tx.commit().await?;
    Ok(true)
}

pub(crate) async fn manually_retarget_pending_submission(
    pool: &PgPool,
    submission_id: i32,
    expected_target_disc_id: Option<i32>,
    target_disc_id: Option<i32>,
) -> AppResult<ManualRetargetOutcome> {
    let mut tx = pool.begin().await?;
    acquire_approval_lock(&mut tx).await?;

    let current_target: Option<(Option<i32>,)> = sqlx::query_as(
        "SELECT target_disc_id FROM disc_submissions
         WHERE id = $1 AND submission_type = 'Disc' AND status = 'Pending'
           AND target_disc_id IS NOT DISTINCT FROM $2
         FOR UPDATE",
    )
    .bind(submission_id)
    .bind(expected_target_disc_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((current_target,)) = current_target else {
        tx.rollback().await?;
        return Ok(ManualRetargetOutcome::SubmissionChanged);
    };
    if current_target == target_disc_id {
        tx.rollback().await?;
        return Ok(ManualRetargetOutcome::Unchanged);
    }

    if let Some(target_disc_id) = target_disc_id {
        let target_status: Option<DiscStatus> =
            sqlx::query_scalar("SELECT status FROM discs WHERE id = $1 FOR KEY SHARE")
                .bind(target_disc_id)
                .fetch_optional(&mut *tx)
                .await?;
        match target_status {
            None => {
                tx.rollback().await?;
                return Ok(ManualRetargetOutcome::TargetNotFound);
            }
            Some(DiscStatus::Disabled) => {
                tx.rollback().await?;
                return Ok(ManualRetargetOutcome::TargetDisabled);
            }
            Some(_) => {}
        }
    }

    let outcome = update_pending_submission_target(
        &mut tx,
        submission_id,
        expected_target_disc_id,
        target_disc_id,
    )
    .await?;
    match outcome {
        PendingTargetUpdateOutcome::Updated => {
            tx.commit().await?;
            Ok(ManualRetargetOutcome::Retargeted)
        }
        PendingTargetUpdateOutcome::SubmissionChanged => {
            tx.rollback().await?;
            Ok(ManualRetargetOutcome::SubmissionChanged)
        }
    }
}

/// Apply approval to a submission atomically: update/create the disc, mark the
/// submission as Approved, and commit every related mutation together.
pub async fn approve_submission(
    pool: &PgPool,
    sub: &DiscSubmission,
    changes: &serde_json::Value,
    reviewer_id: i32,
    review_comment: Option<&str>,
    expected_review_base_hash: Option<&str>,
) -> AppResult<ApprovalOutcome> {
    let mut tx = pool.begin().await?;
    acquire_approval_lock(&mut tx).await?;
    let outcome = approve_submission_on(
        pool,
        &mut tx,
        sub,
        changes,
        reviewer_id,
        review_comment,
        expected_review_base_hash,
    )
    .await;

    match outcome {
        Ok(outcome @ ApprovalOutcome::Approved(_)) => {
            tx.commit().await?;
            Ok(outcome)
        }
        Ok(outcome) => {
            tx.rollback().await?;
            Ok(outcome)
        }
        Err(err) => {
            tx.rollback().await?;
            Err(err)
        }
    }
}

async fn approve_submission_on(
    pool: &PgPool,
    conn: &mut PgConnection,
    sub: &DiscSubmission,
    changes: &serde_json::Value,
    reviewer_id: i32,
    review_comment: Option<&str>,
    expected_review_base_hash: Option<&str>,
) -> AppResult<ApprovalOutcome> {
    let current_state: Option<(SubmissionStatus, Option<i32>)> =
        sqlx::query_as("SELECT status, target_disc_id FROM disc_submissions WHERE id = $1")
            .bind(sub.id)
            .fetch_optional(&mut *conn)
            .await?;
    if current_state != Some((SubmissionStatus::Pending, sub.target_disc_id)) {
        return Ok(ApprovalOutcome::AlreadyProcessed);
    }

    if let Some(disc_id) = sub.target_disc_id {
        let current_hash = current_disc_snapshot_hash(pool, disc_id).await?;
        if let Some(outcome) =
            stale_review_approval_outcome(expected_review_base_hash, &current_hash)
        {
            return Ok(outcome);
        }
    }

    let approval_changes =
        preserve_submitted_status_change(sub.submission_type, &sub.changes, changes)?;
    let resolution = approval_resolution(
        pool,
        sub.submission_type,
        sub.target_disc_id,
        &approval_changes,
    )
    .await?;
    let conflicts = find_approval_conflicts_for_effective_data(
        pool,
        sub.submission_type,
        sub.target_disc_id,
        resolution.target_status,
        &resolution.changes,
        &resolution.effective_data,
    )
    .await?;
    if !conflicts.is_empty() {
        return Ok(ApprovalOutcome::Conflicts(conflicts));
    }

    let previous_system_code: Option<String> = if let Some(existing_id) = sub.target_disc_id {
        sqlx::query_scalar("SELECT system_code FROM discs WHERE id = $1")
            .bind(existing_id)
            .fetch_optional(&mut *conn)
            .await?
    } else {
        None
    };
    let stored_data = resolution.changes;
    let effective_data = resolution.effective_data;

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
         WHERE id = $4 AND status = 'Pending'
           AND target_disc_id IS NOT DISTINCT FROM $5",
    )
    .bind(reviewer_id)
    .bind(normalized_review_comment.as_deref())
    .bind(&stored_data)
    .bind(sub.id)
    .bind(sub.target_disc_id)
    .execute(&mut *conn)
    .await?;

    if claim.rows_affected() == 0 {
        return Ok(ApprovalOutcome::AlreadyProcessed);
    }

    let disc_id = if let Some(existing_id) = sub.target_disc_id {
        disc_service::update_disc(&mut *conn, existing_id, &effective_data).await?;

        if sub.submission_type == SubmissionType::Disc {
            sqlx::query(
                "INSERT INTO disc_dumpers (disc_id, user_id, position)
                 VALUES ($1, $2, COALESCE((SELECT MAX(position) + 1 FROM disc_dumpers WHERE disc_id = $1), 0))
                 ON CONFLICT DO NOTHING",
            )
            .bind(existing_id)
            .bind(sub.submitter_id)
            .execute(&mut *conn)
            .await?;
        }

        existing_id
    } else {
        let new_id = disc_service::create_disc_from_submission(
            &mut *conn,
            &effective_data,
            sub.submitter_id,
        )
        .await?;

        sqlx::query("UPDATE disc_submissions SET target_disc_id = $1 WHERE id = $2")
            .bind(new_id)
            .bind(sub.id)
            .execute(&mut *conn)
            .await?;

        new_id
    };

    let system_code: Option<String> =
        sqlx::query_scalar("SELECT system_code FROM discs WHERE id = $1")
            .bind(disc_id)
            .fetch_optional(&mut *conn)
            .await?;

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
        archive_service::mark_system_archives_dirty_on(&mut *conn, &code).await?;
    }

    Ok(ApprovalOutcome::Approved(disc_id))
}

pub async fn get_submission(pool: &PgPool, id: i32) -> AppResult<DiscSubmission> {
    sqlx::query_as("SELECT * FROM disc_submissions WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

const UNPROCESSED_SUBMISSION_SQL: &str = "ds.status IN ('Pending', 'Draft')";
const SUBMISSION_LIST_SYSTEM_CODE_SQL: &str = "CASE WHEN ds.status = 'Draft' THEN
         COALESCE(ds.changes->'system_code'->'add'->>'new',
                  ds.changes->'system_code'->'modify'->>'new', '')
     ELSE COALESCE(d.system_code,
                  ds.changes->'system_code'->'add'->>'new',
                  ds.changes->'system_code'->'modify'->>'new', '') END";

fn submission_display_kind_sql() -> String {
    format!(
        "CASE \
         WHEN ds.submission_type = 'Edit' THEN 'Edit' \
         WHEN NOT ({UNPROCESSED_SUBMISSION_SQL}) THEN 'Disc' \
         WHEN ds.target_disc_id IS NULL THEN 'New Disc' \
         ELSE 'Verification' \
         END"
    )
}

fn submission_type_filter_condition(type_filter: Option<&str>) -> Option<String> {
    match type_filter.unwrap_or_default() {
        "Edit" => Some("ds.submission_type = 'Edit'".to_string()),
        "New Disc" => Some(format!(
            "ds.submission_type = 'Disc' AND {UNPROCESSED_SUBMISSION_SQL} AND ds.target_disc_id IS NULL"
        )),
        "Verification" => Some(format!(
            "ds.submission_type = 'Disc' AND (NOT ({UNPROCESSED_SUBMISSION_SQL}) OR ds.target_disc_id IS NOT NULL)"
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
    viewer_id: i32,
    can_view_all_drafts: bool,
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
    let mut draft_owner_param = None;
    if !can_view_all_drafts {
        idx += 1;
        draft_owner_param = Some(idx);
        conditions.push(format!(
            "(ds.status <> 'Draft' OR ds.submitter_id = ${idx})"
        ));
    }
    if hide_disabled_disc_targets {
        conditions.push(match draft_owner_param {
            Some(viewer_idx) => format!(
                "(d.id IS NULL OR d.status <> 'Disabled' OR \
                 (ds.status = 'Draft' AND ds.submitter_id = ${viewer_idx}))"
            ),
            None => "(d.id IS NULL OR d.status <> 'Disabled')".to_string(),
        });
    }
    if let Some(status) = status_filter.filter(|status| !status.is_empty()) {
        if status == "Pending and Draft" {
            conditions.push("ds.status IN ('Pending', 'Draft')".to_string());
        } else {
            idx += 1;
            conditions.push(format!("ds.status::text = ${idx}"));
        }
    }
    if let Some(condition) = submission_type_filter_condition(type_filter) {
        conditions.push(condition);
    }
    if system_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("{SUBMISSION_LIST_SYSTEM_CODE_SQL} = ${idx}"));
    }
    if submitter_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("LOWER(u.username) = LOWER(${idx})"));
    }

    let title_expr = "COALESCE(NULLIF(ds.changes->'title'->'add'->>'new', ''),
                               NULLIF(ds.changes->'title'->'modify'->>'new', ''),
                               NULLIF(d.title, ''), 'Untitled')";
    let system_expr = format!(
        "CONCAT_WS(' ', NULLIF(s.manufacturer, ''), COALESCE(s.name, {SUBMISSION_LIST_SYSTEM_CODE_SQL}, ''))"
    );
    let type_expr = submission_display_kind_sql();
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
                ds.target_disc_id IS NOT NULL AS submission_has_target_disc,
                {title_expr} AS title,
                {system_code_expr} AS system_code,
                COALESCE(s.short_name, '') AS system_short_name,
                u.username AS submitter,
                ds.submitter_id,
                ur.username AS reviewer,
                ds.reviewer_id,
                ds.status,
                CASE WHEN ds.status = 'Draft' THEN NULL ELSE ds.target_disc_id END AS target_disc_id,
                ds.created_at
         FROM disc_submissions ds
         JOIN users u ON u.id = ds.submitter_id
         LEFT JOIN users ur ON ur.id = ds.reviewer_id
         LEFT JOIN discs d ON d.id = ds.target_disc_id
         LEFT JOIN systems s
             ON s.code = {system_code_expr}
         WHERE {}
         ORDER BY {sort_col} {sort_dir}{nulls_order}
         LIMIT {page_size} OFFSET {offset}",
        conditions.join(" AND "),
        system_code_expr = SUBMISSION_LIST_SYSTEM_CODE_SQL
    );

    let mut query = sqlx::query_as::<_, SubmissionListRow>(&sql);
    if let Some(uid) = user_id_filter {
        query = query.bind(uid);
    }
    if let Some(disc_id) = disc_id_filter {
        query = query.bind(disc_id);
    }
    if !can_view_all_drafts {
        query = query.bind(viewer_id);
    }
    if let Some(status) = status_filter {
        if !status.is_empty() && status != "Pending and Draft" {
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
    viewer_id: i32,
    can_view_all_drafts: bool,
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
    let mut draft_owner_param = None;
    if !can_view_all_drafts {
        idx += 1;
        draft_owner_param = Some(idx);
        conditions.push(format!(
            "(ds.status <> 'Draft' OR ds.submitter_id = ${idx})"
        ));
    }
    if hide_disabled_disc_targets {
        conditions.push(match draft_owner_param {
            Some(viewer_idx) => format!(
                "(d.id IS NULL OR d.status <> 'Disabled' OR \
                 (ds.status = 'Draft' AND ds.submitter_id = ${viewer_idx}))"
            ),
            None => "(d.id IS NULL OR d.status <> 'Disabled')".to_string(),
        });
    }
    if let Some(status) = status_filter.filter(|status| !status.is_empty()) {
        if status == "Pending and Draft" {
            conditions.push("ds.status IN ('Pending', 'Draft')".to_string());
        } else {
            idx += 1;
            conditions.push(format!("ds.status::text = ${idx}"));
        }
    }
    if let Some(condition) = submission_type_filter_condition(type_filter) {
        conditions.push(condition);
    }
    if system_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("{SUBMISSION_LIST_SYSTEM_CODE_SQL} = ${idx}"));
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
    if !can_view_all_drafts {
        query = query.bind(viewer_id);
    }
    if let Some(status) = status_filter {
        if !status.is_empty() && status != "Pending and Draft" {
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
    use rand::RngCore;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::time::Duration;

    #[test]
    fn submission_tokens_are_optional_and_strictly_validated() {
        assert_eq!(normalize_submission_token(None).unwrap(), None);
        assert_eq!(normalize_submission_token(Some("  ")).unwrap(), None);

        let uppercase = "A".repeat(64);
        assert_eq!(
            normalize_submission_token(Some(&uppercase)).unwrap(),
            Some("a".repeat(64))
        );
        assert!(matches!(
            normalize_submission_token(Some("short")),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            normalize_submission_token(Some(&"z".repeat(64))),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn exact_disc_match_requires_one_shared_target() {
        assert_eq!(resolve_exact_disc_match(None, None), None);
        assert_eq!(resolve_exact_disc_match(Some(42), None), Some(42));
        assert_eq!(resolve_exact_disc_match(None, Some(42)), Some(42));
        assert_eq!(resolve_exact_disc_match(Some(42), Some(42)), Some(42));
        assert_eq!(resolve_exact_disc_match(Some(42), Some(7)), None);
    }

    #[test]
    fn submission_token_migration_is_nullable_and_unique() {
        let migration = include_str!("../../migrations/015_add_submission_tokens.sql");
        assert!(migration.contains("ADD COLUMN submission_token VARCHAR(64)"));
        assert!(migration.contains("ADD COLUMN submission_fingerprint VARCHAR(64)"));
        assert!(migration.contains("ON disc_submissions (submission_token)"));
        assert!(migration.contains("WHERE submission_token IS NOT NULL"));
    }

    #[test]
    fn draft_status_migration_adds_enum_value_after_pending() {
        let migration = include_str!("../../migrations/018_add_draft_submission_status.sql");
        assert!(migration.contains("ADD VALUE 'Draft' AFTER 'Pending'"));
    }

    #[tokio::test]
    async fn draft_requires_a_review_comment_before_database_access() {
        let error = draft_submission(&unreachable_pool(), 1, 1, " \r\n ")
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::BadRequest(_)));
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn concurrent_submission_retries_reuse_one_database_row() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let submitter_id: i32 = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mut token_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut token_bytes);
        let token = hex::encode(token_bytes);
        let fingerprint = "1".repeat(64);
        let changes = serde_json::json!({ "title": { "add": { "new": "Idempotency test" } } });

        let first = create_submission(
            &pool,
            SubmissionType::Disc,
            submitter_id,
            None,
            changes.clone(),
            Some("test"),
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        );
        let second = create_submission(
            &pool,
            SubmissionType::Disc,
            submitter_id,
            None,
            changes,
            Some("test"),
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        );
        let (first, second) = tokio::join!(first, second);
        let first = first.unwrap();
        let second = second.unwrap();

        let row_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM disc_submissions WHERE submission_token = $1")
                .bind(&token)
                .fetch_one(&pool)
                .await
                .unwrap();
        let same_form_after_state_change = create_submission(
            &pool,
            SubmissionType::Disc,
            submitter_id,
            None,
            serde_json::json!({ "title": { "add": { "new": "Derived differently" } } }),
            Some("test"),
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();
        let mismatched = create_submission(
            &pool,
            SubmissionType::Disc,
            submitter_id,
            None,
            serde_json::json!({ "title": { "add": { "new": "Changed" } } }),
            Some("test"),
            None,
            None,
            Some(&token),
            Some(&"2".repeat(64)),
        )
        .await;

        sqlx::query("DELETE FROM disc_submissions WHERE submission_token = $1")
            .bind(&token)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(row_count, 1);
        assert_eq!(first.submission.id, second.submission.id);
        assert_eq!(
            first.submission.id,
            same_form_after_state_change.submission.id
        );
        assert_ne!(first.created, second.created);
        assert!(!same_form_after_state_change.created);
        assert_eq!(first.submission.status, SubmissionStatus::Pending);
        assert_eq!(second.submission.status, SubmissionStatus::Pending);
        assert!(matches!(mismatched, Err(AppError::BadRequest(_))));
    }

    struct ApprovalFixture {
        disc_id: i32,
        submitter_id: i32,
        title: String,
        system_code: String,
        archives_dirty: bool,
    }

    fn random_test_token() -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    async fn approval_fixture(pool: &PgPool, label: &str) -> ApprovalFixture {
        let submitter_id: i32 = sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap();
        let title = format!("Atomic approval {label} {}", &random_test_token()[..12]);
        let (disc_id, system_code): (i32, String) = sqlx::query_as(
            "INSERT INTO discs (system_code, media_type_code, title, category_id, status)
             SELECT system_code, media_type_code, $1, category_id, 'Unverified'
             FROM discs ORDER BY id LIMIT 1
             RETURNING id, system_code",
        )
        .bind(&title)
        .fetch_one(pool)
        .await
        .unwrap();
        let archives_dirty: bool =
            sqlx::query_scalar("SELECT archives_dirty FROM systems WHERE code = $1")
                .bind(&system_code)
                .fetch_one(pool)
                .await
                .unwrap();
        ApprovalFixture {
            disc_id,
            submitter_id,
            title,
            system_code,
            archives_dirty,
        }
    }

    async fn cleanup_approval_fixture(pool: &PgPool, fixture: &ApprovalFixture, tokens: &[&str]) {
        sqlx::query("DELETE FROM disc_submissions WHERE submission_token = ANY($1)")
            .bind(tokens)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM discs WHERE id = $1")
            .bind(fixture.disc_id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("UPDATE systems SET archives_dirty = $1 WHERE code = $2")
            .bind(fixture.archives_dirty)
            .bind(&fixture.system_code)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn manual_retarget(
        pool: &PgPool,
        submission_id: i32,
        expected_target_disc_id: Option<i32>,
        target_disc_id: Option<i32>,
    ) -> ManualRetargetOutcome {
        manually_retarget_pending_submission(
            pool,
            submission_id,
            expected_target_disc_id,
            target_disc_id,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn retargeted_verification_updates_existing_disc_on_second_approval() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "retarget verification").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let universal_hash = random_test_token()[..40].to_string();
        let universal_hash_bytes = hex::decode(&universal_hash).unwrap();
        sqlx::query(
            "UPDATE discs SET universal_hash = $1,
                    serial = ARRAY['EXISTING-001'],
                    edition = ARRAY['Original'],
                    barcode = ARRAY['0000000000000']
             WHERE id = $2",
        )
        .bind(universal_hash_bytes)
        .bind(fixture.disc_id)
        .execute(&pool)
        .await
        .unwrap();
        let existing_ring_id: i32 = sqlx::query_scalar(
            "INSERT INTO disc_ring_code_entries (disc_id, offset_value, comment)
             VALUES ($1, 0, 'existing ring') RETURNING id",
        )
        .bind(fixture.disc_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO disc_ring_code_layers (entry_id, layer, mastering_code)
             VALUES ($1, 0, 'MASTER-EXISTING')",
        )
        .bind(existing_ring_id)
        .execute(&pool)
        .await
        .unwrap();

        let dat = "<rom name=\"Track 1.bin\" size=\"1\" crc=\"11111111\" md5=\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" sha1=\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\" />";
        sqlx::query(
            "INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1)
             VALUES ($1, '1', 1, '11111111', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb')",
        )
        .bind(fixture.disc_id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            find_unambiguous_exact_disc_match(&pool, Some(dat), Some(&universal_hash))
                .await
                .unwrap(),
            Some(fixture.disc_id)
        );

        let original_changes = serde_json::json!({
            "dat": {
                "add": {
                    "new": dat
                }
            },
            "universal_hash": { "add": { "new": universal_hash.clone() } },
            "comments": { "add": { "new": "Original submission" } },
            "serial": { "add": ["NEW-002"] },
            "edition": { "add": ["Big Box"] },
            "barcode": { "add": ["1111111111111"] },
            "ring_codes": [{
                "offset_value": { "add": { "new": "-153" } },
                "layers": [{
                    "index": 0,
                    "mastering_code": { "add": { "new": "MASTER-NEW" } }
                }]
            }]
        });
        let expected_changes = original_changes.clone();
        let creation = create_submission(
            &pool,
            SubmissionType::Disc,
            fixture.submitter_id,
            None,
            original_changes,
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();
        let retargeted = retarget_pending_submission(
            &pool,
            creation.submission.id,
            fixture.disc_id,
            Some(dat),
            Some(&universal_hash),
        )
        .await
        .unwrap();
        let retargeted_submission = get_submission(&pool, creation.submission.id).await.unwrap();

        assert!(retargeted);
        assert_eq!(retargeted_submission.target_disc_id, Some(fixture.disc_id));
        assert_eq!(retargeted_submission.status, SubmissionStatus::Pending);
        assert_eq!(retargeted_submission.changes, expected_changes);
        assert_eq!(retargeted_submission.changes_original, None);
        assert_eq!(
            retargeted_submission.display_kind(),
            SubmissionDisplayKind::Verification
        );

        let repeated = retarget_pending_submission(
            &pool,
            creation.submission.id,
            fixture.disc_id,
            Some(dat),
            Some(&universal_hash),
        )
        .await
        .unwrap();
        assert!(!repeated);

        let target_hash = current_disc_snapshot_hash(&pool, fixture.disc_id)
            .await
            .unwrap();
        let approval = approve_submission(
            &pool,
            &retargeted_submission,
            &retargeted_submission.changes,
            fixture.submitter_id,
            None,
            Some(&target_hash),
        )
        .await
        .unwrap();
        let final_submission = get_submission(&pool, creation.submission.id).await.unwrap();
        let verification_rows = list_submissions(
            &pool,
            Some(fixture.submitter_id),
            None,
            false,
            false,
            fixture.submitter_id,
            true,
            Some("Approved"),
            Some("Verification"),
            None,
            None,
            "date",
            "desc",
            1,
            50,
        )
        .await
        .unwrap();
        let new_disc_rows = list_submissions(
            &pool,
            Some(fixture.submitter_id),
            None,
            false,
            false,
            fixture.submitter_id,
            true,
            Some("Approved"),
            Some("New Disc"),
            None,
            None,
            "date",
            "desc",
            1,
            50,
        )
        .await
        .unwrap();
        let target_status: DiscStatus =
            sqlx::query_scalar("SELECT status FROM discs WHERE id = $1")
                .bind(fixture.disc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let (serial, edition, barcode): (Vec<String>, Vec<String>, Vec<String>) =
            sqlx::query_as("SELECT serial, edition, barcode FROM discs WHERE id = $1")
                .bind(fixture.disc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let ring_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM disc_ring_code_entries WHERE disc_id = $1")
                .bind(fixture.disc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let dumper_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM disc_dumpers WHERE disc_id = $1 AND user_id = $2",
        )
        .bind(fixture.disc_id)
        .bind(fixture.submitter_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;

        assert_eq!(approval, ApprovalOutcome::Approved(fixture.disc_id));
        assert_eq!(final_submission.status, SubmissionStatus::Approved);
        assert_eq!(final_submission.display_kind(), SubmissionDisplayKind::Disc);
        assert!(verification_rows
            .iter()
            .any(|row| row.id == final_submission.id));
        assert!(!new_disc_rows
            .iter()
            .any(|row| row.id == final_submission.id));
        assert_eq!(final_submission.changes_original, Some(expected_changes));
        assert_eq!(target_status, DiscStatus::Verified);
        assert_eq!(serial, vec!["EXISTING-001", "NEW-002"]);
        assert_eq!(edition, vec!["Original", "Big Box"]);
        assert_eq!(barcode, vec!["0000000000000", "1111111111111"]);
        assert_eq!(ring_count, 2);
        assert_eq!(dumper_count, 1);
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn manual_retarget_assigns_replaces_removes_and_overwrites_dat_on_approval() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let primary = approval_fixture(&pool, "manual retarget primary").await;
        let alternate = approval_fixture(&pool, "manual retarget alternate").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let old_dat = "<rom name=\"Track.bin\" size=\"1\" crc=\"11111111\" md5=\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" sha1=\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\" />";
        let new_dat = "<rom name=\"Track.bin\" size=\"2\" crc=\"22222222\" md5=\"cccccccccccccccccccccccccccccccc\" sha1=\"dddddddddddddddddddddddddddddddddddddddd\" />";
        sqlx::query(
            "INSERT INTO files (disc_id, track_number, size, crc32, md5, sha1)
             VALUES ($1, '1', 1, '11111111', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb')",
        )
        .bind(primary.disc_id)
        .execute(&pool)
        .await
        .unwrap();

        let original_changes = serde_json::json!({
            "dat": { "add": { "new": new_dat } },
            "comments": { "add": { "new": "manual retarget payload" } }
        });
        let creation = create_submission(
            &pool,
            SubmissionType::Disc,
            primary.submitter_id,
            None,
            original_changes.clone(),
            Some("stored submission comment"),
            Some("stored dump log"),
            Some("https://example.test/logs"),
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE disc_submissions SET reviewer_id = $1,
                    review_comment = 'stored review comment', reviewed_at = NOW()
             WHERE id = $2",
        )
        .bind(primary.submitter_id)
        .bind(creation.submission.id)
        .execute(&pool)
        .await
        .unwrap();
        let baseline = get_submission(&pool, creation.submission.id).await.unwrap();

        assert_eq!(
            manual_retarget(&pool, baseline.id, None, Some(primary.disc_id)).await,
            ManualRetargetOutcome::Retargeted
        );
        let assigned = get_submission(&pool, baseline.id).await.unwrap();
        assert_eq!(assigned.target_disc_id, Some(primary.disc_id));

        assert_eq!(
            manual_retarget(
                &pool,
                baseline.id,
                Some(primary.disc_id),
                Some(alternate.disc_id),
            )
            .await,
            ManualRetargetOutcome::Retargeted
        );
        let reassigned = get_submission(&pool, baseline.id).await.unwrap();
        assert_eq!(reassigned.target_disc_id, Some(alternate.disc_id));

        assert_eq!(
            manual_retarget(&pool, baseline.id, Some(alternate.disc_id), None).await,
            ManualRetargetOutcome::Retargeted
        );
        let removed = get_submission(&pool, baseline.id).await.unwrap();
        assert_eq!(removed.target_disc_id, None);
        assert_eq!(removed.display_kind(), SubmissionDisplayKind::NewDisc);

        for submission in [&assigned, &reassigned, &removed] {
            assert_eq!(submission.changes, baseline.changes);
            assert_eq!(submission.changes_original, baseline.changes_original);
            assert_eq!(submission.status, baseline.status);
            assert_eq!(submission.reviewer_id, baseline.reviewer_id);
            assert_eq!(submission.review_comment, baseline.review_comment);
            assert_eq!(submission.reviewed_at, baseline.reviewed_at);
            assert_eq!(submission.created_at, baseline.created_at);
            assert_eq!(submission.submission_comment, baseline.submission_comment);
            assert_eq!(submission.dump_log, baseline.dump_log);
            assert_eq!(submission.extra_upload_url, baseline.extra_upload_url);
        }

        assert_eq!(
            manual_retarget(&pool, baseline.id, None, None).await,
            ManualRetargetOutcome::Unchanged
        );
        assert_eq!(
            manual_retarget(&pool, baseline.id, None, Some(i32::MAX)).await,
            ManualRetargetOutcome::TargetNotFound
        );
        sqlx::query("UPDATE discs SET status = 'Disabled' WHERE id = $1")
            .bind(alternate.disc_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            manual_retarget(&pool, baseline.id, None, Some(alternate.disc_id)).await,
            ManualRetargetOutcome::TargetDisabled
        );
        sqlx::query("UPDATE discs SET status = 'Unverified' WHERE id = $1")
            .bind(alternate.disc_id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            manual_retarget(
                &pool,
                baseline.id,
                Some(primary.disc_id),
                Some(alternate.disc_id),
            )
            .await,
            ManualRetargetOutcome::SubmissionChanged
        );

        assert_eq!(
            manual_retarget(&pool, baseline.id, None, Some(primary.disc_id)).await,
            ManualRetargetOutcome::Retargeted
        );
        let submission = get_submission(&pool, baseline.id).await.unwrap();
        let target_hash = current_disc_snapshot_hash(&pool, primary.disc_id)
            .await
            .unwrap();
        let reviewed_changes = serde_json::json!({
            "dat": { "modify": { "old": old_dat, "new": new_dat } }
        });
        let approval = approve_submission(
            &pool,
            &submission,
            &reviewed_changes,
            primary.submitter_id,
            None,
            Some(&target_hash),
        )
        .await
        .unwrap();
        let final_submission = get_submission(&pool, baseline.id).await.unwrap();
        let stored_file: (i64, String, String, String) =
            sqlx::query_as("SELECT size, crc32, md5, sha1 FROM files WHERE disc_id = $1")
                .bind(primary.disc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let dumper_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM disc_dumpers WHERE disc_id = $1 AND user_id = $2",
        )
        .bind(primary.disc_id)
        .bind(primary.submitter_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        cleanup_approval_fixture(&pool, &primary, &[&token]).await;
        cleanup_approval_fixture(&pool, &alternate, &[]).await;

        assert_eq!(approval, ApprovalOutcome::Approved(primary.disc_id));
        assert_eq!(final_submission.changes_original, Some(original_changes));
        assert_eq!(
            stored_file,
            (
                2,
                "22222222".to_string(),
                "cccccccccccccccccccccccccccccccc".to_string(),
                "dddddddddddddddddddddddddddddddddddddddd".to_string(),
            )
        );
        assert_eq!(dumper_count, 1);
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn manual_target_removal_allows_new_disc_approval() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "manual target removal").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let media_type: String =
            sqlx::query_scalar("SELECT media_type_code FROM discs WHERE id = $1")
                .bind(fixture.disc_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let unique_hash = random_test_token();
        let crc32 = &unique_hash[..8];
        let md5 = &unique_hash[..32];
        let sha1 = &unique_hash[..40];
        let dat = format!(
            "<rom name=\"Track.bin\" size=\"3\" crc=\"{crc32}\" md5=\"{md5}\" sha1=\"{sha1}\" />"
        );
        let original_changes = serde_json::json!({
            "system_code": { "add": { "new": fixture.system_code.clone() } },
            "media_type": { "add": { "new": media_type } },
            "title": { "add": { "new": format!("Removed target {}", &token[..12]) } },
            "category": { "add": { "new": "Games" } },
            "dat": { "add": { "new": dat } }
        });
        let creation = create_submission(
            &pool,
            SubmissionType::Disc,
            fixture.submitter_id,
            Some(fixture.disc_id),
            original_changes.clone(),
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();

        assert_eq!(
            manual_retarget(&pool, creation.submission.id, Some(fixture.disc_id), None).await,
            ManualRetargetOutcome::Retargeted
        );
        let removed = get_submission(&pool, creation.submission.id).await.unwrap();
        assert_eq!(removed.target_disc_id, None);
        assert_eq!(removed.display_kind(), SubmissionDisplayKind::NewDisc);
        assert_eq!(removed.changes, original_changes);

        let approval = approve_submission(
            &pool,
            &removed,
            &removed.changes,
            fixture.submitter_id,
            None,
            None,
        )
        .await
        .unwrap();
        let new_disc_id = match approval {
            ApprovalOutcome::Approved(disc_id) => disc_id,
            other => panic!("unexpected approval outcome: {other:?}"),
        };
        let final_submission = get_submission(&pool, removed.id).await.unwrap();
        let stored_file: (i64, String, String, String) =
            sqlx::query_as("SELECT size, crc32, md5, sha1 FROM files WHERE disc_id = $1")
                .bind(new_disc_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;
        sqlx::query("DELETE FROM discs WHERE id = $1")
            .bind(new_disc_id)
            .execute(&pool)
            .await
            .unwrap();

        assert_ne!(new_disc_id, fixture.disc_id);
        assert_eq!(final_submission.target_disc_id, Some(new_disc_id));
        assert_eq!(final_submission.changes_original, Some(original_changes));
        assert_eq!(stored_file.0, 3);
        assert_eq!(stored_file.1, crc32);
        assert_eq!(stored_file.2, md5);
        assert_eq!(stored_file.3, sha1);
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn concurrent_manual_retarget_and_approval_have_one_winner() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let primary = approval_fixture(&pool, "retarget approval race primary").await;
        let alternate = approval_fixture(&pool, "retarget approval race alternate").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let creation = create_submission(
            &pool,
            SubmissionType::Disc,
            primary.submitter_id,
            Some(primary.disc_id),
            serde_json::json!({}),
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();
        let target_hash = current_disc_snapshot_hash(&pool, primary.disc_id)
            .await
            .unwrap();

        let approval = approve_submission(
            &pool,
            &creation.submission,
            &creation.submission.changes,
            primary.submitter_id,
            None,
            Some(&target_hash),
        );
        let retarget = manually_retarget_pending_submission(
            &pool,
            creation.submission.id,
            Some(primary.disc_id),
            Some(alternate.disc_id),
        );
        let (approval, retarget) = tokio::join!(approval, retarget);
        let approval = approval.unwrap();
        let retarget = retarget.unwrap();
        let final_submission = get_submission(&pool, creation.submission.id).await.unwrap();

        match (approval, retarget) {
            (ApprovalOutcome::Approved(disc_id), ManualRetargetOutcome::SubmissionChanged) => {
                assert_eq!(disc_id, primary.disc_id);
                assert_eq!(final_submission.status, SubmissionStatus::Approved);
                assert_eq!(final_submission.target_disc_id, Some(primary.disc_id));
            }
            (ApprovalOutcome::AlreadyProcessed, ManualRetargetOutcome::Retargeted) => {
                assert_eq!(final_submission.status, SubmissionStatus::Pending);
                assert_eq!(final_submission.target_disc_id, Some(alternate.disc_id));
            }
            outcomes => panic!("unexpected race outcomes: {outcomes:?}"),
        }

        cleanup_approval_fixture(&pool, &primary, &[&token]).await;
        cleanup_approval_fixture(&pool, &alternate, &[]).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn atomic_moderator_approval_rolls_back_claim_and_partial_disc_writes() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "moderator rollback").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let changed_title = format!("{} changed", fixture.title);
        let changes = serde_json::json!({
            "title": { "modify": { "old": fixture.title, "new": changed_title } },
            "languages": { "add": ["__"] }
        });
        let creation = create_submission(
            &pool,
            SubmissionType::Edit,
            fixture.submitter_id,
            Some(fixture.disc_id),
            changes,
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();

        let approval = approve_submission(
            &pool,
            &creation.submission,
            &creation.submission.changes,
            fixture.submitter_id,
            None,
            None,
        )
        .await;
        let status: SubmissionStatus =
            sqlx::query_scalar("SELECT status FROM disc_submissions WHERE id = $1")
                .bind(creation.submission.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let stored_title: String = sqlx::query_scalar("SELECT title FROM discs WHERE id = $1")
            .bind(fixture.disc_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;

        assert!(matches!(approval, Err(AppError::Database(_))));
        assert_eq!(status, SubmissionStatus::Pending);
        assert_eq!(stored_title, fixture.title);
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn atomic_direct_failure_leaves_no_submission_or_partial_disc_writes() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "direct rollback").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let changed_title = format!("{} changed", fixture.title);
        let changes = serde_json::json!({
            "title": { "modify": { "old": fixture.title, "new": changed_title } },
            "languages": { "add": ["__"] }
        });

        let approval = create_and_approve_submission(
            &pool,
            SubmissionType::Edit,
            fixture.submitter_id,
            Some(fixture.disc_id),
            changes,
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
            fixture.submitter_id,
        )
        .await;
        let submission_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM disc_submissions WHERE submission_token = $1")
                .bind(&token)
                .fetch_one(&pool)
                .await
                .unwrap();
        let stored_title: String = sqlx::query_scalar("SELECT title FROM discs WHERE id = $1")
            .bind(fixture.disc_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;

        assert!(matches!(approval, Err(AppError::Database(_))));
        assert_eq!(submission_count, 0);
        assert_eq!(stored_title, fixture.title);
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn concurrent_atomic_direct_retries_commit_one_approved_change() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "direct retry").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let changed_title = format!("{} changed", fixture.title);
        let changes = serde_json::json!({
            "title": { "modify": { "old": fixture.title, "new": changed_title } }
        });

        let first = create_and_approve_submission(
            &pool,
            SubmissionType::Edit,
            fixture.submitter_id,
            Some(fixture.disc_id),
            changes.clone(),
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
            fixture.submitter_id,
        );
        let second = create_and_approve_submission(
            &pool,
            SubmissionType::Edit,
            fixture.submitter_id,
            Some(fixture.disc_id),
            changes,
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
            fixture.submitter_id,
        );
        let (first, second) = tokio::join!(first, second);
        let submission_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM disc_submissions
             WHERE submission_token = $1 AND status = 'Approved'",
        )
        .bind(&token)
        .fetch_one(&pool)
        .await
        .unwrap();
        let stored_title: String = sqlx::query_scalar("SELECT title FROM discs WHERE id = $1")
            .bind(fixture.disc_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;

        assert!(
            matches!(first.unwrap(), DirectSubmissionOutcome::Approved(id) if id == fixture.disc_id)
        );
        assert!(
            matches!(second.unwrap(), DirectSubmissionOutcome::Approved(id) if id == fixture.disc_id)
        );
        assert_eq!(submission_count, 1);
        assert_eq!(stored_title, changed_title);
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn atomic_direct_conflict_rolls_back_new_submission() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "direct conflict").await;
        let conflicting_disc_id: i32 = sqlx::query_scalar(
            "INSERT INTO discs (system_code, media_type_code, title, category_id, status)
             SELECT system_code, media_type_code, title, category_id, 'Unverified'
             FROM discs WHERE id = $1 RETURNING id",
        )
        .bind(fixture.disc_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let token = random_test_token();
        let fingerprint = random_test_token();
        let changes = serde_json::json!({
            "comments": { "add": { "new": "Conflict rollback test" } }
        });

        let outcome = create_and_approve_submission(
            &pool,
            SubmissionType::Edit,
            fixture.submitter_id,
            Some(fixture.disc_id),
            changes,
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
            fixture.submitter_id,
        )
        .await
        .unwrap();
        let submission_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM disc_submissions WHERE submission_token = $1")
                .bind(&token)
                .fetch_one(&pool)
                .await
                .unwrap();

        sqlx::query("DELETE FROM discs WHERE id = $1")
            .bind(conflicting_disc_id)
            .execute(&pool)
            .await
            .unwrap();
        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;

        assert!(
            matches!(outcome, DirectSubmissionOutcome::Conflicts(conflicts) if !conflicts.is_empty())
        );
        assert_eq!(submission_count, 0);
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn atomic_approve_reject_race_has_one_consistent_winner() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "approve reject race").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let changed_title = format!("{} changed", fixture.title);
        let changes = serde_json::json!({
            "title": { "modify": { "old": fixture.title, "new": changed_title } }
        });
        let creation = create_submission(
            &pool,
            SubmissionType::Edit,
            fixture.submitter_id,
            Some(fixture.disc_id),
            changes,
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();

        let approve = approve_submission(
            &pool,
            &creation.submission,
            &creation.submission.changes,
            fixture.submitter_id,
            None,
            None,
        );
        let reject = reject_submission(
            &pool,
            creation.submission.id,
            fixture.submitter_id,
            Some("race"),
        );
        let (approve, reject) = tokio::join!(approve, reject);
        let approve = approve.unwrap();
        let reject = reject.unwrap();
        let status: SubmissionStatus =
            sqlx::query_scalar("SELECT status FROM disc_submissions WHERE id = $1")
                .bind(creation.submission.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let stored_title: String = sqlx::query_scalar("SELECT title FROM discs WHERE id = $1")
            .bind(fixture.disc_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;

        match status {
            SubmissionStatus::Approved => {
                assert_eq!(approve, ApprovalOutcome::Approved(fixture.disc_id));
                assert!(!reject);
                assert_eq!(stored_title, changed_title);
            }
            SubmissionStatus::Rejected => {
                assert_eq!(approve, ApprovalOutcome::AlreadyProcessed);
                assert!(reject);
                assert_eq!(stored_title, fixture.title);
            }
            other => panic!("unexpected race outcome: {other:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn draft_round_trip_preserves_review_history_and_submission_identity() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "draft round trip").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let original_changes = serde_json::json!({
            "title": { "add": { "new": "Original submitted title" } }
        });
        let creation = create_submission(
            &pool,
            SubmissionType::Disc,
            fixture.submitter_id,
            None,
            original_changes.clone(),
            Some("original submission comment"),
            Some("original dump log"),
            Some("https://example.test/original"),
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();
        let original = creation.submission;

        assert!(draft_submission(
            &pool,
            original.id,
            fixture.submitter_id,
            "Please correct this submission",
        )
        .await
        .unwrap());
        let drafted = get_submission(&pool, original.id).await.unwrap();
        assert_eq!(drafted.status, SubmissionStatus::Draft);
        assert_eq!(drafted.changes, original_changes);
        assert_eq!(drafted.submission_comment, original.submission_comment);
        assert_eq!(drafted.target_disc_id, original.target_disc_id);
        assert_eq!(drafted.dump_log, original.dump_log);
        assert_eq!(drafted.extra_upload_url, original.extra_upload_url);
        assert_eq!(drafted.created_at, original.created_at);
        assert_eq!(
            drafted.review_comment.as_deref(),
            Some("Please correct this submission")
        );

        let revised_changes = serde_json::json!({
            "comments": { "add": { "new": "Corrected submission" } }
        });
        let revised_fingerprint = random_test_token();
        let updated = submit_draft_submission(
            &pool,
            drafted.id,
            fixture.submitter_id,
            Some(fixture.disc_id),
            revised_changes.clone(),
            Some("revised submission comment"),
            Some("revised dump log"),
            Some("https://example.test/revised"),
            Some(&token),
            Some(&revised_fingerprint),
        )
        .await
        .unwrap()
        .unwrap();
        let overwrite_fingerprint = random_test_token();
        let repeated = submit_draft_submission(
            &pool,
            drafted.id,
            fixture.submitter_id,
            Some(fixture.disc_id),
            serde_json::json!({"title": {"add": {"new": "overwrite"}}}),
            None,
            None,
            None,
            Some(&token),
            Some(&overwrite_fingerprint),
        )
        .await
        .unwrap();

        assert_eq!(updated.id, original.id);
        assert_eq!(updated.status, SubmissionStatus::Pending);
        assert_eq!(updated.changes, revised_changes);
        assert_eq!(updated.target_disc_id, Some(fixture.disc_id));
        assert_eq!(updated.created_at, original.created_at);
        assert_eq!(updated.reviewer_id, drafted.reviewer_id);
        assert_eq!(updated.review_comment, drafted.review_comment);
        assert_eq!(updated.reviewed_at, drafted.reviewed_at);
        assert!(repeated.is_none());

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn atomic_approve_draft_race_has_one_consistent_winner() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "approve draft race").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let changed_title = format!("{} changed", fixture.title);
        let changes = serde_json::json!({
            "title": { "modify": { "old": fixture.title, "new": changed_title } }
        });
        let creation = create_submission(
            &pool,
            SubmissionType::Disc,
            fixture.submitter_id,
            Some(fixture.disc_id),
            changes,
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();

        let approve = approve_submission(
            &pool,
            &creation.submission,
            &creation.submission.changes,
            fixture.submitter_id,
            None,
            None,
        );
        let draft = draft_submission(&pool, creation.submission.id, fixture.submitter_id, "race");
        let (approve, draft) = tokio::join!(approve, draft);
        let approve = approve.unwrap();
        let draft = draft.unwrap();
        let final_submission = get_submission(&pool, creation.submission.id).await.unwrap();
        let stored_title: String = sqlx::query_scalar("SELECT title FROM discs WHERE id = $1")
            .bind(fixture.disc_id)
            .fetch_one(&pool)
            .await
            .unwrap();

        match final_submission.status {
            SubmissionStatus::Approved => {
                assert_eq!(approve, ApprovalOutcome::Approved(fixture.disc_id));
                assert!(!draft);
                assert_eq!(stored_title, changed_title);
            }
            SubmissionStatus::Draft => {
                assert_eq!(approve, ApprovalOutcome::AlreadyProcessed);
                assert!(draft);
                assert_eq!(stored_title, fixture.title);
            }
            other => panic!("unexpected race outcome: {other:?}"),
        }

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn draft_queue_visibility_uses_target_title_without_exposing_target_id() {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let fixture = approval_fixture(&pool, "draft visibility").await;
        let token = random_test_token();
        let fingerprint = random_test_token();
        let outsider_name = format!("draft-outsider-{}", &random_test_token()[..12]);
        let outsider_id: i32 =
            sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
                .bind(&outsider_name)
                .fetch_one(&pool)
                .await
                .unwrap();
        let creation = create_submission(
            &pool,
            SubmissionType::Disc,
            fixture.submitter_id,
            Some(fixture.disc_id),
            serde_json::json!({
                "comments": { "add": { "new": "Verification submission" } }
            }),
            None,
            None,
            None,
            Some(&token),
            Some(&fingerprint),
        )
        .await
        .unwrap();
        draft_submission(
            &pool,
            creation.submission.id,
            fixture.submitter_id,
            "fix it",
        )
        .await
        .unwrap();

        let owner_rows = list_submissions(
            &pool,
            None,
            None,
            false,
            true,
            fixture.submitter_id,
            false,
            Some("Draft"),
            Some("Verification"),
            None,
            None,
            "date",
            "desc",
            1,
            50,
        )
        .await
        .unwrap();
        let outsider_rows = list_submissions(
            &pool,
            None,
            None,
            false,
            true,
            outsider_id,
            false,
            Some("Draft"),
            None,
            None,
            None,
            "date",
            "desc",
            1,
            50,
        )
        .await
        .unwrap();
        let moderator_rows = list_submissions(
            &pool,
            None,
            None,
            false,
            false,
            outsider_id,
            true,
            Some("Draft"),
            None,
            None,
            None,
            "date",
            "desc",
            1,
            50,
        )
        .await
        .unwrap();
        let active_owner_rows = list_submissions(
            &pool,
            None,
            None,
            false,
            true,
            fixture.submitter_id,
            false,
            Some("Pending and Draft"),
            None,
            None,
            None,
            "date",
            "desc",
            1,
            50,
        )
        .await
        .unwrap();

        let owner_row = owner_rows
            .iter()
            .find(|row| row.id == creation.submission.id)
            .unwrap();
        assert_eq!(owner_row.title, fixture.title);
        assert_eq!(owner_row.target_disc_id, None);
        assert_eq!(owner_row.display_kind, SubmissionDisplayKind::Verification);
        assert!(!outsider_rows
            .iter()
            .any(|row| row.id == creation.submission.id));
        assert!(moderator_rows
            .iter()
            .any(|row| row.id == creation.submission.id));
        assert!(active_owner_rows
            .iter()
            .any(|row| row.id == creation.submission.id));

        cleanup_approval_fixture(&pool, &fixture, &[&token]).await;
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(outsider_id)
            .execute(&pool)
            .await
            .unwrap();
    }

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

    fn ring_merge_candidate(
        mastering: (&str, &str),
        numeric_fields: [&str; 3],
        comment: &str,
        mould_sids: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": 1,
            "offset_value": numeric_fields[0],
            "offset_extra_value": numeric_fields[1],
            "sample_start": numeric_fields[2],
            "comment": comment,
            "layers": [{
                "mastering_code": mastering.0,
                "mastering_sid": mastering.1,
                "toolstamps": "",
                "mould_sids": mould_sids,
                "additional_moulds": ""
            }]
        })
    }

    fn ring_merge_change(
        mastering: (&str, &str),
        numeric_fields: [&str; 3],
        comment: &str,
        mould_sids: &str,
    ) -> serde_json::Value {
        let mut change = serde_json::json!({
            "layers": [{ "index": 0 }]
        });

        for (key, value) in [
            ("offset_value", numeric_fields[0]),
            ("offset_extra_value", numeric_fields[1]),
            ("sample_data_start", numeric_fields[2]),
            ("comment", comment),
        ] {
            if !value.is_empty() {
                change[key] = serde_json::json!({ "add": { "new": value } });
            }
        }

        for (key, value) in [
            ("mastering_code", mastering.0),
            ("mastering_sid", mastering.1),
            ("mould_sids", mould_sids),
        ] {
            if !value.is_empty() {
                change["layers"][0][key] = serde_json::json!({ "add": { "new": value } });
            }
        }

        change
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
            submission_token: None,
            submission_fingerprint: None,
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

        let dat_err = find_matching_disc(&pool, &dat).await.unwrap_err();
        assert!(matches!(dat_err, AppError::Database(_)));

        let hash_err =
            find_matching_disc_by_universal_hash(&pool, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .await
                .unwrap_err();
        assert!(matches!(hash_err, AppError::Database(_)));
    }

    #[test]
    fn verification_match_queries_always_exclude_disabled_discs() {
        assert!(DAT_MATCH_CANDIDATES_SQL.contains("d.status <> 'Disabled'"));
        assert!(UNIVERSAL_HASH_MATCH_SQL.contains("status <> 'Disabled'"));
    }

    #[test]
    fn status_free_disc_approval_generates_status_changes() {
        let new_disc = resolve_approval_from_snapshot(
            SubmissionType::Disc,
            None,
            &serde_json::json!({}),
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(
            new_disc.changes["status"],
            serde_json::json!({ "add": { "new": "Unverified" } })
        );
        assert_eq!(new_disc.effective_data["status"], "Unverified");

        for old_status in [DiscStatus::Unverified, DiscStatus::Questionable] {
            let snapshot = serde_json::json!({ "status": old_status.to_string() });
            let verification = resolve_approval_from_snapshot(
                SubmissionType::Disc,
                Some(old_status),
                &snapshot,
                &serde_json::json!({}),
            )
            .unwrap();

            assert_eq!(
                verification.changes["status"],
                serde_json::json!({
                    "modify": {
                        "old": old_status.to_string(),
                        "new": "Verified"
                    }
                })
            );
            assert_eq!(verification.effective_data["status"], "Verified");
        }

        let verified = resolve_approval_from_snapshot(
            SubmissionType::Disc,
            Some(DiscStatus::Verified),
            &serde_json::json!({ "status": "Verified" }),
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(verified.changes.get("status").is_none());
        assert_eq!(verified.effective_data["status"], "Verified");
    }

    #[test]
    fn explicit_status_suppresses_disc_approval_automation() {
        let new_disc_changes = serde_json::json!({
            "status": { "add": { "new": "Verified" } }
        });
        let new_disc = resolve_approval_from_snapshot(
            SubmissionType::Disc,
            None,
            &serde_json::json!({}),
            &new_disc_changes,
        )
        .unwrap();
        assert_eq!(new_disc.changes, new_disc_changes);
        assert_eq!(new_disc.effective_data["status"], "Verified");

        let verification_changes = serde_json::json!({
            "status": {
                "modify": { "old": "Unverified", "new": "Questionable" }
            }
        });
        let verification = resolve_approval_from_snapshot(
            SubmissionType::Disc,
            Some(DiscStatus::Unverified),
            &serde_json::json!({ "status": "Unverified" }),
            &verification_changes,
        )
        .unwrap();
        assert_eq!(verification.changes, verification_changes);
        assert_eq!(verification.effective_data["status"], "Questionable");
    }

    #[test]
    fn explicit_disc_status_survives_a_status_free_review_delta() {
        let submitted_changes = serde_json::json!({
            "status": {
                "modify": { "old": "Unverified", "new": "Questionable" }
            }
        });
        let reviewed_changes = serde_json::json!({
            "comments": { "add": { "new": "reviewed" } }
        });

        let approval_changes = preserve_submitted_status_change(
            SubmissionType::Disc,
            &submitted_changes,
            &reviewed_changes,
        )
        .unwrap();
        let resolution = resolve_approval_from_snapshot(
            SubmissionType::Disc,
            Some(DiscStatus::Unverified),
            &serde_json::json!({ "status": "Unverified" }),
            &approval_changes,
        )
        .unwrap();

        assert_eq!(resolution.changes["status"], submitted_changes["status"]);
        assert_eq!(resolution.effective_data["status"], "Questionable");
        assert_eq!(resolution.effective_data["comments"], "reviewed");
    }

    #[test]
    fn edit_approval_never_generates_status_changes() {
        let snapshot = serde_json::json!({ "status": "Unverified" });
        let unchanged = resolve_approval_from_snapshot(
            SubmissionType::Edit,
            Some(DiscStatus::Unverified),
            &snapshot,
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(unchanged.changes.get("status").is_none());
        assert_eq!(unchanged.effective_data["status"], "Unverified");

        let explicit_changes = serde_json::json!({
            "status": {
                "modify": { "old": "Unverified", "new": "Disabled" }
            }
        });
        let explicit = resolve_approval_from_snapshot(
            SubmissionType::Edit,
            Some(DiscStatus::Unverified),
            &snapshot,
            &explicit_changes,
        )
        .unwrap();
        assert_eq!(explicit.changes, explicit_changes);
        assert_eq!(explicit.effective_data["status"], "Disabled");
    }

    #[test]
    fn disabled_targets_block_only_verification_approval() {
        assert!(disabled_verification_target(
            SubmissionType::Disc,
            Some(DiscStatus::Disabled)
        ));
        assert!(!disabled_verification_target(
            SubmissionType::Edit,
            Some(DiscStatus::Disabled)
        ));
        assert!(!disabled_verification_target(
            SubmissionType::Disc,
            Some(DiscStatus::Questionable)
        ));
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
    fn generated_name_conflict_uses_archive_eligible_statuses() {
        assert!(!effective_disc_is_archive_eligible(
            &serde_json::json!({"status": "Disabled"})
        ));
        assert!(!effective_disc_is_archive_eligible(
            &serde_json::json!({"status": "Questionable"})
        ));
        assert!(effective_disc_is_archive_eligible(
            &serde_json::json!({"status": "Unverified"})
        ));
        assert!(effective_disc_is_archive_eligible(
            &serde_json::json!({"status": "Verified"})
        ));
        assert!(effective_disc_is_active(
            &serde_json::json!({"status": "Questionable"})
        ));
    }

    #[test]
    fn generated_name_conflict_query_excludes_archive_ineligible_statuses() {
        assert!(GENERATED_NAME_CONFLICT_CANDIDATES_SQL
            .contains("d.status NOT IN ('Disabled', 'Questionable')"));
        assert!(!GENERATED_NAME_CONFLICT_CANDIDATES_SQL.contains("d.status <> 'Disabled'"));
    }

    #[test]
    fn submission_type_filter_conditions_use_status_and_target() {
        assert_eq!(submission_type_filter_condition(None), None);
        assert_eq!(submission_type_filter_condition(Some("")), None);
        assert_eq!(
            submission_type_filter_condition(Some("Edit")),
            Some("ds.submission_type = 'Edit'".to_string())
        );
        assert_eq!(
            submission_type_filter_condition(Some("New Disc")),
            Some(format!(
                "ds.submission_type = 'Disc' AND {UNPROCESSED_SUBMISSION_SQL} AND ds.target_disc_id IS NULL"
            ))
        );
        assert_eq!(
            submission_type_filter_condition(Some("Verification")),
            Some(format!(
                "ds.submission_type = 'Disc' AND (NOT ({UNPROCESSED_SUBMISSION_SQL}) OR ds.target_disc_id IS NOT NULL)"
            ))
        );
        assert_eq!(submission_type_filter_condition(Some("Disc")), None);
        assert_eq!(submission_type_filter_condition(Some("Unknown")), None);
    }

    #[test]
    fn submission_display_kind_sql_uses_status_and_target() {
        let sql = submission_display_kind_sql();

        assert!(sql.contains(UNPROCESSED_SUBMISSION_SQL));
        assert!(sql.contains("ds.target_disc_id IS NULL"));
        assert!(sql.contains("THEN 'Disc'"));
        assert!(!sql.contains("ds.changes"));
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
    fn resolver_canonicalizes_hex_case_without_rewriting_stored_changes() {
        let db = serde_json::json!({
            "disc_id": "aabbccdd",
            "disc_key": "aabbccdd",
            "universal_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "sbi": "MSF: 02:03:04 Q-Data: A1B2C3 0A:0B:0C 00 0D:0E:0F ABCD",
            "pvd": "0320 : AA BB                                           ab",
            "dat": "<rom name=\"Track.bin\" size=\"1\" crc=\"abcdef12\" md5=\"aabbccddeeff00112233445566778899\" sha1=\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\" />"
        });
        let changes = serde_json::json!({
            "disc_id": {"modify": {"old": "aabbccdd", "new": "AABBCCDD"}},
            "disc_key": {"modify": {"old": "aabbccdd", "new": "AABBCCDD"}},
            "universal_hash": {"modify": {"old": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "new": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}},
            "sbi": {"modify": {"old": "MSF: 02:03:04 Q-Data: A1B2C3 0A:0B:0C 00 0D:0E:0F ABCD", "new": "MSF: 02:03:04 Q-Data: a1b2c3 0a:0b:0c 00 0d:0e:0f abcd"}},
            "pvd": {"modify": {"old": "0320 : AA BB                                           ab", "new": "0320 : aa bb                                           ab"}},
            "dat": {"modify": {"old": db["dat"], "new": "<rom name=\"Track.bin\" size=\"1\" crc=\"ABCDEF12\" md5=\"AABBCCDDEEFF00112233445566778899\" sha1=\"ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD\" />"}}
        });
        let stored_changes = changes.clone();

        let resolved = resolve_submission_snapshot(&db, &changes).unwrap();

        assert_eq!(resolved, db);
        assert_eq!(changes, stored_changes);
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
    fn no_merge_when_mastering_identity_differs_only_by_case() {
        for (database_mastering, change_mastering) in [
            (("MASTER", "SID-1"), ("master", "SID-1")),
            (("MASTER", "SID-1"), ("MASTER", "sid-1")),
        ] {
            let old = serde_json::json!([ring_merge_candidate(
                database_mastering,
                ["10", "20", "30"],
                "same comment",
                "IFPI 1111",
            )]);
            let changes = serde_json::json!([ring_merge_change(
                change_mastering,
                ["10", "20", "30"],
                "same comment",
                "IFPI 2222",
            )]);

            let result = apply_ring_codes_history(&old, &changes).unwrap();
            assert_eq!(result.as_array().unwrap().len(), 2);
        }
    }

    #[test]
    fn no_merge_when_database_has_mastering_identity_on_omitted_change_layer() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [
                {
                    "mastering_code": "",
                    "mastering_sid": "",
                    "toolstamps": "",
                    "mould_sids": "IFPI 1111",
                    "additional_moulds": ""
                },
                {
                    "mastering_code": "L1-MASTER",
                    "mastering_sid": "",
                    "toolstamps": "",
                    "mould_sids": "",
                    "additional_moulds": ""
                }
            ]
        }]);
        let changes = serde_json::json!([{
            "layers": [{
                "index": 0,
                "mould_sids": { "add": { "new": "IFPI 2222" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        assert_eq!(result.as_array().unwrap().len(), 2);
    }

    #[test]
    fn merge_csv_values_preserves_case_variants_and_deduplicates_exact_values() {
        let old = serde_json::json!([{
            "id": 1,
            "offset_value": "",
            "offset_extra_value": "",
            "sample_start": "",
            "comment": "",
            "layers": [{
                "mastering_code": "X",
                "mastering_sid": "",
                "toolstamps": "STAMP",
                "mould_sids": "MOULD",
                "additional_moulds": "ADDITIONAL"
            }]
        }]);
        let changes = serde_json::json!([{
            "layers": [{
                "index": 0,
                "mastering_code": { "add": { "new": "X" } },
                "mastering_sid": { "add": { "new": "" } },
                "toolstamps": { "add": { "new": "stamp, STAMP" } },
                "mould_sids": { "add": { "new": "mould, MOULD" } },
                "additional_moulds": { "add": { "new": "additional, ADDITIONAL" } }
            }]
        }]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let layer = &entries[0]["layers"][0];
        assert_eq!(layer["toolstamps"], "STAMP, stamp");
        assert_eq!(layer["mould_sids"], "MOULD, mould");
        assert_eq!(layer["additional_moulds"], "ADDITIONAL, additional");
    }

    #[test]
    fn identified_merge_allows_empty_numeric_value_on_either_side() {
        for field_index in 0..3 {
            for empty_database_value in [false, true] {
                let mut database_numbers = ["10", "20", "30"];
                let mut change_numbers = database_numbers;
                if empty_database_value {
                    database_numbers[field_index] = "";
                } else {
                    change_numbers[field_index] = "";
                }

                let old = serde_json::json!([ring_merge_candidate(
                    ("MASTER", "SID-1"),
                    database_numbers,
                    "same comment",
                    "IFPI 1111",
                )]);
                let changes = serde_json::json!([ring_merge_change(
                    ("MASTER", "SID-1"),
                    change_numbers,
                    "same comment",
                    "IFPI 2222",
                )]);

                let result = apply_ring_codes_history(&old, &changes).unwrap();
                let entries = result.as_array().unwrap();
                assert_eq!(
                    entries.len(),
                    1,
                    "field {field_index} should allow an empty value on either side"
                );
                assert_eq!(
                    entries[0]["layers"][0]["mould_sids"],
                    "IFPI 1111, IFPI 2222"
                );
            }
        }
    }

    #[test]
    fn identified_merge_rejects_different_nonempty_numeric_values() {
        for field_index in 0..3 {
            let database_numbers = ["10", "20", "30"];
            let mut change_numbers = database_numbers;
            change_numbers[field_index] = "99";

            let old = serde_json::json!([ring_merge_candidate(
                ("MASTER", "SID-1"),
                database_numbers,
                "same comment",
                "IFPI 1111",
            )]);
            let changes = serde_json::json!([ring_merge_change(
                ("MASTER", "SID-1"),
                change_numbers,
                "same comment",
                "IFPI 2222",
            )]);

            let result = apply_ring_codes_history(&old, &changes).unwrap();
            assert_eq!(
                result.as_array().unwrap().len(),
                2,
                "field {field_index} should reject different non-empty values"
            );
        }
    }

    #[test]
    fn unidentified_merge_requires_exact_numeric_values() {
        let numbers = ["10", "20", "30"];
        let old = serde_json::json!([ring_merge_candidate(
            ("", ""),
            numbers,
            "same comment",
            "IFPI 1111",
        )]);
        let changes = serde_json::json!([ring_merge_change(
            ("", ""),
            numbers,
            "same comment",
            "IFPI 2222",
        )]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        let entries = result.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0]["layers"][0]["mould_sids"],
            "IFPI 1111, IFPI 2222"
        );

        for field_index in 0..3 {
            for empty_database_value in [false, true] {
                let mut database_numbers = numbers;
                let mut change_numbers = numbers;
                if empty_database_value {
                    database_numbers[field_index] = "";
                } else {
                    change_numbers[field_index] = "";
                }

                let old = serde_json::json!([ring_merge_candidate(
                    ("", ""),
                    database_numbers,
                    "same comment",
                    "IFPI 1111",
                )]);
                let changes = serde_json::json!([ring_merge_change(
                    ("", ""),
                    change_numbers,
                    "same comment",
                    "IFPI 2222",
                )]);

                let result = apply_ring_codes_history(&old, &changes).unwrap();
                assert_eq!(
                    result.as_array().unwrap().len(),
                    2,
                    "field {field_index} should require exact values without mastering identity"
                );
            }
        }
    }

    #[test]
    fn unidentified_entries_with_unknown_offsets_do_not_merge() {
        let old = serde_json::json!([ring_merge_candidate(
            ("", ""),
            ["", "", ""],
            "",
            "IFPI 1234",
        )]);
        let changes =
            serde_json::json!([ring_merge_change(("", ""), ["2", "", ""], "", "IFPI 5432",)]);

        let result = apply_ring_codes_history(&old, &changes).unwrap();
        assert_eq!(result.as_array().unwrap().len(), 2);
    }

    #[test]
    fn ring_merge_requires_exact_comments_in_both_modes() {
        for mastering in [("MASTER", "SID-1"), ("", "")] {
            for (database_comment, change_comment) in [
                ("Pressing A", "Pressing B"),
                ("", "Pressing A"),
                ("Pressing A", ""),
                ("Pressing A", "pressing A"),
                ("Pressing A", "Pressing A "),
            ] {
                let old = serde_json::json!([ring_merge_candidate(
                    mastering,
                    ["10", "20", "30"],
                    database_comment,
                    "IFPI 1111",
                )]);
                let changes = serde_json::json!([ring_merge_change(
                    mastering,
                    ["10", "20", "30"],
                    change_comment,
                    "IFPI 2222",
                )]);

                let result = apply_ring_codes_history(&old, &changes).unwrap();
                assert_eq!(
                    result.as_array().unwrap().len(),
                    2,
                    "comments must match exactly for mastering {mastering:?}"
                );
            }
        }
    }

    #[test]
    fn matching_nonempty_comments_merge_in_both_modes() {
        for mastering in [("MASTER", "SID-1"), ("", "")] {
            let old = serde_json::json!([ring_merge_candidate(
                mastering,
                ["10", "20", "30"],
                "same comment",
                "IFPI 1111",
            )]);
            let changes = serde_json::json!([ring_merge_change(
                mastering,
                ["10", "20", "30"],
                "same comment",
                "IFPI 2222",
            )]);

            let result = apply_ring_codes_history(&old, &changes).unwrap();
            assert_eq!(result.as_array().unwrap().len(), 1);
        }
    }
}
