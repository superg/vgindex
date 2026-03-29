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
    dump_log: Option<&str>,
    extra_upload_url: Option<&str>,
) -> AppResult<DiscSubmission> {
    let sub: DiscSubmission = sqlx::query_as(
        "INSERT INTO disc_submissions (submission_type, submitter_id, target_disc_id, changes, dump_log, extra_upload_url)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING *"
    )
    .bind(sub_type)
    .bind(submitter_id)
    .bind(target_disc_id)
    .bind(&data)
    .bind(dump_log)
    .bind(extra_upload_url)
    .fetch_one(pool)
    .await?;

    Ok(sub)
}

pub async fn find_matching_disc(pool: &PgPool, data: &serde_json::Value) -> Option<i32> {
    if let Some(files_xml) = data["files_xml"].as_str() {
        for line in files_xml.lines() {
            let line = line.trim();
            if !line.starts_with("<rom ") {
                continue;
            }
            if let Some(sha1) = extract_xml_attr(line, "sha1") {
                let disc_id: Option<i32> = sqlx::query_scalar(
                    "SELECT disc_id FROM files WHERE sha1 = $1 LIMIT 1"
                )
                .bind(&sha1)
                .fetch_optional(pool)
                .await
                .unwrap_or(None);

                if disc_id.is_some() {
                    return disc_id;
                }
            }
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

pub async fn mark_submission_approved(
    pool: &PgPool,
    submission_id: i32,
    reviewer_id: i32,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE disc_submissions SET status = 'Approved', reviewer_id = $1, reviewed_at = NOW()
         WHERE id = $2"
    )
    .bind(reviewer_id)
    .bind(submission_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn review_submission(
    pool: &PgPool,
    submission_id: i32,
    reviewer_id: i32,
    new_status: SubmissionStatus,
    comment: Option<&str>,
) -> AppResult<()> {
    let sub: DiscSubmission = sqlx::query_as("SELECT * FROM disc_submissions WHERE id = $1")
        .bind(submission_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    if sub.status != SubmissionStatus::Pending {
        return Err(AppError::BadRequest("Submission already reviewed".into()));
    }

    if new_status == SubmissionStatus::Approved {
        match sub.submission_type {
            SubmissionType::Disc => {
                if let Some(disc_id) = sub.target_disc_id {
                    // Verification: add submitter as dumper.
                    sqlx::query(
                        "INSERT INTO disc_dumpers (disc_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
                    )
                    .bind(disc_id)
                    .bind(sub.submitter_id)
                    .execute(pool)
                    .await?;
                } else {
                    // New disc: create from submission payload.
                    let disc_id = disc_service::create_disc_from_submission(
                        pool,
                        &sub.changes,
                        sub.submitter_id,
                    )
                    .await?;

                    sqlx::query(
                        "UPDATE disc_submissions SET status = $1, reviewer_id = $2,
                         review_comment = $3, reviewed_at = NOW(), target_disc_id = $4
                         WHERE id = $5"
                    )
                    .bind(new_status)
                    .bind(reviewer_id)
                    .bind(comment)
                    .bind(disc_id)
                    .bind(submission_id)
                    .execute(pool)
                    .await?;
                    return Ok(());
                }
            }
            SubmissionType::Edit => {
                if let Some(disc_id) = sub.target_disc_id {
                    disc_service::update_disc(pool, disc_id, &sub.changes).await?;
                }
            }
        }
    }

    sqlx::query(
        "UPDATE disc_submissions SET status = $1, reviewer_id = $2,
         review_comment = $3, reviewed_at = NOW()
         WHERE id = $4"
    )
    .bind(new_status)
    .bind(reviewer_id)
    .bind(comment)
    .bind(submission_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn list_submissions(
    pool: &PgPool,
    user_id_filter: Option<i32>,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
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

    let sql = format!(
        "SELECT ds.id, ds.submission_type, COALESCE(d.title, ds.changes->>'title', 'Untitled') AS title,
                u.username AS submitter, ds.status, ds.review_comment, ds.created_at
         FROM disc_submissions ds
         JOIN users u ON u.id = ds.submitter_id
         LEFT JOIN discs d ON d.id = ds.target_disc_id
         WHERE {}
         ORDER BY ds.created_at DESC
         LIMIT {} OFFSET {}",
        conditions.join(" AND "),
        page_size,
        offset
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

    Ok(query.fetch_all(pool).await?)
}

pub async fn count_submissions(
    pool: &PgPool,
    user_id_filter: Option<i32>,
    status_filter: Option<&str>,
    type_filter: Option<&str>,
) -> AppResult<i64> {
    let mut conditions = vec!["1=1".to_string()];
    let mut idx = 0u32;

    if user_id_filter.is_some() {
        idx += 1;
        conditions.push(format!("submitter_id = ${idx}"));
    }
    if status_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("status::text = ${idx}"));
    }
    if type_filter.is_some_and(|s| !s.is_empty()) {
        idx += 1;
        conditions.push(format!("submission_type::text = ${idx}"));
    }

    let sql = format!(
        "SELECT COUNT(*) FROM disc_submissions WHERE {}",
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

    Ok(query.fetch_one(pool).await?)
}

// Manual FromRow for SubmissionListRow since it's from a custom query
impl sqlx::FromRow<'_, sqlx::postgres::PgRow> for SubmissionListRow {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            id: row.try_get("id")?,
            submission_type: row.try_get("submission_type")?,
            title: row.try_get("title")?,
            submitter: row.try_get("submitter")?,
            status: row.try_get("status")?,
            review_comment: row.try_get("review_comment")?,
            created_at: row.try_get("created_at")?,
        })
    }
}
