use askama::Template;
use axum::{
    extract::State,
    response::{Html, Redirect},
    routing::get,
    Form, Router,
};
use serde::Deserialize;

use crate::auth::middleware::RequireAuth;
use crate::db::models::*;
use crate::error::AppResult;
use crate::services::{disc_service, submission_service};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/disc/submit/", get(submit_page).post(submit_handler))
}

#[derive(Template)]
#[template(path = "disc_submit.html")]
struct DiscSubmitTemplate {
    current_user: Option<String>,
    systems: Vec<System>,
    regions: Vec<SubmitRegion>,
    languages: Vec<SubmitLang>,
    categories: Vec<String>,
    media_types: Vec<MediaTypeOption>,
}

struct MediaTypeOption {
    code: String,
    name: String,
}

struct SubmitRegion {
    code: String,
    name: String,
    flag_lower: String,
}

struct SubmitLang {
    id: i32,
    name: String,
    flag_lower: String,
}

async fn submit_page(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
) -> AppResult<Html<String>> {
    let systems = disc_service::get_all_systems(&state.pool).await?;
    let all_regions: Vec<Region> =
        sqlx::query_as("SELECT * FROM regions ORDER BY sort_order")
            .fetch_all(&state.pool).await?;
    let langs: Vec<Language> =
        sqlx::query_as("SELECT * FROM languages ORDER BY sort_order")
            .fetch_all(&state.pool).await?;

    Ok(Html(
        DiscSubmitTemplate {
            current_user: Some(user.username),
            systems,
            regions: all_regions.iter().map(|r| SubmitRegion {
                code: r.code.trim().to_string(),
                name: r.name.clone(),
                flag_lower: r.flag_code.to_lowercase(),
            }).collect(),
            languages: langs.iter().map(|l| SubmitLang {
                id: l.id,
                name: l.name.clone(),
                flag_lower: l.flag_code.to_lowercase(),
            }).collect(),
            categories: Category::ALL.iter().map(|c| c.to_string()).collect(),
            media_types: MediaType::ALL.iter().map(|m| MediaTypeOption {
                code: m.code().to_string(),
                name: m.to_string(),
            }).collect(),
        }
        .render()
        .unwrap(),
    ))
}

#[derive(Deserialize)]
pub struct DiscSubmitForm {
    pub system_code: String,
    pub media_type: String,
    pub title: String,
    pub category: String,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub barcode: Option<String>,
    pub comments: Option<String>,
    pub filename_suffix: Option<String>,
    pub exe_date: Option<String>,
    pub protection: Option<String>,
    pub error_count: Option<i32>,
    pub files_xml: Option<String>,
    pub cue_content: Option<String>,
    pub dump_log: Option<String>,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub languages: Vec<i32>,
}

async fn submit_handler(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Form(form): Form<DiscSubmitForm>,
) -> AppResult<Redirect> {
    let data = serde_json::json!({
        "system_code": form.system_code,
        "media_type": form.media_type,
        "title": form.title,
        "category": form.category,
        "version": form.version,
        "edition": form.edition,
        "barcode": form.barcode,
        "comments": form.comments,
        "filename_suffix": form.filename_suffix,
        "exe_date": form.exe_date,
        "protection": form.protection,
        "error_count": form.error_count,
        "files_xml": form.files_xml,
        "cue_content": form.cue_content,
        "regions": form.regions,
        "languages": form.languages,
    });

    let sub_type = submission_service::detect_submission_type(&state.pool, &data).await;
    let target_disc_id = if sub_type == SubmissionType::Verification {
        submission_service::find_matching_disc(&state.pool, &data).await
    } else {
        None
    };

    let sub = submission_service::create_submission(
        &state.pool,
        sub_type,
        user.id,
        target_disc_id,
        data,
        form.dump_log.as_deref(),
        None,
    ).await?;

    Ok(Redirect::to(&format!("/submissions/{}/", sub.id)))
}
