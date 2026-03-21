use askama::Template;
use axum::{
    extract::{Path, State},
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
        .route("/disc/{id}/edit/", get(edit_page).post(edit_submit))
}

#[derive(Template)]
#[template(path = "disc_edit.html")]
struct DiscEditTemplate {
    current_user: Option<String>,
    disc_id: i32,
    disc_title: String,
    categories: Vec<SelectOption>,
    regions: Vec<CheckOption>,
    languages: Vec<CheckOption>,
    barcode: String,
    comments: String,
    version: String,
    edition: String,
    exe_date: String,
    show_date_field: bool,
    edc_value: String,
    show_edc_field: bool,
    protection: String,
    error_count: String,
}

struct SelectOption {
    id: i32,
    value: String,
    name: String,
    selected: bool,
}

struct CheckOption {
    id: i32,
    name: String,
    flag_lower: String,
    selected: bool,
}

async fn edit_page(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
) -> AppResult<Html<String>> {
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;

    let all_regions: Vec<Region> =
        sqlx::query_as("SELECT * FROM regions ORDER BY display_order")
            .fetch_all(&state.pool).await?;
    let langs: Vec<Language> =
        sqlx::query_as("SELECT * FROM languages ORDER BY display_order")
            .fetch_all(&state.pool).await?;

    let disc_region_ids: Vec<i32> = sqlx::query_scalar(
        "SELECT region_id FROM disc_regions WHERE disc_id = $1"
    ).bind(id).fetch_all(&state.pool).await?;

    let disc_lang_ids: Vec<i32> = sqlx::query_scalar(
        "SELECT language_id FROM disc_languages WHERE disc_id = $1"
    ).bind(id).fetch_all(&state.pool).await?;

    let categories: Vec<SelectOption> = Category::ALL.iter().map(|c| SelectOption {
        id: 0,
        value: c.to_string(),
        name: c.to_string(),
        selected: detail.disc.category == *c,
    }).collect();

    let regions: Vec<CheckOption> = all_regions.iter().map(|r| CheckOption {
        id: r.id,
        name: r.name.clone(),
        flag_lower: r.flag_code.to_lowercase(),
        selected: disc_region_ids.contains(&r.id),
    }).collect();

    let languages: Vec<CheckOption> = langs.iter().map(|l| CheckOption {
        id: l.id,
        name: l.name.clone(),
        flag_lower: l.flag_code.to_lowercase(),
        selected: disc_lang_ids.contains(&l.id),
    }).collect();

    Ok(Html(
        DiscEditTemplate {
            current_user: Some(user.username),
            disc_id: id,
            disc_title: detail.disc.title.clone(),
            categories,
            regions,
            languages,
            barcode: detail.disc.barcode.unwrap_or_default(),
            comments: detail.disc.comments.unwrap_or_default(),
            version: detail.disc.version.unwrap_or_default(),
            edition: detail.disc.edition.unwrap_or_default(),
            exe_date: detail.disc.exe_date.map(|d| d.to_string()).unwrap_or_default(),
            show_date_field: detail.system.has_date_field,
            edc_value: detail.disc.edc.map(|e| e.to_string()).unwrap_or_default(),
            show_edc_field: detail.system.has_edc_field,
            protection: detail.disc.protection.unwrap_or_default(),
            error_count: detail.disc.error_count.map(|e| e.to_string()).unwrap_or("0".to_string()),
        }
        .render()
        .unwrap(),
    ))
}

#[derive(Deserialize)]
pub struct DiscEditForm {
    pub title: String,
    pub category: String,
    pub version: Option<String>,
    pub edition: Option<String>,
    pub barcode: Option<String>,
    pub comments: Option<String>,
    pub exe_date: Option<String>,
    pub edc: Option<String>,
    pub protection: Option<String>,
    pub error_count: Option<i32>,
    #[serde(default)]
    pub regions: Vec<i32>,
    #[serde(default)]
    pub languages: Vec<i32>,
}

async fn edit_submit(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    Path(id): Path<i32>,
    Form(form): Form<DiscEditForm>,
) -> AppResult<Redirect> {
    let data = serde_json::json!({
        "title": form.title,
        "category": form.category,
        "version": form.version,
        "edition": form.edition,
        "barcode": form.barcode,
        "comments": form.comments,
        "exe_date": form.exe_date,
        "edc": form.edc,
        "protection": form.protection,
        "error_count": form.error_count,
        "regions": form.regions,
        "languages": form.languages,
    });

    if user.role.can_edit_directly() {
        disc_service::update_disc(&state.pool, id, &data).await?;
    } else {
        submission_service::create_submission(
            &state.pool,
            SubmissionType::Edit,
            user.id,
            Some(id),
            data,
            None,
            None,
        ).await?;
    }

    Ok(Redirect::to(&format!("/disc/{id}/")))
}
