use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{Html, Redirect},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;

use crate::auth::middleware::{RequireAuth, RequireModerator};
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::AppResult;
use crate::services::submission_service;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/submissions", get(submissions_list))
        .route("/submissions/", get(submissions_list))
        .route("/submissions/{id}", get(submission_detail))
        .route("/submissions/{id}/", get(submission_detail))
        .route("/submissions/{id}/review", post(review_submission))
}

#[derive(Deserialize, Default)]
pub struct SubmissionsQuery {
    pub status: Option<String>,
    pub sub_type: Option<String>,
    pub page: Option<i64>,
}

#[derive(Template)]
#[template(path = "submissions.html")]
struct SubmissionsTemplate {
    current_user: Option<String>,
    is_moderator: bool,
    submissions: Vec<SubmissionListRow>,
    query: SubmissionsQuery,
    page: i64,
    total_pages: i64,
}
impl SiteConfig for SubmissionsTemplate {}

#[derive(Template)]
#[template(path = "submission_detail.html")]
struct SubmissionDetailTemplate {
    current_user: Option<String>,
    is_moderator: bool,
    is_pending: bool,
    sub_id: i32,
    sub_type: String,
    submitter_name: String,
    status_class: String,
    status_display: String,
    created_at: String,
    disc_title: String,
    has_target_disc: bool,
    target_disc_id: i32,
    review_comment: String,
    data_json: String,
    dump_log: String,
}
impl SiteConfig for SubmissionDetailTemplate {}

async fn submissions_list(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Query(query): Query<SubmissionsQuery>,
) -> AppResult<Html<String>> {
    let page = query.page.unwrap_or(1).max(1);
    let is_mod = user.role.can_moderate();

    let submissions = submission_service::list_submissions(
        &state.pool,
        if is_mod { None } else { Some(user.id) },
        query.status.as_deref(),
        query.sub_type.as_deref(),
        page,
        50,
    ).await?;

    let total = submission_service::count_submissions(
        &state.pool,
        if is_mod { None } else { Some(user.id) },
        query.status.as_deref(),
        query.sub_type.as_deref(),
    ).await?;

    Ok(Html(
        SubmissionsTemplate {
            current_user: Some(user.username),
            is_moderator: is_mod,
            submissions,
            query,
            page,
            total_pages: (total + 49) / 50,
        }
        .render()
        .unwrap(),
    ))
}

async fn submission_detail(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let sub: DiscSubmission = sqlx::query_as("SELECT * FROM disc_submissions WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(crate::error::AppError::NotFound)?;

    if !user.role.can_moderate() && sub.submitter_id != user.id {
        return Err(crate::error::AppError::Forbidden);
    }

    let submitter_name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(sub.submitter_id)
        .fetch_one(&state.pool)
        .await?;

    let disc_title = if let Some(disc_id) = sub.target_disc_id {
        sqlx::query_scalar::<_, String>("SELECT title FROM discs WHERE id = $1")
            .bind(disc_id)
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or_default()
    } else {
        sub.data.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string()
    };

    Ok(Html(
        SubmissionDetailTemplate {
            current_user: Some(user.username),
            is_moderator: user.role.can_moderate(),
            is_pending: sub.status == SubmissionStatus::Pending,
            sub_id: sub.id,
            sub_type: sub.submission_type.to_string(),
            submitter_name,
            status_class: sub.status.css_class().to_string(),
            status_display: sub.status.to_string(),
            created_at: sub.created_at.format("%Y-%m-%d %H:%M").to_string(),
            disc_title,
            has_target_disc: sub.target_disc_id.is_some(),
            target_disc_id: sub.target_disc_id.unwrap_or(0),
            review_comment: sub.review_comment.unwrap_or_default(),
            data_json: serde_json::to_string_pretty(&sub.data).unwrap_or_default(),
            dump_log: sub.dump_log.unwrap_or_default(),
        }
        .render()
        .unwrap(),
    ))
}

#[derive(Deserialize)]
pub struct ReviewForm {
    pub action: String,
    pub comment: Option<String>,
}

async fn review_submission(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Path(id): Path<i32>,
    Form(form): Form<ReviewForm>,
) -> AppResult<Redirect> {
    let new_status = match form.action.as_str() {
        "approve" => SubmissionStatus::Approved,
        "deny" => SubmissionStatus::Denied,
        _ => return Err(crate::error::AppError::BadRequest("Invalid action".into())),
    };

    submission_service::review_submission(
        &state.pool,
        id,
        user.id,
        new_status,
        form.comment.as_deref(),
    ).await?;

    Ok(Redirect::to(&format!("/submissions/{id}/")))
}
