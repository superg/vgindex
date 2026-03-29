use askama::Template;
use axum::{
    extract::State,
    response::{Html, Redirect},
    routing::get,
    Form, Router,
};
use serde::Deserialize;

use crate::auth::middleware::RequireAuth;
use crate::config::SiteConfig;
use crate::db::models::*;
use crate::error::AppResult;
use crate::services::{disc_service, submission_service};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/disc/submit", get(submit_page).post(submit_handler))
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
impl SiteConfig for DiscSubmitTemplate {}

struct MediaTypeOption {
    code: String,
    name: String,
}

struct SubmitRegion {
    code: String,
    flag_code: String,
    name: String,
}

struct SubmitLang {
    code: String,
    flag_code: String,
    name: String,
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
    let all_media_types: Vec<MediaTypeRow> =
        sqlx::query_as("SELECT code, name, layer_count, rom_extension FROM media_types ORDER BY name")
            .fetch_all(&state.pool).await?;

    Ok(Html(
        DiscSubmitTemplate {
            current_user: Some(user.username),
            systems,
            regions: all_regions.iter().map(|r| SubmitRegion {
                code: r.code.trim().to_string(),
                flag_code: r.flag_code.trim().to_lowercase(),
                name: r.name.clone(),
            }).collect(),
            languages: langs.iter().map(|l| SubmitLang {
                code: l.code.trim().to_string(),
                flag_code: l.flag_code.trim().to_lowercase(),
                name: l.name.clone(),
            }).collect(),
            categories: Category::ALL.iter().map(|c| c.to_string()).collect(),
            media_types: all_media_types.iter().map(|m| MediaTypeOption {
                code: m.code.clone(),
                name: m.name.clone(),
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
    pub pvd: Option<String>,
    pub header: Option<String>,
    pub files_xml: Option<String>,
    pub cue_content: Option<String>,
    pub dump_log: Option<String>,
    #[serde(default)]
    pub regions: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
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
        "pvd": form.pvd,
        "header": form.header,
        "files_xml": form.files_xml,
        "cue_content": form.cue_content,
        "regions": form.regions,
        "languages": form.languages,
    });

    let target_disc_id = submission_service::find_matching_disc(&state.pool, &data).await;

    let sub = submission_service::create_submission(
        &state.pool,
        SubmissionType::Disc,
        user.id,
        target_disc_id,
        data,
        form.dump_log.as_deref(),
        None,
    ).await?;

    Ok(Redirect::to(&format!("/submissions/{}/", sub.id)))
}
