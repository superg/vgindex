use sqlx::PgPool;

use crate::db::models::*;
use crate::error::{AppError, AppResult};
use crate::services::disc_service;

pub async fn create_submission(
    pool: &PgPool,
    sub_type: SubmissionType,
    submitter_id: i32,
    target_disc_id: Option<i32>,
    data: serde_json::Value,
    submission_comment: Option<&str>,
    dump_log: Option<&str>,
    extra_upload_url: Option<&str>,
) -> AppResult<DiscSubmission> {
    let sub: DiscSubmission = sqlx::query_as(
        "INSERT INTO disc_submissions (submission_type, submitter_id, submission_comment, target_disc_id, data, dump_log, extra_upload_url)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING *"
    )
    .bind(sub_type)
    .bind(submitter_id)
    .bind(submission_comment)
    .bind(target_disc_id)
    .bind(&data)
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

/// Atomically reject a submission.  Returns `true` if the rejection was
/// applied, `false` if the submission was already processed by another
/// moderator (race condition).
pub async fn reject_submission(
    pool: &PgPool,
    id: i32,
    reviewer_id: i32,
    review_comment: Option<&str>,
) -> AppResult<bool> {
    let result = sqlx::query(
        "UPDATE disc_submissions SET status = 'Rejected', reviewer_id = $1,
         review_comment = $2, reviewed_at = NOW()
         WHERE id = $3 AND status = 'Pending'",
    )
    .bind(reviewer_id)
    .bind(review_comment)
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
    data: &serde_json::Value,
    reviewer_id: i32,
    review_comment: Option<&str>,
) -> AppResult<Option<i32>> {
    // Atomically claim the submission by setting status = 'Approved'
    // only when it is still 'Pending'.  If another moderator already
    // processed it, rows_affected will be 0.
    let claim = sqlx::query(
        "UPDATE disc_submissions SET status = 'Approved', reviewer_id = $1,
         review_comment = $2, reviewed_at = NOW(), data = $3
         WHERE id = $4 AND status = 'Pending'",
    )
    .bind(reviewer_id)
    .bind(review_comment)
    .bind(data)
    .bind(sub.id)
    .execute(pool)
    .await?;

    if claim.rows_affected() == 0 {
        return Ok(None);
    }

    let disc_id = if let Some(existing_id) = sub.target_disc_id {
        disc_service::update_disc(pool, existing_id, data).await?;

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
            data,
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
        conditions.push(format!("COALESCE(d.system_code, ds.data->>'system_code') = ${idx}"));
    }
    if submitter_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("u.username = ${idx}"));
    }

    let sort_col = match sort_column {
        "date"      => "ds.created_at",
        "title"     => "LOWER(COALESCE(d.title, ds.data->>'title', 'Untitled'))",
        "system"    => "LOWER(COALESCE(d.system_code, ds.data->>'system_code', ''))",
        "submitter" => "LOWER(u.username)",
        "reviewer"  => "LOWER(COALESCE(ur.username, ''))",
        "type"      => "ds.submission_type",
        "status"    => "ds.status",
        _           => "ds.created_at",
    };
    let sort_dir = if sort_order == "asc" { "ASC" } else { "DESC" };

    let sql = format!(
        "SELECT ds.id, ds.submission_type,
                COALESCE(d.title, ds.data->>'title', 'Untitled') AS title,
                COALESCE(d.system_code, ds.data->>'system_code', '') AS system_code,
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
         WHERE {}
         ORDER BY {sort_col} {sort_dir}
         LIMIT {page_size} OFFSET {offset}",
        conditions.join(" AND ")
    );

    let mut query = sqlx::query_as::<_, SubmissionListRow>(&sql);
    if let Some(uid) = user_id_filter {
        query = query.bind(uid);
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
        conditions.push(format!("COALESCE(d.system_code, ds.data->>'system_code') = ${idx}"));
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
