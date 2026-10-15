use askama::Template;
use axum::{
    extract::{Path as AxumPath, Query, Request, State},
    http::{header, HeaderValue},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use axum_extra::extract::Form;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::auth::{
    csrf::{self, CsrfForm},
    middleware::{AuthenticatedUser, RequireAdmin, RequireModerator},
};
use crate::config::SiteConfig;
use crate::db::models::{SubmissionStatus, SubmissionType, System};
use crate::error::{AppError, AppResult};
use crate::services::{archive_service, database_export_service, disc_service};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/maintenance", get(maintenance_page))
        .route("/maintenance/systems", post(save_systems))
        .route("/maintenance/regions", post(save_regions))
        .route("/maintenance/languages", post(save_languages))
        .route("/maintenance/rebuild-cue", post(rebuild_cue))
        .route(
            "/maintenance/trigger-archive-generation",
            post(trigger_archive_generation),
        )
        .route("/maintenance/users/{user_id}/logout", post(logout_user))
        .route("/maintenance/users/{user_id}/rename", post(rename_user))
        .route("/maintenance/users/{user_id}/delete", post(delete_user))
        .route(
            "/maintenance/submissions/{submission_id}",
            post(save_submission),
        )
        .route("/maintenance/backups/{filename}", get(download_backup))
}

const BACKUP_DIR: &str = "./data/backups";
const BACKUP_PREFIX: &str = "vgindex-backup-";
const BACKUP_SUFFIX: &str = ".tar.gz";
const DELETED_USERNAME: &str = "Deleted";

#[derive(Deserialize, Default)]
struct MaintenanceQuery {
    status: Option<String>,
    error: Option<String>,
    tab: Option<String>,
    submission_id: Option<String>,
}

#[derive(Template)]
#[template(path = "maintenance.html")]
struct MaintenanceTemplate {
    current_user: Option<AuthenticatedUser>,
    status_message: String,
    error_message: String,
    maintenance_errors: Vec<String>,
    system_rows: Vec<SystemEditorRow>,
    region_rows: Vec<LookupEditorRow>,
    language_rows: Vec<LookupEditorRow>,
    flag_options: Vec<FlagOption>,
    system_input_sizes: SystemInputSizes,
    region_input_sizes: LookupInputSizes,
    language_input_sizes: LookupInputSizes,
    user_rows: Vec<MaintenanceUser>,
    submission_users: Vec<UserIdentity>,
    submission_lookup_id: String,
    submission_editor: Option<MaintenanceSubmissionEditor>,
    backup_files: Vec<BackupFile>,
    can_admin: bool,
    show_general: bool,
    show_systems: bool,
    show_regions: bool,
    show_languages: bool,
    show_misc: bool,
    show_users: bool,
    show_submissions: bool,
    show_backup: bool,
}
impl SiteConfig for MaintenanceTemplate {}

struct BackupFile {
    filename: String,
    created_at: String,
    size: String,
}

#[derive(sqlx::FromRow)]
struct MaintenanceUser {
    id: i32,
    username: String,
    active_session_count: i64,
}

#[derive(Clone, sqlx::FromRow)]
struct UserIdentity {
    id: i32,
    username: String,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct MaintenanceSubmissionRow {
    id: i32,
    submission_type: SubmissionType,
    submitter_id: i32,
    submission_comment: Option<String>,
    target_disc_id: Option<i32>,
    changes_original: Option<serde_json::Value>,
    changes: serde_json::Value,
    dump_log: Option<String>,
    extra_upload_url: Option<String>,
    submission_token: Option<String>,
    submission_fingerprint: Option<String>,
    status: SubmissionStatus,
    reviewer_id: Option<i32>,
    review_comment: Option<String>,
    created_at: DateTime<Utc>,
    reviewed_at: Option<DateTime<Utc>>,
    row_version: String,
}

#[derive(Clone, Debug, Default)]
struct MaintenanceSubmissionEditor {
    id: i32,
    row_version: String,
    submission_type: String,
    submitter_id: String,
    submitter_username: String,
    submission_comment: String,
    target_disc_id: String,
    changes_original: String,
    changes: String,
    dump_log: String,
    extra_upload_url: String,
    submission_token: String,
    submission_fingerprint: String,
    status: String,
    reviewer_id: String,
    reviewer_username: String,
    review_comment: String,
    created_at: String,
    reviewed_at: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SubmissionMaintenanceForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    row_version: String,
    #[serde(default)]
    submission_type: String,
    #[serde(default)]
    submitter_id: String,
    #[serde(default)]
    submitter_username: String,
    #[serde(default)]
    submission_comment: String,
    #[serde(default)]
    target_disc_id: String,
    #[serde(default)]
    changes_original: String,
    #[serde(default)]
    changes: String,
    #[serde(default)]
    dump_log: String,
    #[serde(default)]
    extra_upload_url: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    reviewer_id: String,
    #[serde(default)]
    reviewer_username: String,
    #[serde(default)]
    review_comment: String,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    reviewed_at: String,
}

#[derive(Clone, Debug)]
struct ValidatedSubmissionUpdate {
    row_version: String,
    submission_type: SubmissionType,
    submitter_id: i32,
    submission_comment: Option<String>,
    target_disc_id: Option<i32>,
    changes_original: Option<serde_json::Value>,
    changes: serde_json::Value,
    dump_log: Option<String>,
    extra_upload_url: Option<String>,
    status: SubmissionStatus,
    reviewer_id: Option<i32>,
    review_comment: Option<String>,
    created_at: DateTime<Utc>,
    reviewed_at: Option<DateTime<Utc>>,
}

enum SubmissionUpdateOutcome {
    Updated(Vec<&'static str>),
    Unchanged,
    NotFound,
    Stale,
}

#[derive(Deserialize)]
struct RenameUserForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    new_username: String,
}

#[derive(Deserialize)]
struct DeleteUserForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    confirm_username: String,
}

async fn maintenance_page(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Query(query): Query<MaintenanceQuery>,
) -> AppResult<Html<String>> {
    if matches!(
        query.tab.as_deref(),
        Some("users" | "submissions" | "backup")
    ) && !user.role.can_admin()
    {
        return Err(AppError::Forbidden);
    }

    let template = build_maintenance_template(
        &state.pool,
        user,
        query,
        Vec::new(),
        MaintenanceOverrides::default(),
    )
    .await?;
    Ok(Html(template.render().unwrap()))
}

async fn logout_user(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    AxumPath(user_id): AxumPath<i32>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    csrf::verify_form(&admin, &form)?;

    let Some(target) = fetch_user_identity(&state.pool, user_id).await? else {
        return Ok(redirect_with_message(
            "users",
            "error",
            "User was not found.",
        ));
    };

    let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(&state.pool)
        .await?;
    let revoked = result.rows_affected();

    tracing::info!(
        administrator_id = admin.id,
        administrator_username = %admin.username,
        target_user_id = target.id,
        target_username = %target.username,
        sessions_revoked = revoked,
        "Administrator revoked all app sessions for user"
    );

    Ok(redirect_with_message(
        "users",
        "status",
        &format!(
            "Logged out {} from {revoked} app {}.",
            target.username,
            if revoked == 1 { "session" } else { "sessions" }
        ),
    ))
}

enum RenameUserResult {
    Renamed { old_username: String },
    Unchanged,
    NotFound,
    Conflict { username: String },
}

async fn rename_user(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    AxumPath(user_id): AxumPath<i32>,
    Form(form): Form<RenameUserForm>,
) -> AppResult<Response> {
    csrf::verify_token(&admin, &form.csrf_token)?;

    let new_username = match normalize_maintenance_username(&form.new_username) {
        Ok(username) => username,
        Err(message) => return Ok(redirect_with_message("users", "error", message)),
    };

    match rename_app_user(&state.pool, user_id, &new_username).await? {
        RenameUserResult::Renamed { old_username } => {
            tracing::info!(
                administrator_id = admin.id,
                administrator_username = %admin.username,
                target_user_id = user_id,
                old_username = %old_username,
                new_username = %new_username,
                "Administrator renamed app user"
            );
            Ok(redirect_with_message(
                "users",
                "status",
                &format!("Renamed {old_username} to {new_username}."),
            ))
        }
        RenameUserResult::Unchanged => Ok(redirect_with_message(
            "users",
            "status",
            &format!("User is already named {new_username}."),
        )),
        RenameUserResult::NotFound => Ok(redirect_with_message(
            "users",
            "error",
            "User was not found.",
        )),
        RenameUserResult::Conflict { username } => Ok(redirect_with_message(
            "users",
            "error",
            &format!("Username {username} already belongs to another app user."),
        )),
    }
}

struct DeletedUserSummary {
    username: String,
    disc_credits: u64,
    duplicate_disc_credits: u64,
    submission_attributions: u64,
    review_attributions: u64,
    sessions_revoked: i64,
}

enum DeleteUserResult {
    Deleted(DeletedUserSummary),
    NotFound,
    Protected,
    ConfirmationMismatch { username: String },
}

async fn delete_user(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    AxumPath(user_id): AxumPath<i32>,
    Form(form): Form<DeleteUserForm>,
) -> AppResult<Response> {
    csrf::verify_token(&admin, &form.csrf_token)?;

    match delete_app_user(&state.pool, user_id, &form.confirm_username).await? {
        DeleteUserResult::Deleted(summary) => {
            state.discs_cache.invalidate_dumper_data().await;
            tracing::info!(
                administrator_id = admin.id,
                administrator_username = %admin.username,
                target_user_id = user_id,
                target_username = %summary.username,
                disc_credits_transferred = summary.disc_credits,
                duplicate_disc_credits_merged = summary.duplicate_disc_credits,
                submission_attributions_transferred = summary.submission_attributions,
                review_attributions_transferred = summary.review_attributions,
                sessions_revoked = summary.sessions_revoked,
                "Administrator transferred user attribution and deleted app user"
            );
            Ok(redirect_with_message(
                "users",
                "status",
                &format!(
                    "Deleted {}. Transferred {} disc credit(s) ({} duplicate(s) merged), {} submission attribution(s), and {} review attribution(s); revoked {} app session(s).",
                    summary.username,
                    summary.disc_credits,
                    summary.duplicate_disc_credits,
                    summary.submission_attributions,
                    summary.review_attributions,
                    summary.sessions_revoked,
                ),
            ))
        }
        DeleteUserResult::NotFound => Ok(redirect_with_message(
            "users",
            "error",
            "User was not found.",
        )),
        DeleteUserResult::Protected => Ok(redirect_with_message(
            "users",
            "error",
            "The Deleted user cannot be deleted.",
        )),
        DeleteUserResult::ConfirmationMismatch { username } => Ok(redirect_with_message(
            "users",
            "error",
            &format!("Confirmation must exactly match {username}."),
        )),
    }
}

fn normalize_maintenance_username(value: &str) -> Result<String, &'static str> {
    let username = value.trim();
    if username.is_empty() {
        return Err("Username cannot be empty.");
    }
    if username.chars().count() > 64 {
        return Err("Username must be 64 characters or fewer.");
    }
    Ok(username.to_string())
}

async fn rename_app_user(
    pool: &sqlx::PgPool,
    user_id: i32,
    new_username: &str,
) -> Result<RenameUserResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *transaction)
        .await?;

    let target = sqlx::query_as::<_, UserIdentity>(
        "SELECT id, username FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(target) = target else {
        transaction.rollback().await?;
        return Ok(RenameUserResult::NotFound);
    };

    if target.username == new_username {
        transaction.rollback().await?;
        return Ok(RenameUserResult::Unchanged);
    }

    let conflict = sqlx::query_as::<_, UserIdentity>(
        "SELECT id, username
         FROM users
         WHERE id <> $1 AND LOWER(username) = LOWER($2)
         ORDER BY id
         LIMIT 1",
    )
    .bind(user_id)
    .bind(new_username)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(conflict) = conflict {
        transaction.rollback().await?;
        return Ok(RenameUserResult::Conflict {
            username: conflict.username,
        });
    }

    sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(user_id)
        .bind(new_username)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    Ok(RenameUserResult::Renamed {
        old_username: target.username,
    })
}

async fn delete_app_user(
    pool: &sqlx::PgPool,
    user_id: i32,
    confirm_username: &str,
) -> Result<DeleteUserResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *transaction)
        .await?;

    let result = delete_app_user_on(&mut transaction, user_id, confirm_username).await?;
    if matches!(result, DeleteUserResult::Deleted(_)) {
        transaction.commit().await?;
    } else {
        transaction.rollback().await?;
    }
    Ok(result)
}

async fn delete_app_user_on(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: i32,
    confirm_username: &str,
) -> Result<DeleteUserResult, sqlx::Error> {
    let target = sqlx::query_as::<_, UserIdentity>(
        "SELECT id, username FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(target) = target else {
        return Ok(DeleteUserResult::NotFound);
    };

    if target.username == DELETED_USERNAME {
        return Ok(DeleteUserResult::Protected);
    }
    if target.username != confirm_username {
        return Ok(DeleteUserResult::ConfirmationMismatch {
            username: target.username,
        });
    }

    let deleted = sqlx::query_as::<_, UserIdentity>(
        "INSERT INTO users (username)
         VALUES ($1)
         ON CONFLICT (username) DO UPDATE SET username = EXCLUDED.username
         RETURNING id, username",
    )
    .bind(DELETED_USERNAME)
    .fetch_one(&mut **transaction)
    .await?;

    let duplicate_disc_credits = sqlx::query(
        "UPDATE disc_dumpers AS destination
         SET position = LEAST(destination.position, source.position)
         FROM disc_dumpers AS source
         WHERE source.user_id = $1
           AND destination.user_id = $2
           AND destination.disc_id = source.disc_id",
    )
    .bind(target.id)
    .bind(deleted.id)
    .execute(&mut **transaction)
    .await?
    .rows_affected();

    sqlx::query(
        "DELETE FROM disc_dumpers AS source
         USING disc_dumpers AS destination
         WHERE source.user_id = $1
           AND destination.user_id = $2
           AND destination.disc_id = source.disc_id",
    )
    .bind(target.id)
    .bind(deleted.id)
    .execute(&mut **transaction)
    .await?;

    let moved_disc_credits = sqlx::query("UPDATE disc_dumpers SET user_id = $2 WHERE user_id = $1")
        .bind(target.id)
        .bind(deleted.id)
        .execute(&mut **transaction)
        .await?
        .rows_affected();

    let submission_attributions =
        sqlx::query("UPDATE disc_submissions SET submitter_id = $2 WHERE submitter_id = $1")
            .bind(target.id)
            .bind(deleted.id)
            .execute(&mut **transaction)
            .await?
            .rows_affected();
    let review_attributions =
        sqlx::query("UPDATE disc_submissions SET reviewer_id = $2 WHERE reviewer_id = $1")
            .bind(target.id)
            .bind(deleted.id)
            .execute(&mut **transaction)
            .await?
            .rows_affected();
    let sessions_revoked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE user_id = $1")
            .bind(target.id)
            .fetch_one(&mut **transaction)
            .await?;

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(target.id)
        .execute(&mut **transaction)
        .await?;

    Ok(DeleteUserResult::Deleted(DeletedUserSummary {
        username: target.username,
        disc_credits: moved_disc_credits + duplicate_disc_credits,
        duplicate_disc_credits,
        submission_attributions,
        review_attributions,
        sessions_revoked,
    }))
}

async fn fetch_user_identity(
    pool: &sqlx::PgPool,
    user_id: i32,
) -> Result<Option<UserIdentity>, sqlx::Error> {
    sqlx::query_as("SELECT id, username FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

async fn download_backup(
    RequireAdmin(user): RequireAdmin,
    AxumPath(filename): AxumPath<String>,
    request: Request,
) -> Response {
    if backup_timestamp(&filename).is_none() {
        return AppError::NotFound.into_response();
    }

    let path = Path::new(BACKUP_DIR).join(&filename);
    let is_regular_file = std::fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false);
    if !is_regular_file {
        return AppError::NotFound.into_response();
    }

    tracing::info!(
        user_id = user.id,
        username = %user.username,
        backup = %filename,
        "Administrator downloaded database backup"
    );

    let mut response = ServeFile::new(path)
        .oneshot(request)
        .await
        .expect("ServeFile is infallible")
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .expect("validated backup filename is a valid header value"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

#[derive(Deserialize)]
struct SystemsForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    systems_payload: String,
}

#[derive(Deserialize)]
struct RegionsForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    regions_payload: String,
}

#[derive(Deserialize)]
struct LanguagesForm {
    #[serde(default, rename = "_csrf")]
    csrf_token: String,
    #[serde(default)]
    languages_payload: String,
}

async fn save_systems(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<SystemsForm>,
) -> AppResult<Response> {
    csrf::verify_token(&user, &form.csrf_token)?;

    let media_types = fetch_media_types(&state.pool).await?;
    let existing_systems = disc_service::get_all_systems(&state.pool).await?;
    let payload = match serde_json::from_str::<SystemsPayload>(&form.systems_payload) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!("Failed to parse systems maintenance payload: {err}");
            let rows = build_system_editor_rows(&existing_systems);
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some("systems".to_string()),
                    ..Default::default()
                },
                vec![
                    "Systems form payload was invalid. Reload the page and try again.".to_string(),
                ],
                MaintenanceOverrides {
                    system_rows: Some(rows),
                    ..Default::default()
                },
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let rows_for_errors = build_payload_system_rows(&payload);
    let validated = match validate_systems_payload(&payload, &existing_systems, &media_types) {
        Ok(rows) => rows,
        Err(errors) => {
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some("systems".to_string()),
                    ..Default::default()
                },
                errors,
                MaintenanceOverrides {
                    system_rows: Some(rows_for_errors),
                    ..Default::default()
                },
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let saved = match save_validated_systems(&state.pool, &validated, &existing_systems).await {
        Ok(saved) => saved,
        Err(err) => {
            tracing::error!("Failed to save systems: {err}");
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some("systems".to_string()),
                    ..Default::default()
                },
                vec!["Failed to save systems.".to_string()],
                MaintenanceOverrides {
                    system_rows: Some(rows_for_errors),
                    ..Default::default()
                },
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    Ok(redirect_with_message(
        "systems",
        "status",
        &format!("Saved {saved} system change(s)."),
    ))
}

async fn save_regions(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<RegionsForm>,
) -> AppResult<Response> {
    save_lookup_table(
        &state,
        user,
        &form.csrf_token,
        &form.regions_payload,
        LookupTable::Regions,
    )
    .await
}

async fn save_languages(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<LanguagesForm>,
) -> AppResult<Response> {
    save_lookup_table(
        &state,
        user,
        &form.csrf_token,
        &form.languages_payload,
        LookupTable::Languages,
    )
    .await
}

async fn render_submission_validation_errors(
    state: &AppState,
    user: AuthenticatedUser,
    submission_id: i32,
    form: &SubmissionMaintenanceForm,
    errors: Vec<String>,
) -> AppResult<Response> {
    let Some(row) = fetch_maintenance_submission(&state.pool, submission_id).await? else {
        return Ok(redirect_with_message(
            "submissions",
            "error",
            "Submission was not found.",
        ));
    };
    let editor = submission_editor_from_form(&row, form);
    let template = build_maintenance_template(
        &state.pool,
        user,
        MaintenanceQuery {
            tab: Some("submissions".to_string()),
            submission_id: Some(submission_id.to_string()),
            ..Default::default()
        },
        errors,
        MaintenanceOverrides {
            submission_editor: Some(editor),
            ..Default::default()
        },
    )
    .await?;
    Ok(Html(template.render().unwrap()).into_response())
}

async fn save_submission(
    State(state): State<AppState>,
    RequireAdmin(admin): RequireAdmin,
    AxumPath(submission_id): AxumPath<i32>,
    Form(form): Form<SubmissionMaintenanceForm>,
) -> AppResult<Response> {
    csrf::verify_token(&admin, &form.csrf_token)?;
    if submission_id <= 0 {
        return Ok(redirect_with_message(
            "submissions",
            "error",
            "Submission ID must be a positive integer.",
        ));
    }

    let update = match validate_submission_update(&state.pool, &form).await? {
        Ok(update) => update,
        Err(errors) => {
            return render_submission_validation_errors(
                &state,
                admin,
                submission_id,
                &form,
                errors,
            )
            .await;
        }
    };

    match update_maintenance_submission(&state.pool, submission_id, &update).await? {
        SubmissionUpdateOutcome::Updated(changed_fields) => {
            tracing::info!(
                administrator_id = admin.id,
                administrator_username = %admin.username,
                submission_id,
                changed_fields = ?changed_fields,
                "Administrator repaired submission record"
            );
            state.homepage_cache.invalidate().await;
            Ok(redirect_submission_with_message(
                submission_id,
                "status",
                &format!(
                    "Saved submission {submission_id}: {}.",
                    changed_fields.join(", ")
                ),
            ))
        }
        SubmissionUpdateOutcome::Unchanged => Ok(redirect_submission_with_message(
            submission_id,
            "status",
            &format!("Submission {submission_id} was unchanged."),
        )),
        SubmissionUpdateOutcome::NotFound => Ok(redirect_with_message(
            "submissions",
            "error",
            "Submission was not found.",
        )),
        SubmissionUpdateOutcome::Stale => Ok(redirect_submission_with_message(
            submission_id,
            "error",
            "The submission changed after it was loaded. Review the current values and try again.",
        )),
    }
}

async fn save_lookup_table(
    state: &AppState,
    user: AuthenticatedUser,
    csrf_token: &str,
    payload_json: &str,
    table: LookupTable,
) -> AppResult<Response> {
    csrf::verify_token(&user, csrf_token)?;

    let flag_options = load_flag_options()?;
    let existing_rows = fetch_lookup_records(&state.pool, table).await?;
    let payload = match serde_json::from_str::<LookupPayload>(payload_json) {
        Ok(payload) => payload,
        Err(err) => {
            tracing::warn!(
                "Failed to parse {} maintenance payload: {err}",
                table.slug()
            );
            let rows = build_lookup_editor_rows(&existing_rows);
            let (region_rows, language_rows) = table.lookup_row_overrides(rows);
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some(table.slug().to_string()),
                    ..Default::default()
                },
                vec![format!(
                    "{} form payload was invalid. Reload the page and try again.",
                    table.plural_title()
                )],
                MaintenanceOverrides {
                    region_rows,
                    language_rows,
                    ..Default::default()
                },
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let rows_for_errors = build_payload_lookup_rows(&payload);
    let validated = match validate_lookup_payload(&payload, &existing_rows, &flag_options, table) {
        Ok(rows) => rows,
        Err(errors) => {
            let template = build_maintenance_template(
                &state.pool,
                user,
                MaintenanceQuery {
                    tab: Some(table.slug().to_string()),
                    ..Default::default()
                },
                errors,
                MaintenanceOverrides {
                    region_rows: table.region_rows_override(rows_for_errors.clone()),
                    language_rows: table.language_rows_override(rows_for_errors),
                    ..Default::default()
                },
            )
            .await?;
            return Ok(Html(template.render().unwrap()).into_response());
        }
    };

    let saved =
        match save_validated_lookup_rows(&state.pool, &validated, &existing_rows, table).await {
            Ok(saved) => saved,
            Err(err) => {
                tracing::error!("Failed to save {}: {err}", table.slug());
                let template = build_maintenance_template(
                    &state.pool,
                    user,
                    MaintenanceQuery {
                        tab: Some(table.slug().to_string()),
                        ..Default::default()
                    },
                    vec![format!("Failed to save {}.", table.plural_lower())],
                    MaintenanceOverrides {
                        region_rows: table.region_rows_override(rows_for_errors.clone()),
                        language_rows: table.language_rows_override(rows_for_errors),
                        ..Default::default()
                    },
                )
                .await?;
                return Ok(Html(template.render().unwrap()).into_response());
            }
        };

    Ok(redirect_with_message(
        table.slug(),
        "status",
        &format!("Saved {saved} {} change(s).", table.singular_lower()),
    ))
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct MaintenanceMediaType {
    code: String,
    name: String,
}

#[derive(Clone, Debug)]
struct SystemEditorRow {
    original_code: String,
    code: String,
    system_type: String,
    manufacturer: String,
    name: String,
    short_name: String,
    media_types_csv: String,
    flags: Vec<SystemFlagOption>,
}

#[derive(Clone, Debug)]
struct SystemInputSizes {
    code: usize,
    system_type: usize,
    manufacturer: usize,
    name: usize,
    short_name: usize,
    media_types: usize,
}

#[derive(Clone, Debug)]
struct LookupEditorRow {
    original_code: String,
    code: String,
    name: String,
    flag_code: String,
    sort_order: String,
}

#[derive(Clone, Debug)]
struct LookupInputSizes {
    code: usize,
    name: usize,
    sort_order: usize,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct LookupRecord {
    code: String,
    name: String,
    flag_code: String,
    sort_order: i32,
}

#[derive(Clone, Debug)]
struct FlagOption {
    code: String,
}

#[derive(Clone, Debug)]
struct SystemFlagOption {
    field: &'static str,
    label: &'static str,
    checked: bool,
}

#[derive(Clone, Copy, Debug)]
struct SystemFlagDefinition {
    field: &'static str,
    label: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupTable {
    Regions,
    Languages,
}

impl LookupTable {
    fn slug(self) -> &'static str {
        match self {
            LookupTable::Regions => "regions",
            LookupTable::Languages => "languages",
        }
    }

    fn singular_title(self) -> &'static str {
        match self {
            LookupTable::Regions => "Region",
            LookupTable::Languages => "Language",
        }
    }

    fn plural_title(self) -> &'static str {
        match self {
            LookupTable::Regions => "Regions",
            LookupTable::Languages => "Languages",
        }
    }

    fn singular_lower(self) -> &'static str {
        match self {
            LookupTable::Regions => "region",
            LookupTable::Languages => "language",
        }
    }

    fn plural_lower(self) -> &'static str {
        match self {
            LookupTable::Regions => "regions",
            LookupTable::Languages => "languages",
        }
    }

    fn name_max_len(self) -> usize {
        match self {
            LookupTable::Regions => 32,
            LookupTable::Languages => 16,
        }
    }

    fn lookup_row_overrides(
        self,
        rows: Vec<LookupEditorRow>,
    ) -> (Option<Vec<LookupEditorRow>>, Option<Vec<LookupEditorRow>>) {
        match self {
            LookupTable::Regions => (Some(rows), None),
            LookupTable::Languages => (None, Some(rows)),
        }
    }

    fn region_rows_override(self, rows: Vec<LookupEditorRow>) -> Option<Vec<LookupEditorRow>> {
        match self {
            LookupTable::Regions => Some(rows),
            LookupTable::Languages => None,
        }
    }

    fn language_rows_override(self, rows: Vec<LookupEditorRow>) -> Option<Vec<LookupEditorRow>> {
        match self {
            LookupTable::Regions => None,
            LookupTable::Languages => Some(rows),
        }
    }

    fn insert_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => INSERT_REGION_SQL,
            LookupTable::Languages => INSERT_LANGUAGE_SQL,
        }
    }

    fn update_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => UPDATE_REGION_SQL,
            LookupTable::Languages => UPDATE_LANGUAGE_SQL,
        }
    }

    fn rename_junction_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => RENAME_DISC_REGIONS_SQL,
            LookupTable::Languages => RENAME_DISC_LANGUAGES_SQL,
        }
    }

    fn delete_sql(self) -> &'static str {
        match self {
            LookupTable::Regions => DELETE_REGION_SQL,
            LookupTable::Languages => DELETE_LANGUAGE_SQL,
        }
    }
}

const SYSTEM_FLAGS: &[SystemFlagDefinition] = &[
    SystemFlagDefinition {
        field: "has_title_foreign",
        label: "Foreign Title",
    },
    SystemFlagDefinition {
        field: "has_disc_number",
        label: "Disc Number",
    },
    SystemFlagDefinition {
        field: "has_disc_title",
        label: "Disc Title",
    },
    SystemFlagDefinition {
        field: "has_serial",
        label: "Serial",
    },
    SystemFlagDefinition {
        field: "has_edition",
        label: "Edition",
    },
    SystemFlagDefinition {
        field: "has_barcode",
        label: "Barcode",
    },
    SystemFlagDefinition {
        field: "has_version",
        label: "Version",
    },
    SystemFlagDefinition {
        field: "has_exe_date",
        label: "EXE Date",
    },
    SystemFlagDefinition {
        field: "has_edc",
        label: "EDC",
    },
    SystemFlagDefinition {
        field: "has_error_count",
        label: "Error Count",
    },
    SystemFlagDefinition {
        field: "has_universal_hash",
        label: "Universal Hash",
    },
    SystemFlagDefinition {
        field: "has_protection",
        label: "Protection",
    },
    SystemFlagDefinition {
        field: "has_sector_ranges",
        label: "Sector Ranges",
    },
    SystemFlagDefinition {
        field: "has_sbi",
        label: "SBI",
    },
    SystemFlagDefinition {
        field: "has_pvd",
        label: "PVD",
    },
    SystemFlagDefinition {
        field: "has_header",
        label: "Header",
    },
    SystemFlagDefinition {
        field: "has_bca",
        label: "BCA",
    },
    SystemFlagDefinition {
        field: "has_sample_start",
        label: "Sample Start",
    },
    SystemFlagDefinition {
        field: "has_offset_extra",
        label: "Offset Extra",
    },
];

#[derive(Clone, Debug, Default, Deserialize)]
struct SystemsPayload {
    #[serde(default)]
    rows: Vec<SystemPayloadRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SystemPayloadRow {
    #[serde(default)]
    original_code: String,
    #[serde(default)]
    code: String,
    #[serde(default, rename = "type")]
    system_type: String,
    #[serde(default)]
    manufacturer: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    short_name: String,
    #[serde(default)]
    media_types: String,
    #[serde(default)]
    flags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedSystemRow {
    original_code: Option<String>,
    code: String,
    system_type: String,
    manufacturer: String,
    name: String,
    short_name: String,
    media_types: Vec<String>,
    flags: HashSet<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LookupPayload {
    #[serde(default)]
    rows: Vec<LookupPayloadRow>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LookupPayloadRow {
    #[serde(default)]
    original_code: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    flag_code: String,
    #[serde(default)]
    sort_order: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedLookupRow {
    original_code: Option<String>,
    code: String,
    name: String,
    flag_code: String,
    sort_order: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SystemSaveAction {
    Unchanged,
    Insert,
    Update,
    Rename,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LookupSaveAction {
    Unchanged,
    Insert,
    Update,
    Rename,
}

const INSERT_SYSTEM_SQL: &str = "
    INSERT INTO systems
        (code, type, manufacturer, name, short_name, media_types,
         has_title_foreign, has_disc_number, has_disc_title, has_serial,
         has_edition, has_barcode, has_version, has_exe_date, has_edc,
         has_error_count, has_universal_hash, has_protection,
         has_sector_ranges, has_sbi, has_pvd, has_header, has_bca,
         has_sample_start, has_offset_extra, archives_dirty)
    VALUES
        ($1, $2, $3, $4, $5, $6,
         $7, $8, $9, $10,
         $11, $12, $13, $14, $15,
         $16, $17, $18,
         $19, $20, $21, $22, $23,
         $24, $25, TRUE)";

const UPDATE_SYSTEM_SQL: &str = "
    UPDATE systems
    SET type = $2,
        manufacturer = $3,
        name = $4,
        short_name = $5,
        media_types = $6,
        has_title_foreign = $7,
        has_disc_number = $8,
        has_disc_title = $9,
        has_serial = $10,
        has_edition = $11,
        has_barcode = $12,
        has_version = $13,
        has_exe_date = $14,
        has_edc = $15,
        has_error_count = $16,
        has_universal_hash = $17,
        has_protection = $18,
        has_sector_ranges = $19,
        has_sbi = $20,
        has_pvd = $21,
        has_header = $22,
        has_bca = $23,
        has_sample_start = $24,
        has_offset_extra = $25,
        archives_dirty = TRUE
    WHERE code = $1";

const RENAME_DISCS_SYSTEM_SQL: &str = "UPDATE discs SET system_code = $1 WHERE system_code = $2";
const DELETE_SYSTEM_SQL: &str = "DELETE FROM systems WHERE code = $1";

const INSERT_REGION_SQL: &str =
    "INSERT INTO regions (code, name, flag_code, sort_order) VALUES ($1, $2, $3, $4)";
const UPDATE_REGION_SQL: &str =
    "UPDATE regions SET name = $2, flag_code = $3, sort_order = $4 WHERE code = $1";
const RENAME_DISC_REGIONS_SQL: &str =
    "UPDATE disc_regions SET region_code = $1 WHERE region_code = $2";
const DELETE_REGION_SQL: &str = "DELETE FROM regions WHERE code = $1";

const INSERT_LANGUAGE_SQL: &str =
    "INSERT INTO languages (code, name, flag_code, sort_order) VALUES ($1, $2, $3, $4)";
const UPDATE_LANGUAGE_SQL: &str =
    "UPDATE languages SET name = $2, flag_code = $3, sort_order = $4 WHERE code = $1";
const RENAME_DISC_LANGUAGES_SQL: &str =
    "UPDATE disc_languages SET language_code = $1 WHERE language_code = $2";
const DELETE_LANGUAGE_SQL: &str = "DELETE FROM languages WHERE code = $1";

#[derive(Default)]
struct MaintenanceOverrides {
    system_rows: Option<Vec<SystemEditorRow>>,
    region_rows: Option<Vec<LookupEditorRow>>,
    language_rows: Option<Vec<LookupEditorRow>>,
    submission_editor: Option<MaintenanceSubmissionEditor>,
}

async fn build_maintenance_template(
    pool: &sqlx::PgPool,
    user: AuthenticatedUser,
    query: MaintenanceQuery,
    mut maintenance_errors: Vec<String>,
    overrides: MaintenanceOverrides,
) -> AppResult<MaintenanceTemplate> {
    let system_rows = match overrides.system_rows {
        Some(rows) => rows,
        None => {
            let systems = disc_service::get_all_systems(pool).await?;
            build_system_editor_rows(&systems)
        }
    };
    let region_rows = match overrides.region_rows {
        Some(rows) => rows,
        None => {
            let regions = fetch_lookup_records(pool, LookupTable::Regions).await?;
            build_lookup_editor_rows(&regions)
        }
    };
    let language_rows = match overrides.language_rows {
        Some(rows) => rows,
        None => {
            let languages = fetch_lookup_records(pool, LookupTable::Languages).await?;
            build_lookup_editor_rows(&languages)
        }
    };
    let flag_options = load_flag_options()?;
    let user_rows = if user.role.can_admin() && query.tab.as_deref() == Some("users") {
        fetch_maintenance_users(pool).await?
    } else {
        Vec::new()
    };
    let submission_lookup_id = query.submission_id.clone().unwrap_or_default();
    let mut submission_users = Vec::new();
    let submission_editor = if user.role.can_admin()
        && query.tab.as_deref() == Some("submissions")
        && overrides.submission_editor.is_some()
    {
        submission_users = fetch_submission_users(pool).await?;
        overrides.submission_editor
    } else if user.role.can_admin()
        && query.tab.as_deref() == Some("submissions")
        && !submission_lookup_id.trim().is_empty()
    {
        match parse_positive_id(&submission_lookup_id, "Submission ID") {
            Ok(id) => {
                submission_users = fetch_submission_users(pool).await?;
                match fetch_maintenance_submission(pool, id).await? {
                    Some(row) => Some(maintenance_submission_editor(&row, &submission_users)),
                    None => {
                        maintenance_errors.push(format!("Submission {id} was not found."));
                        None
                    }
                }
            }
            Err(error) => {
                maintenance_errors.push(error);
                None
            }
        }
    } else {
        None
    };
    let backup_files = if user.role.can_admin() {
        list_backup_files(Path::new(BACKUP_DIR))
    } else {
        Vec::new()
    };

    Ok(maintenance_template(
        user,
        query,
        maintenance_errors,
        system_rows,
        region_rows,
        language_rows,
        flag_options,
        user_rows,
        submission_users,
        submission_lookup_id,
        submission_editor,
        backup_files,
    ))
}

fn maintenance_template(
    user: AuthenticatedUser,
    query: MaintenanceQuery,
    maintenance_errors: Vec<String>,
    system_rows: Vec<SystemEditorRow>,
    region_rows: Vec<LookupEditorRow>,
    language_rows: Vec<LookupEditorRow>,
    flag_options: Vec<FlagOption>,
    user_rows: Vec<MaintenanceUser>,
    submission_users: Vec<UserIdentity>,
    submission_lookup_id: String,
    submission_editor: Option<MaintenanceSubmissionEditor>,
    backup_files: Vec<BackupFile>,
) -> MaintenanceTemplate {
    let active_tab = match query.tab.as_deref() {
        Some("systems") => "systems",
        Some("regions") => "regions",
        Some("languages") => "languages",
        Some("misc") => "misc",
        Some("users") if user.role.can_admin() => "users",
        Some("submissions") if user.role.can_admin() => "submissions",
        Some("backup") if user.role.can_admin() => "backup",
        _ => "general",
    };
    let system_input_sizes = system_input_sizes(&system_rows);
    let region_input_sizes = lookup_input_sizes(&region_rows);
    let language_input_sizes = lookup_input_sizes(&language_rows);
    let can_admin = user.role.can_admin();
    MaintenanceTemplate {
        current_user: Some(user),
        status_message: query.status.unwrap_or_default(),
        error_message: query.error.unwrap_or_default(),
        maintenance_errors,
        system_rows,
        region_rows,
        language_rows,
        flag_options,
        system_input_sizes,
        region_input_sizes,
        language_input_sizes,
        user_rows,
        submission_users,
        submission_lookup_id,
        submission_editor,
        backup_files,
        can_admin,
        show_general: active_tab == "general",
        show_systems: active_tab == "systems",
        show_regions: active_tab == "regions",
        show_languages: active_tab == "languages",
        show_misc: active_tab == "misc",
        show_users: active_tab == "users",
        show_submissions: active_tab == "submissions",
        show_backup: active_tab == "backup",
    }
}

async fn fetch_maintenance_users(pool: &sqlx::PgPool) -> AppResult<Vec<MaintenanceUser>> {
    let rows = sqlx::query_as(
        "SELECT u.id,
                u.username,
                COUNT(s.id)::BIGINT AS active_session_count
         FROM users u
         LEFT JOIN sessions s ON s.user_id = u.id
         GROUP BY u.id, u.username
         ORDER BY LOWER(u.username), u.username, u.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn fetch_submission_users(pool: &sqlx::PgPool) -> AppResult<Vec<UserIdentity>> {
    Ok(
        sqlx::query_as("SELECT id, username FROM users ORDER BY LOWER(username), username, id")
            .fetch_all(pool)
            .await?,
    )
}

async fn fetch_maintenance_submission(
    pool: &sqlx::PgPool,
    submission_id: i32,
) -> AppResult<Option<MaintenanceSubmissionRow>> {
    Ok(sqlx::query_as(
        "SELECT id, submission_type, submitter_id, submission_comment, target_disc_id,
                changes_original, changes, dump_log, extra_upload_url, submission_token,
                submission_fingerprint, status, reviewer_id, review_comment, created_at,
                reviewed_at, xmin::TEXT AS row_version
         FROM disc_submissions
         WHERE id = $1",
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await?)
}

fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn format_maintenance_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn username_for_id(users: &[UserIdentity], user_id: Option<i32>) -> String {
    user_id
        .and_then(|id| users.iter().find(|user| user.id == id))
        .map(|user| user.username.clone())
        .unwrap_or_default()
}

fn maintenance_submission_editor(
    row: &MaintenanceSubmissionRow,
    users: &[UserIdentity],
) -> MaintenanceSubmissionEditor {
    MaintenanceSubmissionEditor {
        id: row.id,
        row_version: row.row_version.clone(),
        submission_type: row.submission_type.to_string(),
        submitter_id: row.submitter_id.to_string(),
        submitter_username: username_for_id(users, Some(row.submitter_id)),
        submission_comment: row.submission_comment.clone().unwrap_or_default(),
        target_disc_id: row
            .target_disc_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        changes_original: row
            .changes_original
            .as_ref()
            .map(pretty_json)
            .unwrap_or_default(),
        changes: pretty_json(&row.changes),
        dump_log: row.dump_log.clone().unwrap_or_default(),
        extra_upload_url: row.extra_upload_url.clone().unwrap_or_default(),
        submission_token: row.submission_token.clone().unwrap_or_default(),
        submission_fingerprint: row.submission_fingerprint.clone().unwrap_or_default(),
        status: row.status.to_string(),
        reviewer_id: row.reviewer_id.map(|id| id.to_string()).unwrap_or_default(),
        reviewer_username: username_for_id(users, row.reviewer_id),
        review_comment: row.review_comment.clone().unwrap_or_default(),
        created_at: format_maintenance_timestamp(row.created_at),
        reviewed_at: row
            .reviewed_at
            .map(format_maintenance_timestamp)
            .unwrap_or_default(),
    }
}

fn submission_editor_from_form(
    row: &MaintenanceSubmissionRow,
    form: &SubmissionMaintenanceForm,
) -> MaintenanceSubmissionEditor {
    MaintenanceSubmissionEditor {
        id: row.id,
        row_version: form.row_version.clone(),
        submission_type: form.submission_type.clone(),
        submitter_id: form.submitter_id.clone(),
        submitter_username: form.submitter_username.clone(),
        submission_comment: form.submission_comment.clone(),
        target_disc_id: form.target_disc_id.clone(),
        changes_original: form.changes_original.clone(),
        changes: form.changes.clone(),
        dump_log: form.dump_log.clone(),
        extra_upload_url: form.extra_upload_url.clone(),
        submission_token: row.submission_token.clone().unwrap_or_default(),
        submission_fingerprint: row.submission_fingerprint.clone().unwrap_or_default(),
        status: form.status.clone(),
        reviewer_id: form.reviewer_id.clone(),
        reviewer_username: form.reviewer_username.clone(),
        review_comment: form.review_comment.clone(),
        created_at: form.created_at.clone(),
        reviewed_at: form.reviewed_at.clone(),
    }
}

fn parse_positive_id(value: &str, field: &str) -> Result<i32, String> {
    value
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| format!("{field} must be a positive integer."))
}

fn parse_optional_positive_id(value: &str, field: &str) -> Result<Option<i32>, String> {
    if value.trim().is_empty() {
        Ok(None)
    } else {
        parse_positive_id(value, field).map(Some)
    }
}

fn nullable_text(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn parse_submission_type(value: &str) -> Result<SubmissionType, String> {
    match value.trim() {
        "Disc" => Ok(SubmissionType::Disc),
        "Edit" => Ok(SubmissionType::Edit),
        _ => Err("Submission Type must be Disc or Edit.".to_string()),
    }
}

fn parse_submission_status(value: &str) -> Result<SubmissionStatus, String> {
    match value.trim() {
        "Pending" => Ok(SubmissionStatus::Pending),
        "Draft" => Ok(SubmissionStatus::Draft),
        "Approved" => Ok(SubmissionStatus::Approved),
        "Rejected" => Ok(SubmissionStatus::Rejected),
        "Legacy" => Ok(SubmissionStatus::Legacy),
        _ => Err("Status must be Pending, Draft, Approved, Rejected, or Legacy.".to_string()),
    }
}

fn parse_json(
    value: &str,
    field: &str,
    required: bool,
) -> Result<Option<serde_json::Value>, String> {
    let value = value.trim();
    if value.is_empty() {
        return if required {
            Err(format!("{field} is required."))
        } else {
            Ok(None)
        };
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|error| format!("{field} must be valid JSON: {error}"))
}

fn parse_timestamp(
    value: &str,
    field: &str,
    required: bool,
) -> Result<Option<DateTime<Utc>>, String> {
    let value = value.trim();
    if value.is_empty() {
        return if required {
            Err(format!("{field} is required."))
        } else {
            Ok(None)
        };
    }
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| Some(timestamp.with_timezone(&Utc)))
        .map_err(|_| format!("{field} must be an RFC 3339 timestamp."))
}

async fn record_exists(pool: &sqlx::PgPool, table: &str, id: i32) -> AppResult<bool> {
    let sql = match table {
        "users" => "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)",
        "discs" => "SELECT EXISTS(SELECT 1 FROM discs WHERE id = $1)",
        _ => {
            return Err(AppError::Internal(
                "Invalid maintenance lookup table.".into(),
            ))
        }
    };
    Ok(sqlx::query_scalar(sql).bind(id).fetch_one(pool).await?)
}

async fn validate_submission_update(
    pool: &sqlx::PgPool,
    form: &SubmissionMaintenanceForm,
) -> AppResult<Result<ValidatedSubmissionUpdate, Vec<String>>> {
    let mut errors = Vec::new();

    let row_version = form.row_version.trim().to_string();
    if row_version.parse::<u64>().is_err() {
        errors.push("The row version is invalid. Reload the submission and try again.".into());
    }

    let submission_type = parse_submission_type(&form.submission_type)
        .map_err(|error| errors.push(error))
        .ok();
    let submitter_id = parse_positive_id(&form.submitter_id, "Submitter")
        .map_err(|error| errors.push(error))
        .ok();
    let target_disc_id = parse_optional_positive_id(&form.target_disc_id, "Target Disc ID")
        .map_err(|error| errors.push(error))
        .ok()
        .flatten();
    let changes_original = parse_json(&form.changes_original, "Original Changes", false)
        .map_err(|error| errors.push(error))
        .ok()
        .flatten();
    let changes = parse_json(&form.changes, "Changes", true)
        .map_err(|error| errors.push(error))
        .ok()
        .flatten();
    let status = parse_submission_status(&form.status)
        .map_err(|error| errors.push(error))
        .ok();
    let reviewer_id = parse_optional_positive_id(&form.reviewer_id, "Reviewer")
        .map_err(|error| errors.push(error))
        .ok()
        .flatten();
    let created_at = parse_timestamp(&form.created_at, "Created At", true)
        .map_err(|error| errors.push(error))
        .ok()
        .flatten();
    let reviewed_at = parse_timestamp(&form.reviewed_at, "Reviewed At", false)
        .map_err(|error| errors.push(error))
        .ok()
        .flatten();

    let extra_upload_url = nullable_text(&form.extra_upload_url);
    if extra_upload_url
        .as_ref()
        .is_some_and(|value| value.chars().count() > 512)
    {
        errors.push("Extra Upload URL must be 512 characters or fewer.".into());
    }

    if let Some(id) = submitter_id {
        if !record_exists(pool, "users", id).await? {
            errors.push(format!("Submitter user {id} was not found."));
        }
    }
    if let Some(id) = reviewer_id {
        if !record_exists(pool, "users", id).await? {
            errors.push(format!("Reviewer user {id} was not found."));
        }
    }
    if let Some(id) = target_disc_id {
        if !record_exists(pool, "discs", id).await? {
            errors.push(format!("Target disc {id} was not found."));
        }
    }
    if matches!(
        status,
        Some(SubmissionStatus::Approved | SubmissionStatus::Legacy)
    ) && reviewed_at.is_none()
    {
        errors.push("Reviewed At is required for Approved and Legacy submissions.".into());
    }

    if !errors.is_empty() {
        return Ok(Err(errors));
    }

    Ok(Ok(ValidatedSubmissionUpdate {
        row_version,
        submission_type: submission_type.unwrap(),
        submitter_id: submitter_id.unwrap(),
        submission_comment: nullable_text(&form.submission_comment),
        target_disc_id,
        changes_original,
        changes: changes.unwrap(),
        dump_log: nullable_text(&form.dump_log),
        extra_upload_url,
        status: status.unwrap(),
        reviewer_id,
        review_comment: nullable_text(&form.review_comment),
        created_at: created_at.unwrap(),
        reviewed_at,
    }))
}

fn changed_submission_fields(
    row: &MaintenanceSubmissionRow,
    update: &ValidatedSubmissionUpdate,
) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if row.submission_type != update.submission_type {
        fields.push("submission_type");
    }
    if row.submitter_id != update.submitter_id {
        fields.push("submitter_id");
    }
    if row.submission_comment != update.submission_comment {
        fields.push("submission_comment");
    }
    if row.target_disc_id != update.target_disc_id {
        fields.push("target_disc_id");
    }
    if row.changes_original != update.changes_original {
        fields.push("changes_original");
    }
    if row.changes != update.changes {
        fields.push("changes");
    }
    if row.dump_log != update.dump_log {
        fields.push("dump_log");
    }
    if row.extra_upload_url != update.extra_upload_url {
        fields.push("extra_upload_url");
    }
    if row.status != update.status {
        fields.push("status");
    }
    if row.reviewer_id != update.reviewer_id {
        fields.push("reviewer_id");
    }
    if row.review_comment != update.review_comment {
        fields.push("review_comment");
    }
    if row.created_at != update.created_at {
        fields.push("created_at");
    }
    if row.reviewed_at != update.reviewed_at {
        fields.push("reviewed_at");
    }
    fields
}

async fn update_maintenance_submission(
    pool: &sqlx::PgPool,
    submission_id: i32,
    update: &ValidatedSubmissionUpdate,
) -> AppResult<SubmissionUpdateOutcome> {
    let mut transaction = pool.begin().await?;
    let outcome = update_maintenance_submission_on(&mut transaction, submission_id, update).await?;
    transaction.commit().await?;
    Ok(outcome)
}

async fn update_maintenance_submission_on(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    submission_id: i32,
    update: &ValidatedSubmissionUpdate,
) -> AppResult<SubmissionUpdateOutcome> {
    let row: Option<MaintenanceSubmissionRow> = sqlx::query_as(
        "SELECT id, submission_type, submitter_id, submission_comment, target_disc_id,
                changes_original, changes, dump_log, extra_upload_url, submission_token,
                submission_fingerprint, status, reviewer_id, review_comment, created_at,
                reviewed_at, xmin::TEXT AS row_version
         FROM disc_submissions
         WHERE id = $1
         FOR UPDATE",
    )
    .bind(submission_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(row) = row else {
        return Ok(SubmissionUpdateOutcome::NotFound);
    };
    if row.row_version != update.row_version {
        return Ok(SubmissionUpdateOutcome::Stale);
    }

    let changed_fields = changed_submission_fields(&row, update);
    if changed_fields.is_empty() {
        return Ok(SubmissionUpdateOutcome::Unchanged);
    }

    sqlx::query(
        "UPDATE disc_submissions
         SET submission_type = $2, submitter_id = $3, submission_comment = $4,
             target_disc_id = $5, changes_original = $6, changes = $7, dump_log = $8,
             extra_upload_url = $9, status = $10, reviewer_id = $11,
             review_comment = $12, created_at = $13, reviewed_at = $14
         WHERE id = $1",
    )
    .bind(submission_id)
    .bind(update.submission_type)
    .bind(update.submitter_id)
    .bind(update.submission_comment.as_deref())
    .bind(update.target_disc_id)
    .bind(&update.changes_original)
    .bind(&update.changes)
    .bind(update.dump_log.as_deref())
    .bind(update.extra_upload_url.as_deref())
    .bind(update.status)
    .bind(update.reviewer_id)
    .bind(update.review_comment.as_deref())
    .bind(update.created_at)
    .bind(update.reviewed_at)
    .execute(&mut **transaction)
    .await?;

    Ok(SubmissionUpdateOutcome::Updated(changed_fields))
}

fn list_backup_files(directory: &Path) -> Vec<BackupFile> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(path = %directory.display(), "Could not list backups: {err}");
            return Vec::new();
        }
    };

    let mut backups = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let filename = entry.file_name().into_string().ok()?;
            let created_at = backup_timestamp(&filename)?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some(BackupFile {
                filename,
                created_at,
                size: format_file_size(metadata.len()),
            })
        })
        .collect::<Vec<_>>();
    backups.sort_by(|left, right| right.filename.cmp(&left.filename));
    backups
}

fn backup_timestamp(filename: &str) -> Option<String> {
    let timestamp = filename
        .strip_prefix(BACKUP_PREFIX)?
        .strip_suffix(BACKUP_SUFFIX)?;
    let bytes = timestamp.as_bytes();
    if bytes.len() != 16
        || bytes[8] != b'T'
        || bytes[15] != b'Z'
        || !bytes[..8].iter().all(u8::is_ascii_digit)
        || !bytes[9..15].iter().all(u8::is_ascii_digit)
    {
        return None;
    }

    Some(format!(
        "{}-{}-{} {}:{}:{} UTC",
        &timestamp[0..4],
        &timestamp[4..6],
        &timestamp[6..8],
        &timestamp[9..11],
        &timestamp[11..13],
        &timestamp[13..15],
    ))
}

fn format_file_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

async fn fetch_media_types(pool: &sqlx::PgPool) -> AppResult<Vec<MaintenanceMediaType>> {
    Ok(
        sqlx::query_as("SELECT code, name FROM media_types ORDER BY LOWER(name), code")
            .fetch_all(pool)
            .await?,
    )
}

async fn fetch_lookup_records(
    pool: &sqlx::PgPool,
    table: LookupTable,
) -> AppResult<Vec<LookupRecord>> {
    let sql = match table {
        LookupTable::Regions => {
            "SELECT TRIM(code)::TEXT AS code, name, flag_code::TEXT AS flag_code, sort_order \
             FROM regions ORDER BY sort_order, LOWER(name), code"
        }
        LookupTable::Languages => {
            "SELECT TRIM(code)::TEXT AS code, name, flag_code::TEXT AS flag_code, sort_order \
             FROM languages ORDER BY sort_order, LOWER(name), code"
        }
    };
    Ok(sqlx::query_as(sql).fetch_all(pool).await?)
}

fn load_flag_options() -> AppResult<Vec<FlagOption>> {
    let flag_dir = Path::new("static/flags");
    let entries = std::fs::read_dir(flag_dir).map_err(|err| {
        AppError::Internal(format!(
            "Failed to read flag directory {}: {err}",
            flag_dir.display()
        ))
    })?;
    let mut codes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            AppError::Internal(format!(
                "Failed to read flag directory entry {}: {err}",
                flag_dir.display()
            ))
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("svg") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            codes.push(stem.to_string());
        }
    }
    codes.sort();
    Ok(codes.into_iter().map(|code| FlagOption { code }).collect())
}

fn build_system_editor_rows(systems: &[System]) -> Vec<SystemEditorRow> {
    let mut rows: Vec<_> = systems.iter().map(system_editor_row).collect();
    rows.push(blank_system_editor_row());
    rows
}

fn system_editor_row(system: &System) -> SystemEditorRow {
    SystemEditorRow {
        original_code: system.code.clone(),
        code: system.code.clone(),
        system_type: system.system_type.clone(),
        manufacturer: system.manufacturer.clone(),
        name: system.name.clone(),
        short_name: system.short_name.clone(),
        media_types_csv: system.media_types.join(","),
        flags: flag_options_for_system(system),
    }
}

fn blank_system_editor_row() -> SystemEditorRow {
    let flags = HashSet::from(["has_error_count".to_string()]);
    SystemEditorRow {
        original_code: String::new(),
        code: String::new(),
        system_type: String::new(),
        manufacturer: String::new(),
        name: String::new(),
        short_name: String::new(),
        media_types_csv: String::new(),
        flags: flag_options_from_set(&flags),
    }
}

fn build_payload_system_rows(payload: &SystemsPayload) -> Vec<SystemEditorRow> {
    let mut rows: Vec<_> = payload
        .rows
        .iter()
        .map(|row| {
            let flags: HashSet<String> = row
                .flags
                .iter()
                .map(|flag| flag.trim().to_string())
                .collect();
            SystemEditorRow {
                original_code: row.original_code.trim().to_string(),
                code: row.code.trim().to_string(),
                system_type: row.system_type.trim().to_string(),
                manufacturer: row.manufacturer.trim().to_string(),
                name: row.name.trim().to_string(),
                short_name: row.short_name.trim().to_string(),
                media_types_csv: row.media_types.trim().to_string(),
                flags: flag_options_from_set(&flags),
            }
        })
        .collect();
    if !rows.iter().any(|row| row.original_code.is_empty()) {
        rows.push(blank_system_editor_row());
    }
    rows
}

fn build_lookup_editor_rows(records: &[LookupRecord]) -> Vec<LookupEditorRow> {
    let mut rows: Vec<_> = records.iter().map(lookup_editor_row).collect();
    rows.push(blank_lookup_editor_row());
    rows
}

fn lookup_editor_row(record: &LookupRecord) -> LookupEditorRow {
    LookupEditorRow {
        original_code: record.code.trim().to_string(),
        code: record.code.trim().to_string(),
        name: record.name.clone(),
        flag_code: record.flag_code.trim().to_string(),
        sort_order: record.sort_order.to_string(),
    }
}

fn blank_lookup_editor_row() -> LookupEditorRow {
    LookupEditorRow {
        original_code: String::new(),
        code: String::new(),
        name: String::new(),
        flag_code: String::new(),
        sort_order: String::new(),
    }
}

fn build_payload_lookup_rows(payload: &LookupPayload) -> Vec<LookupEditorRow> {
    let mut rows: Vec<_> = payload
        .rows
        .iter()
        .map(|row| LookupEditorRow {
            original_code: row.original_code.trim().to_string(),
            code: row.code.trim().to_string(),
            name: row.name.trim().to_string(),
            flag_code: row.flag_code.trim().to_string(),
            sort_order: row.sort_order.trim().to_string(),
        })
        .collect();
    if !rows.iter().any(|row| row.original_code.is_empty()) {
        rows.push(blank_lookup_editor_row());
    }
    rows
}

fn system_input_sizes(rows: &[SystemEditorRow]) -> SystemInputSizes {
    SystemInputSizes {
        code: column_input_size(rows.iter().map(|row| row.code.as_str()), 4, 16),
        system_type: column_input_size(rows.iter().map(|row| row.system_type.as_str()), 4, 8),
        manufacturer: column_input_size(rows.iter().map(|row| row.manufacturer.as_str()), 12, 32),
        name: column_input_size(rows.iter().map(|row| row.name.as_str()), 12, 64),
        short_name: column_input_size(rows.iter().map(|row| row.short_name.as_str()), 10, 32),
        media_types: column_input_size(rows.iter().map(|row| row.media_types_csv.as_str()), 11, 80),
    }
}

fn lookup_input_sizes(rows: &[LookupEditorRow]) -> LookupInputSizes {
    LookupInputSizes {
        code: column_input_size(rows.iter().map(|row| row.code.as_str()), 4, 2),
        name: column_input_size(rows.iter().map(|row| row.name.as_str()), 12, 32),
        sort_order: column_input_size(rows.iter().map(|row| row.sort_order.as_str()), 5, 11),
    }
}

fn column_input_size<'a>(
    values: impl Iterator<Item = &'a str>,
    min_size: usize,
    max_size: usize,
) -> usize {
    values
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0)
        .max(min_size)
        .min(max_size)
}

fn flag_options_for_system(system: &System) -> Vec<SystemFlagOption> {
    SYSTEM_FLAGS
        .iter()
        .map(|flag| SystemFlagOption {
            field: flag.field,
            label: flag.label,
            checked: system_flag_value(system, flag.field),
        })
        .collect()
}

fn flag_options_from_set(flags: &HashSet<String>) -> Vec<SystemFlagOption> {
    SYSTEM_FLAGS
        .iter()
        .map(|flag| SystemFlagOption {
            field: flag.field,
            label: flag.label,
            checked: flags.contains(flag.field),
        })
        .collect()
}

fn system_flag_value(system: &System, field: &str) -> bool {
    match field {
        "has_title_foreign" => system.has_title_foreign,
        "has_disc_number" => system.has_disc_number,
        "has_disc_title" => system.has_disc_title,
        "has_serial" => system.has_serial,
        "has_edition" => system.has_edition,
        "has_barcode" => system.has_barcode,
        "has_version" => system.has_version,
        "has_exe_date" => system.has_exe_date,
        "has_edc" => system.has_edc,
        "has_error_count" => system.has_error_count,
        "has_universal_hash" => system.has_universal_hash,
        "has_protection" => system.has_protection,
        "has_sector_ranges" => system.has_sector_ranges,
        "has_sbi" => system.has_sbi,
        "has_pvd" => system.has_pvd,
        "has_header" => system.has_header,
        "has_bca" => system.has_bca,
        "has_sample_start" => system.has_sample_start,
        "has_offset_extra" => system.has_offset_extra,
        _ => false,
    }
}

fn validate_systems_payload(
    payload: &SystemsPayload,
    existing_systems: &[System],
    media_types: &[MaintenanceMediaType],
) -> Result<Vec<ValidatedSystemRow>, Vec<String>> {
    let existing_codes: HashSet<String> = existing_systems
        .iter()
        .map(|system| system.code.clone())
        .collect();
    let media_codes: HashSet<String> = media_types
        .iter()
        .map(|media_type| media_type.code.clone())
        .collect();
    let valid_flags: HashSet<&'static str> = SYSTEM_FLAGS.iter().map(|flag| flag.field).collect();
    let mut errors = Vec::new();
    let mut original_codes_seen = HashSet::new();
    let mut rows = Vec::new();

    for (index, row) in payload.rows.iter().enumerate() {
        let normalized = normalize_system_payload_row(row);
        if normalized.original_code.is_none() && normalized.is_blank() {
            continue;
        }

        let label = normalized.row_label(index + 1);
        if let Some(original_code) = &normalized.original_code {
            if !existing_codes.contains(original_code) {
                errors.push(format!("{label}: original system does not exist."));
            }
            if !original_codes_seen.insert(original_code.clone()) {
                errors.push(format!("{label}: original system appears more than once."));
            }
        }

        validate_required_and_lengths(&normalized, &label, &mut errors);
        validate_media_types(&normalized, &label, &media_codes, &mut errors);
        validate_flags(&normalized, &label, &valid_flags, &mut errors);
        rows.push(normalized);
    }

    validate_code_collisions(&rows, &existing_codes, &mut errors);

    if errors.is_empty() {
        Ok(rows)
    } else {
        Err(errors)
    }
}

fn normalize_system_payload_row(row: &SystemPayloadRow) -> ValidatedSystemRow {
    let flags = row
        .flags
        .iter()
        .map(|flag| flag.trim().to_string())
        .filter(|flag| !flag.is_empty())
        .collect();
    let media_types = row
        .media_types
        .split(',')
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty())
        .collect();
    ValidatedSystemRow {
        original_code: non_empty_trimmed(&row.original_code),
        code: row.code.trim().to_string(),
        system_type: row.system_type.trim().to_string(),
        manufacturer: row.manufacturer.trim().to_string(),
        name: row.name.trim().to_string(),
        short_name: row.short_name.trim().to_string(),
        media_types,
        flags,
    }
}

fn non_empty_trimmed(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

impl ValidatedSystemRow {
    fn is_blank(&self) -> bool {
        self.code.is_empty()
            && self.system_type.is_empty()
            && self.manufacturer.is_empty()
            && self.name.is_empty()
            && self.short_name.is_empty()
            && self.media_types.is_empty()
    }

    fn row_label(&self, index: usize) -> String {
        if let Some(original_code) = &self.original_code {
            format!("System {original_code}")
        } else if self.code.is_empty() {
            format!("New system row {index}")
        } else {
            format!("New system {}", self.code)
        }
    }

    fn flag(&self, field: &str) -> bool {
        self.flags.contains(field)
    }
}

fn validate_required_and_lengths(row: &ValidatedSystemRow, label: &str, errors: &mut Vec<String>) {
    if row.code.is_empty() {
        errors.push(format!("{label}: code is required."));
    }
    if row.name.is_empty() {
        errors.push(format!("{label}: name is required."));
    }
    validate_length(label, "code", &row.code, 16, errors);
    validate_length(label, "type", &row.system_type, 8, errors);
    validate_length(label, "manufacturer", &row.manufacturer, 32, errors);
    validate_length(label, "name", &row.name, 64, errors);
    validate_length(label, "short_name", &row.short_name, 32, errors);
}

fn validate_length(
    label: &str,
    field: &str,
    value: &str,
    max_len: usize,
    errors: &mut Vec<String>,
) {
    if value.chars().count() > max_len {
        errors.push(format!(
            "{label}: {field} must be {max_len} characters or less."
        ));
    }
}

fn validate_media_types(
    row: &ValidatedSystemRow,
    label: &str,
    media_codes: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    if row.media_types.is_empty() {
        errors.push(format!("{label}: at least one media type is required."));
        return;
    }

    let mut seen = HashSet::new();
    for code in &row.media_types {
        if !media_codes.contains(code) {
            errors.push(format!("{label}: media type {code} does not exist."));
        }
        if !seen.insert(code) {
            errors.push(format!(
                "{label}: media type {code} is selected more than once."
            ));
        }
    }
}

fn validate_flags(
    row: &ValidatedSystemRow,
    label: &str,
    valid_flags: &HashSet<&'static str>,
    errors: &mut Vec<String>,
) {
    for flag in &row.flags {
        if !valid_flags.contains(flag.as_str()) {
            errors.push(format!("{label}: capability flag {flag} does not exist."));
        }
    }
}

fn validate_code_collisions(
    rows: &[ValidatedSystemRow],
    existing_codes: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    let mut final_codes: HashMap<&str, &ValidatedSystemRow> = HashMap::new();
    for row in rows {
        if row.code.is_empty() {
            continue;
        }
        if let Some(previous) = final_codes.insert(row.code.as_str(), row) {
            errors.push(format!(
                "{}: code duplicates {}.",
                row.row_label(0),
                previous.row_label(0)
            ));
        }

        match &row.original_code {
            Some(original_code) if original_code == &row.code => {}
            Some(_) | None => {
                if existing_codes.contains(&row.code) {
                    errors.push(format!(
                        "{}: code {} already exists.",
                        row.row_label(0),
                        row.code
                    ));
                }
            }
        }
    }
}

fn validate_lookup_payload(
    payload: &LookupPayload,
    existing_records: &[LookupRecord],
    flag_options: &[FlagOption],
    table: LookupTable,
) -> Result<Vec<ValidatedLookupRow>, Vec<String>> {
    let existing_codes: HashSet<String> = existing_records
        .iter()
        .map(|record| record.code.trim().to_string())
        .collect();
    let flag_codes: HashSet<String> = flag_options
        .iter()
        .map(|option| option.code.clone())
        .collect();
    let mut errors = Vec::new();
    let mut original_codes_seen = HashSet::new();
    let mut rows = Vec::new();

    for (index, row) in payload.rows.iter().enumerate() {
        let normalized = normalize_lookup_payload_row(row);
        if normalized.original_code.is_none() && lookup_payload_row_is_blank(row) {
            continue;
        }

        let label = normalized.row_label(table, index + 1);
        if let Some(original_code) = &normalized.original_code {
            if !existing_codes.contains(original_code) {
                errors.push(format!(
                    "{label}: original {} does not exist.",
                    table.singular_lower()
                ));
            }
            if !original_codes_seen.insert(original_code.clone()) {
                errors.push(format!(
                    "{label}: original {} appears more than once.",
                    table.singular_lower()
                ));
            }
        }

        validate_lookup_required_and_lengths(&normalized, &label, table, &mut errors);
        validate_lookup_flag_code(&normalized, &label, &flag_codes, &mut errors);
        validate_lookup_sort_order_text(row, &label, &mut errors);
        rows.push(normalized);
    }

    validate_lookup_code_collisions(&rows, &existing_codes, table, &mut errors);

    if errors.is_empty() {
        Ok(rows)
    } else {
        Err(errors)
    }
}

fn normalize_lookup_payload_row(row: &LookupPayloadRow) -> ValidatedLookupRow {
    ValidatedLookupRow {
        original_code: non_empty_trimmed(&row.original_code),
        code: row.code.trim().to_string(),
        name: row.name.trim().to_string(),
        flag_code: row.flag_code.trim().to_string(),
        sort_order: row.sort_order.trim().parse::<i32>().unwrap_or_default(),
    }
}

fn lookup_payload_row_is_blank(row: &LookupPayloadRow) -> bool {
    row.code.trim().is_empty()
        && row.name.trim().is_empty()
        && row.flag_code.trim().is_empty()
        && row.sort_order.trim().is_empty()
}

impl ValidatedLookupRow {
    fn is_blank(&self) -> bool {
        self.code.is_empty()
            && self.name.is_empty()
            && self.flag_code.is_empty()
            && self.sort_order == 0
    }

    fn row_label(&self, table: LookupTable, index: usize) -> String {
        if let Some(original_code) = &self.original_code {
            format!("{} {original_code}", table.singular_title())
        } else if self.code.is_empty() {
            format!("New {} row {index}", table.singular_lower())
        } else {
            format!("New {} {}", table.singular_lower(), self.code)
        }
    }
}

fn validate_lookup_required_and_lengths(
    row: &ValidatedLookupRow,
    label: &str,
    table: LookupTable,
    errors: &mut Vec<String>,
) {
    if row.code.is_empty() {
        errors.push(format!("{label}: code is required."));
    } else if row.code.chars().count() != 2 {
        errors.push(format!("{label}: code must be exactly 2 characters."));
    }
    if row.name.is_empty() {
        errors.push(format!("{label}: name is required."));
    }
    if row.flag_code.is_empty() {
        errors.push(format!("{label}: flag_code is required."));
    }
    validate_length(label, "name", &row.name, table.name_max_len(), errors);
    validate_length(label, "flag_code", &row.flag_code, 16, errors);
}

fn validate_lookup_flag_code(
    row: &ValidatedLookupRow,
    label: &str,
    flag_codes: &HashSet<String>,
    errors: &mut Vec<String>,
) {
    if !row.flag_code.is_empty() && !flag_codes.contains(&row.flag_code) {
        errors.push(format!(
            "{label}: flag_code {} does not match a static flag SVG.",
            row.flag_code
        ));
    }
}

fn validate_lookup_sort_order_text(row: &LookupPayloadRow, label: &str, errors: &mut Vec<String>) {
    let sort_order = row.sort_order.trim();
    if sort_order.is_empty() {
        errors.push(format!("{label}: sort_order is required."));
    } else if sort_order.parse::<i32>().is_err() {
        errors.push(format!("{label}: sort_order must be a valid integer."));
    }
}

fn validate_lookup_code_collisions(
    rows: &[ValidatedLookupRow],
    existing_codes: &HashSet<String>,
    table: LookupTable,
    errors: &mut Vec<String>,
) {
    let mut final_codes: HashMap<&str, &ValidatedLookupRow> = HashMap::new();
    for row in rows {
        if row.code.is_empty() {
            continue;
        }
        if let Some(previous) = final_codes.insert(row.code.as_str(), row) {
            errors.push(format!(
                "{}: code duplicates {}.",
                row.row_label(table, 0),
                previous.row_label(table, 0)
            ));
        }

        match &row.original_code {
            Some(original_code) if original_code == &row.code => {}
            Some(_) | None => {
                if existing_codes.contains(&row.code) {
                    errors.push(format!(
                        "{}: code {} already exists.",
                        row.row_label(table, 0),
                        row.code
                    ));
                }
            }
        }
    }
}

async fn save_validated_systems(
    pool: &sqlx::PgPool,
    rows: &[ValidatedSystemRow],
    existing_systems: &[System],
) -> AppResult<usize> {
    let existing_by_code: HashMap<String, System> = existing_systems
        .iter()
        .map(|system| (system.code.clone(), system.clone()))
        .collect();
    let mut saved = 0;
    let mut tx = pool.begin().await?;

    for row in rows {
        match system_save_action(row, &existing_by_code) {
            SystemSaveAction::Unchanged => {}
            SystemSaveAction::Insert => {
                insert_system(&mut tx, row).await?;
                saved += 1;
            }
            SystemSaveAction::Update => {
                update_system(&mut tx, row).await?;
                saved += 1;
            }
            SystemSaveAction::Rename => {
                let old_code = row.original_code.as_deref().unwrap_or_default();
                insert_system(&mut tx, row).await?;
                sqlx::query(RENAME_DISCS_SYSTEM_SQL)
                    .bind(&row.code)
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(DELETE_SYSTEM_SQL)
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                saved += 1;
            }
        }
    }

    tx.commit().await?;
    Ok(saved)
}

fn system_save_action(
    row: &ValidatedSystemRow,
    existing_by_code: &HashMap<String, System>,
) -> SystemSaveAction {
    let Some(original_code) = &row.original_code else {
        return SystemSaveAction::Insert;
    };
    let Some(existing) = existing_by_code.get(original_code) else {
        return SystemSaveAction::Insert;
    };
    if original_code != &row.code {
        return SystemSaveAction::Rename;
    }
    if system_matches_row(existing, row) {
        SystemSaveAction::Unchanged
    } else {
        SystemSaveAction::Update
    }
}

fn system_matches_row(system: &System, row: &ValidatedSystemRow) -> bool {
    system.code == row.code
        && system.system_type == row.system_type
        && system.manufacturer == row.manufacturer
        && system.name == row.name
        && system.short_name == row.short_name
        && system.media_types == row.media_types
        && SYSTEM_FLAGS
            .iter()
            .all(|flag| system_flag_value(system, flag.field) == row.flag(flag.field))
}

async fn insert_system(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedSystemRow,
) -> Result<(), sqlx::Error> {
    bind_system_fields(sqlx::query(INSERT_SYSTEM_SQL), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn update_system(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedSystemRow,
) -> Result<(), sqlx::Error> {
    bind_system_fields(sqlx::query(UPDATE_SYSTEM_SQL), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn bind_system_fields<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    row: &'q ValidatedSystemRow,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(&row.code)
        .bind(&row.system_type)
        .bind(&row.manufacturer)
        .bind(&row.name)
        .bind(&row.short_name)
        .bind(&row.media_types)
        .bind(row.flag("has_title_foreign"))
        .bind(row.flag("has_disc_number"))
        .bind(row.flag("has_disc_title"))
        .bind(row.flag("has_serial"))
        .bind(row.flag("has_edition"))
        .bind(row.flag("has_barcode"))
        .bind(row.flag("has_version"))
        .bind(row.flag("has_exe_date"))
        .bind(row.flag("has_edc"))
        .bind(row.flag("has_error_count"))
        .bind(row.flag("has_universal_hash"))
        .bind(row.flag("has_protection"))
        .bind(row.flag("has_sector_ranges"))
        .bind(row.flag("has_sbi"))
        .bind(row.flag("has_pvd"))
        .bind(row.flag("has_header"))
        .bind(row.flag("has_bca"))
        .bind(row.flag("has_sample_start"))
        .bind(row.flag("has_offset_extra"))
}

async fn save_validated_lookup_rows(
    pool: &sqlx::PgPool,
    rows: &[ValidatedLookupRow],
    existing_records: &[LookupRecord],
    table: LookupTable,
) -> AppResult<usize> {
    let existing_by_code: HashMap<String, LookupRecord> = existing_records
        .iter()
        .map(|record| (record.code.trim().to_string(), record.clone()))
        .collect();
    let mut saved = 0;
    let mut tx = pool.begin().await?;

    for row in rows {
        match lookup_save_action(row, &existing_by_code) {
            LookupSaveAction::Unchanged => {}
            LookupSaveAction::Insert => {
                insert_lookup_row(&mut tx, row, table).await?;
                saved += 1;
            }
            LookupSaveAction::Update => {
                update_lookup_row(&mut tx, row, table).await?;
                saved += 1;
            }
            LookupSaveAction::Rename => {
                let old_code = row.original_code.as_deref().unwrap_or_default();
                insert_lookup_row(&mut tx, row, table).await?;
                sqlx::query(table.rename_junction_sql())
                    .bind(&row.code)
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(table.delete_sql())
                    .bind(old_code)
                    .execute(&mut *tx)
                    .await?;
                saved += 1;
            }
        }
    }

    tx.commit().await?;
    Ok(saved)
}

fn lookup_save_action(
    row: &ValidatedLookupRow,
    existing_by_code: &HashMap<String, LookupRecord>,
) -> LookupSaveAction {
    let Some(original_code) = &row.original_code else {
        return LookupSaveAction::Insert;
    };
    let Some(existing) = existing_by_code.get(original_code) else {
        return LookupSaveAction::Insert;
    };
    if original_code != &row.code {
        return LookupSaveAction::Rename;
    }
    if lookup_record_matches_row(existing, row) {
        LookupSaveAction::Unchanged
    } else {
        LookupSaveAction::Update
    }
}

fn lookup_record_matches_row(record: &LookupRecord, row: &ValidatedLookupRow) -> bool {
    record.code.trim() == row.code
        && record.name == row.name
        && record.flag_code.trim() == row.flag_code
        && record.sort_order == row.sort_order
}

async fn insert_lookup_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedLookupRow,
    table: LookupTable,
) -> Result<(), sqlx::Error> {
    bind_lookup_fields(sqlx::query(table.insert_sql()), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn update_lookup_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ValidatedLookupRow,
    table: LookupTable,
) -> Result<(), sqlx::Error> {
    bind_lookup_fields(sqlx::query(table.update_sql()), row)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn bind_lookup_fields<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    row: &'q ValidatedLookupRow,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(&row.code)
        .bind(&row.name)
        .bind(&row.flag_code)
        .bind(row.sort_order)
}

async fn rebuild_cue(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<CsrfForm>,
) -> crate::error::AppResult<Response> {
    csrf::verify_form(&user, &form)?;

    Ok(match disc_service::regenerate_all_cue_entries(&state.pool).await {
        Ok(summary) => redirect_with_message(
            "misc",
            "status",
            &format!(
                "Rebuilt CUE for {} disc(s): {} active, {} cue text update(s), {} file metadata upsert(s), {} file metadata delete(s), {} unchanged.",
                summary.total,
                summary.active,
                summary.updated_cues,
                summary.upserted_file_entries,
                summary.deleted_file_entries,
                summary.skipped,
            ),
        ),
        Err(err) => {
            tracing::error!("Failed to rebuild database cue: {err}");
            redirect_with_message("misc", "error", "Failed to rebuild database cue.")
        }
    })
}

async fn trigger_archive_generation(
    State(state): State<AppState>,
    RequireModerator(user): RequireModerator,
    Form(form): Form<CsrfForm>,
) -> crate::error::AppResult<Response> {
    csrf::verify_form(&user, &form)?;

    if let Err(err) = database_export_service::mark_export_dirty() {
        tracing::error!("Failed to mark database export for regeneration: {err}");
        return Ok(redirect_with_message(
            "misc",
            "error",
            "Failed to trigger database export generation.",
        ));
    }

    Ok(
        match archive_service::mark_all_system_archives_dirty(&state.pool).await {
            Ok(count) => redirect_with_message(
                "misc",
                "status",
                &format!(
                    "Triggered archive generation for {count} system(s) and the disc database."
                ),
            ),
            Err(err) => {
                tracing::error!("Failed to trigger archive generation: {err}");
                redirect_with_message("misc", "error", "Failed to trigger archive generation.")
            }
        },
    )
}

fn redirect_with_message(tab: &str, param: &str, message: &str) -> Response {
    let location = format!(
        "/maintenance?tab={}&{param}={}",
        urlencoding::encode(tab),
        urlencoding::encode(message)
    );
    Redirect::to(&location).into_response()
}

fn redirect_submission_with_message(submission_id: i32, param: &str, message: &str) -> Response {
    let location = format!(
        "/maintenance?tab=submissions&submission_id={submission_id}&{param}={}",
        urlencoding::encode(message)
    );
    Redirect::to(&location).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::middleware::AuthenticatedUser;
    use crate::db::models::UserRole;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    #[derive(Template)]
    #[template(
        source = "{% extends \"base.html\" %}{% block title %}Test{% endblock %}{% block content %}{% endblock %}",
        ext = "html"
    )]
    struct BaseMenuTemplate {
        current_user: Option<AuthenticatedUser>,
    }
    impl SiteConfig for BaseMenuTemplate {}

    fn auth_user(role: UserRole) -> AuthenticatedUser {
        AuthenticatedUser {
            id: 1,
            username: "tester".to_string(),
            role,
            csrf_token: "test-csrf-token".to_string(),
            avatar_url: None,
            draft_count: 0,
        }
    }

    fn render_menu(role: UserRole) -> String {
        BaseMenuTemplate {
            current_user: Some(auth_user(role)),
        }
        .render()
        .unwrap()
    }

    fn test_state() -> AppState {
        let database_url = "postgres://postgres:postgres@localhost/postgres".to_string();

        AppState {
            pool: sqlx::postgres::PgPoolOptions::new()
                .connect_lazy(&database_url)
                .unwrap(),
            config: Arc::new(crate::config::Config {
                site_name: "localhost".to_string(),
                database_url,
                site_url: "http://localhost".to_string(),
                base_url: "http://localhost".to_string(),
                wiki_url: "#".to_string(),
                forum_url: "#".to_string(),
                news_feed_url: "#".to_string(),
                asset_version: "dev".to_string(),
                port: 0,
                oidc_provider_url: "#".to_string(),
                oidc_client_id: "test".to_string(),
                oidc_client_secret: "test".to_string(),
            }),
            http: reqwest::Client::new(),
            edition_suggestions: crate::services::disc_service::EditionSuggestionsCache::new(
                Duration::from_secs(60),
            ),
            news_cache: crate::services::news_service::NewsCache::new(Duration::from_secs(
                crate::services::news_service::NEWS_FEED_TTL_SECONDS,
            )),
            homepage_cache: crate::routes::main_page::HomepageCache::new(Duration::from_secs(60)),
            discs_cache: crate::routes::discs::DiscsCache::new(
                Duration::from_secs(60),
                Duration::from_secs(60),
                Duration::from_secs(60),
            ),
            archive_versions_cache:
                crate::services::archive_service::ArchiveVersionsCache::new(),
            transliteration: Arc::new(
                crate::transliteration::TransliterationRegistry::new().unwrap(),
            ),
            draft_count_cache: crate::auth::middleware::DraftCountCache::new(
                Duration::from_secs(60),
            ),
        }
    }

    fn test_media_types() -> Vec<MaintenanceMediaType> {
        vec![
            MaintenanceMediaType {
                code: "cd".to_string(),
                name: "CD".to_string(),
            },
            MaintenanceMediaType {
                code: "dvd5".to_string(),
                name: "DVD-5".to_string(),
            },
        ]
    }

    fn test_system(code: &str, name: &str) -> System {
        System {
            code: code.to_string(),
            system_type: String::new(),
            manufacturer: "Example".to_string(),
            name: name.to_string(),
            short_name: code.to_string(),
            media_types: vec!["cd".to_string()],
            has_exe_date: false,
            has_sbi: false,
            has_pvd: false,
            has_edc: false,
            has_error_count: false,
            has_universal_hash: false,
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
            archives_dirty: false,
        }
    }

    fn test_lookup_record(
        code: &str,
        name: &str,
        flag_code: &str,
        sort_order: i32,
    ) -> LookupRecord {
        LookupRecord {
            code: code.to_string(),
            name: name.to_string(),
            flag_code: flag_code.to_string(),
            sort_order,
        }
    }

    fn test_flag_options() -> Vec<FlagOption> {
        vec![
            FlagOption {
                code: "gb".to_string(),
            },
            FlagOption {
                code: "gb-eng".to_string(),
            },
            FlagOption {
                code: "jp".to_string(),
            },
            FlagOption {
                code: "us".to_string(),
            },
        ]
    }

    fn maintenance_template_for_role(
        role: UserRole,
        tab: Option<&str>,
        backup_files: Vec<BackupFile>,
    ) -> MaintenanceTemplate {
        let systems = vec![test_system("SYS", "System")];
        let system_rows = build_system_editor_rows(&systems);
        let region_rows = build_lookup_editor_rows(&[test_lookup_record("us", "USA", "us", 1)]);
        let language_rows =
            build_lookup_editor_rows(&[test_lookup_record("en", "English", "gb", 1)]);
        maintenance_template(
            auth_user(role),
            MaintenanceQuery {
                tab: tab.map(str::to_string),
                ..Default::default()
            },
            Vec::new(),
            system_rows,
            region_rows,
            language_rows,
            test_flag_options(),
            Vec::new(),
            Vec::new(),
            String::new(),
            None,
            backup_files,
        )
    }

    fn maintenance_template_for_tests(show_misc: bool) -> MaintenanceTemplate {
        maintenance_template_for_role(UserRole::Moderator, show_misc.then_some("misc"), Vec::new())
    }

    fn test_submission_editor() -> MaintenanceSubmissionEditor {
        MaintenanceSubmissionEditor {
            id: 42,
            row_version: "1234".to_string(),
            submission_type: "Disc".to_string(),
            submitter_id: "7".to_string(),
            submitter_username: "Alice".to_string(),
            submission_comment: "Submission comment".to_string(),
            target_disc_id: "99".to_string(),
            changes_original: "{\n  \"old\": true\n}".to_string(),
            changes: "{\n  \"title\": \"Example\"\n}".to_string(),
            dump_log: "dump log".to_string(),
            extra_upload_url: "https://example.com/upload".to_string(),
            submission_token: "submission-token".to_string(),
            submission_fingerprint: "submission-fingerprint".to_string(),
            status: "Pending".to_string(),
            reviewer_id: "8".to_string(),
            reviewer_username: "Bob".to_string(),
            review_comment: "Review comment".to_string(),
            created_at: "2026-09-06T12:34:56Z".to_string(),
            reviewed_at: "2026-09-06T13:34:56Z".to_string(),
        }
    }

    fn test_submission_form() -> SubmissionMaintenanceForm {
        SubmissionMaintenanceForm {
            csrf_token: "test-csrf-token".to_string(),
            row_version: "1234".to_string(),
            submission_type: "Disc".to_string(),
            submitter_id: "7".to_string(),
            submitter_username: "Alice".to_string(),
            submission_comment: String::new(),
            target_disc_id: String::new(),
            changes_original: String::new(),
            changes: "null".to_string(),
            dump_log: String::new(),
            extra_upload_url: String::new(),
            status: "Pending".to_string(),
            reviewer_id: String::new(),
            reviewer_username: String::new(),
            review_comment: String::new(),
            created_at: "2026-09-06T12:34:56.123456Z".to_string(),
            reviewed_at: String::new(),
        }
    }

    fn payload_row(original_code: &str, code: &str, name: &str) -> SystemPayloadRow {
        SystemPayloadRow {
            original_code: original_code.to_string(),
            code: code.to_string(),
            system_type: String::new(),
            manufacturer: "Example".to_string(),
            name: name.to_string(),
            short_name: code.to_string(),
            media_types: "cd".to_string(),
            flags: Vec::new(),
        }
    }

    fn lookup_payload_row(
        original_code: &str,
        code: &str,
        name: &str,
        flag_code: &str,
        sort_order: &str,
    ) -> LookupPayloadRow {
        LookupPayloadRow {
            original_code: original_code.to_string(),
            code: code.to_string(),
            name: name.to_string(),
            flag_code: flag_code.to_string(),
            sort_order: sort_order.to_string(),
        }
    }

    fn validation_errors(payload: SystemsPayload) -> Vec<String> {
        validate_systems_payload(
            &payload,
            &[test_system("SYS", "System"), test_system("OTHER", "Other")],
            &test_media_types(),
        )
        .unwrap_err()
    }

    fn lookup_records(table: LookupTable) -> Vec<LookupRecord> {
        match table {
            LookupTable::Regions => vec![
                test_lookup_record("us", "USA", "us", 1),
                test_lookup_record("jp", "Japan", "jp", 2),
            ],
            LookupTable::Languages => vec![
                test_lookup_record("en", "English", "gb", 1),
                test_lookup_record("ja", "Japanese", "jp", 2),
            ],
        }
    }

    fn lookup_validation_errors(table: LookupTable, payload: LookupPayload) -> Vec<String> {
        validate_lookup_payload(
            &payload,
            &lookup_records(table),
            &test_flag_options(),
            table,
        )
        .unwrap_err()
    }

    fn assert_error_contains(errors: &[String], expected: &str) {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "expected an error containing {expected:?}, got {errors:?}"
        );
    }

    #[test]
    fn user_menu_removes_settings_for_all_roles() {
        for role in [
            UserRole::User,
            UserRole::UserPlus,
            UserRole::Moderator,
            UserRole::Admin,
        ] {
            let html = render_menu(role);
            assert!(!html.contains("Settings"));
        }
    }

    #[test]
    fn user_menu_shows_maintenance_only_to_moderators_and_admins() {
        for role in [UserRole::User, UserRole::UserPlus] {
            let html = render_menu(role);
            assert!(!html.contains(r#"<a href="/maintenance">Maintenance</a>"#));
        }

        for role in [UserRole::Moderator, UserRole::Admin] {
            let html = render_menu(role);
            assert!(html.contains(r#"<a href="/maintenance">Maintenance</a>"#));
        }
    }

    #[test]
    fn user_menu_places_maintenance_above_logout() {
        let html = render_menu(UserRole::Moderator);
        let maintenance_pos = html
            .find(r#"<a href="/maintenance">Maintenance</a>"#)
            .unwrap();
        let logout_pos = html.find(r#"action="/logout""#).unwrap();

        assert!(maintenance_pos < logout_pos);
    }

    #[test]
    fn logged_in_base_template_emits_csrf_meta_and_logout_field() {
        let html = render_menu(UserRole::User);

        assert!(html.contains(r#"<meta name="csrf-token" content="test-csrf-token">"#));
        assert!(html.contains(r#"<input type="hidden" name="_csrf" value="test-csrf-token">"#));
    }

    #[test]
    fn maintenance_forms_include_csrf_fields() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        assert_eq!(
            html.matches(r#"name="_csrf" value="test-csrf-token""#)
                .count(),
            6
        );
        assert!(html.contains(r#"action="/maintenance/systems""#));
        assert!(html.contains(r#"action="/maintenance/regions""#));
        assert!(html.contains(r#"action="/maintenance/languages""#));
        assert!(html.contains(r#"action="/maintenance/rebuild-cue""#));
        assert!(html.contains(r#"action="/maintenance/trigger-archive-generation""#));
    }

    #[test]
    fn maintenance_tabs_place_systems_before_miscellaneous() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        let systems_tab = html.find(r#"href="/maintenance?tab=systems""#).unwrap();
        let regions_tab = html.find(r#"href="/maintenance?tab=regions""#).unwrap();
        let languages_tab = html.find(r#"href="/maintenance?tab=languages""#).unwrap();
        let misc_tab = html.find(r#"href="/maintenance?tab=misc""#).unwrap();
        let systems_panel = html.find(r#"id="maintenance-systems-panel""#).unwrap();
        let regions_panel = html.find(r#"id="maintenance-regions-panel""#).unwrap();
        let languages_panel = html.find(r#"id="maintenance-languages-panel""#).unwrap();
        let misc_panel = html.find(r#"id="maintenance-misc-panel""#).unwrap();

        assert!(systems_tab < misc_tab);
        assert!(systems_tab < regions_tab);
        assert!(regions_tab < languages_tab);
        assert!(languages_tab < misc_tab);
        assert!(systems_panel < misc_panel);
        assert!(systems_panel < regions_panel);
        assert!(regions_panel < languages_panel);
        assert!(languages_panel < misc_panel);
    }

    #[test]
    fn backup_tab_is_visible_only_to_admins() {
        let moderator = maintenance_template_for_role(UserRole::Moderator, None, Vec::new())
            .render()
            .unwrap();
        assert!(!moderator.contains(r#"href="/maintenance?tab=backup""#));
        assert!(!moderator.contains(r#"id="maintenance-backup-panel""#));

        let admin = maintenance_template_for_role(UserRole::Admin, Some("backup"), Vec::new())
            .render()
            .unwrap();
        assert!(admin.contains(r#"href="/maintenance?tab=backup" class="active""#));
        assert!(admin.contains(r#"id="maintenance-backup-panel""#));
    }

    #[test]
    fn users_tab_is_visible_only_to_admins() {
        let moderator = maintenance_template_for_role(UserRole::Moderator, None, Vec::new())
            .render()
            .unwrap();
        assert!(!moderator.contains(r#"href="/maintenance?tab=users""#));
        assert!(!moderator.contains(r#"id="maintenance-users-panel""#));

        let admin = maintenance_template_for_role(UserRole::Admin, Some("users"), Vec::new())
            .render()
            .unwrap();
        assert!(admin.contains(r#"href="/maintenance?tab=users" class="active""#));
        assert!(admin.contains(r#"id="maintenance-users-panel""#));
    }

    #[test]
    fn submissions_tab_is_visible_only_to_admins() {
        let moderator = maintenance_template_for_role(UserRole::Moderator, None, Vec::new())
            .render()
            .unwrap();
        assert!(!moderator.contains(r#"href="/maintenance?tab=submissions""#));
        assert!(!moderator.contains(r#"id="maintenance-submissions-panel""#));

        let admin = maintenance_template_for_role(UserRole::Admin, Some("submissions"), Vec::new())
            .render()
            .unwrap();
        assert!(admin.contains(r#"href="/maintenance?tab=submissions" class="active""#));
        assert!(admin.contains(r#"id="maintenance-submissions-panel""#));
        assert!(admin.contains(r#"name="submission_id""#));
        assert!(admin.contains(
            r#"type="text" id="maintenance-submission-id" name="submission_id" inputmode="numeric""#
        ));
        assert!(!admin.contains(r#"type="number""#));
    }

    #[test]
    fn submissions_tab_renders_repair_fields_and_read_only_metadata() {
        let mut template =
            maintenance_template_for_role(UserRole::Admin, Some("submissions"), Vec::new());
        template.submission_lookup_id = "42".to_string();
        template.submission_users = vec![
            UserIdentity {
                id: 7,
                username: "Alice".to_string(),
            },
            UserIdentity {
                id: 8,
                username: "Bob".to_string(),
            },
        ];
        template.submission_editor = Some(test_submission_editor());

        let html = template.render().unwrap();
        assert!(html.contains(r#"action="/maintenance/submissions/42""#));
        assert!(html.contains(r#"name="_csrf" value="test-csrf-token""#));
        assert!(html.contains(r#"name="row_version" value="1234""#));
        for field in [
            "submission_type",
            "submitter_id",
            "submission_comment",
            "target_disc_id",
            "changes_original",
            "changes",
            "dump_log",
            "extra_upload_url",
            "status",
            "reviewer_id",
            "review_comment",
            "created_at",
            "reviewed_at",
        ] {
            assert!(
                html.contains(&format!(r#"name="{field}""#)),
                "missing {field}"
            );
        }
        assert!(html.contains("submission-token"));
        assert!(html.contains("submission-fingerprint"));
        assert!(!html.contains(r#"name="submission_token""#));
        assert!(!html.contains(r#"name="submission_fingerprint""#));
        assert!(html.contains("not applied to the associated disc"));
        assert_eq!(html.matches("data-maintenance-user-reference>").count(), 2);
        assert!(html
            .contains(r#"type="text" name="target_disc_id" inputmode="numeric" pattern="[0-9]+""#));
        assert!(!html.contains(r#"type="number""#));
    }

    #[test]
    fn submission_user_references_are_local_and_do_not_fetch() {
        let script = include_str!("../../static/js/maintenance.js");
        assert!(script.contains("function initMaintenanceUserReferences()"));
        assert!(script.contains("data-maintenance-user-reference-id"));
        assert!(script.contains("idInput.value = option ? option.value : ''"));
        assert!(script.contains("initMaintenanceUserReferences();"));
        assert!(!script.contains("fetch("));
    }

    #[test]
    fn submission_maintenance_parsers_accept_database_valid_values() {
        assert_eq!(parse_positive_id(" 42 ", "ID").unwrap(), 42);
        assert_eq!(parse_optional_positive_id(" ", "ID").unwrap(), None);
        assert_eq!(parse_submission_type("Edit").unwrap(), SubmissionType::Edit);
        assert_eq!(
            parse_submission_status("Legacy").unwrap(),
            SubmissionStatus::Legacy
        );
        assert_eq!(
            parse_json("null", "Changes", true).unwrap(),
            Some(serde_json::Value::Null)
        );
        assert_eq!(parse_json("", "Original Changes", false).unwrap(), None);
        assert_eq!(
            parse_timestamp("2026-09-06T08:34:56-04:00", "Created At", true)
                .unwrap()
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            "2026-09-06T12:34:56Z"
        );
        assert_eq!(nullable_text(" \n\t"), None);
        assert_eq!(nullable_text(" value ").as_deref(), Some(" value "));
        assert_eq!(
            parse_timestamp("2026-09-06T12:34:56.123456Z", "Created At", true)
                .unwrap()
                .unwrap()
                .timestamp_subsec_micros(),
            123_456
        );
    }

    #[test]
    fn submission_maintenance_parsers_reject_invalid_values() {
        assert!(parse_positive_id("0", "ID").is_err());
        assert!(parse_positive_id("abc", "ID").is_err());
        assert!(parse_submission_type("New").is_err());
        assert!(parse_submission_status("Done").is_err());
        assert!(parse_json("{", "Changes", true).is_err());
        assert!(parse_json("", "Changes", true).is_err());
        assert!(parse_timestamp("2026-09-06 12:34", "Created At", true).is_err());
        assert!(parse_timestamp("", "Created At", true).is_err());
    }

    #[tokio::test]
    async fn submission_validation_collects_constraint_errors_before_database_access() {
        let mut form = test_submission_form();
        form.row_version = "invalid".to_string();
        form.submission_type = "Unknown".to_string();
        form.submitter_id = "not-an-id".to_string();
        form.target_disc_id = "-1".to_string();
        form.changes = "{".to_string();
        form.extra_upload_url = "x".repeat(513);
        form.status = "Approved".to_string();
        form.created_at = "yesterday".to_string();

        let errors = validate_submission_update(&test_state().pool, &form)
            .await
            .unwrap()
            .unwrap_err();
        for expected in [
            "row version",
            "Submission Type",
            "Submitter",
            "Target Disc ID",
            "Changes must be valid JSON",
            "Extra Upload URL",
            "Created At",
            "Reviewed At",
        ] {
            assert_error_contains(&errors, expected);
        }
    }

    #[test]
    fn users_tab_renders_picker_actions_and_counts() {
        let mut template =
            maintenance_template_for_role(UserRole::Admin, Some("users"), Vec::new());
        template.user_rows = vec![MaintenanceUser {
            id: 42,
            username: "Alice".to_string(),
            active_session_count: 3,
        }];

        let html = template.render().unwrap();
        assert!(html.contains(r#"class="maintenance-user-field-label">Username</span>"#));
        assert!(html.contains(r#"id="maintenance-user-input""#));
        assert!(html.contains(r#"class="maintenance-user-select""#));
        assert!(html.contains(r#"value="42" data-username="Alice" data-session-count="3""#));
        assert!(html.contains(r#"id="maintenance-user-rename-form""#));
        assert!(html.contains(r#"id="maintenance-user-logout-form""#));
        assert!(html.contains(r#"id="maintenance-user-delete-form""#));
        assert!(html.contains(r#"id="maintenance-delete-confirmation""#));
        assert!(html.contains(r#"name="confirm_username""#));
        assert!(html.contains(r#"form="maintenance-user-rename-form""#));
        assert!(html.contains(r#"form="maintenance-user-delete-form""#));
        assert_eq!(html.matches(r#"data-lpignore="true""#).count(), 3);
        assert!(!html.contains("maintenance-users-search"));
    }

    #[test]
    fn users_picker_selects_locally_without_search_requests() {
        let script = include_str!("../../static/js/maintenance.js");

        assert!(script.contains("function initMaintenanceUserPicker()"));
        assert!(script.contains("var usersByName = Object.create(null)"));
        assert!(script.contains("select.addEventListener('change'"));
        assert!(script.contains("input.addEventListener('input'"));
        assert!(script.contains("'/maintenance/users/' + option.value + '/rename'"));
        assert!(script.contains("'/maintenance/users/' + option.value + '/logout'"));
        assert!(script.contains("'/maintenance/users/' + option.value + '/delete'"));
        assert!(script.contains("deleteConfirmation.value !== selectedDeleteUsername"));
        assert!(script.contains("selectedDeleteUsername === 'Deleted'"));
        assert!(script.contains("deleteConfirmation.addEventListener('input'"));
        assert!(!script.contains("fetch("));
    }

    #[test]
    fn maintenance_username_validation_trims_and_enforces_limits() {
        assert_eq!(
            normalize_maintenance_username("  New Name  ").unwrap(),
            "New Name"
        );
        assert_eq!(
            normalize_maintenance_username("é").unwrap(),
            "é".to_string()
        );
        assert!(normalize_maintenance_username("   ").is_err());
        assert!(normalize_maintenance_username(&"x".repeat(65)).is_err());
        assert!(normalize_maintenance_username(&"é".repeat(64)).is_ok());
    }

    #[test]
    fn backup_tab_renders_creation_timestamp_in_utc() {
        let html = maintenance_template_for_role(
            UserRole::Admin,
            Some("backup"),
            vec![BackupFile {
                filename: "vgindex-backup-20260703T060000Z.tar.gz".to_string(),
                created_at: "2026-07-03 06:00:00 UTC".to_string(),
                size: "1 MiB".to_string(),
            }],
        )
        .render()
        .unwrap();

        assert!(html.contains("2026-07-03 06:00:00 UTC"));
    }

    #[test]
    fn backup_timestamp_accepts_only_completed_backup_names() {
        assert_eq!(
            backup_timestamp("vgindex-backup-20260703T060000Z.tar.gz").as_deref(),
            Some("2026-07-03 06:00:00 UTC")
        );
        assert!(backup_timestamp(".vgindex-backup-20260703T060000Z.tar.gz.partial").is_none());
        assert!(backup_timestamp("vgindex-backup-20260703.tar.gz").is_none());
        assert!(backup_timestamp("../vgindex-backup-20260703T060000Z.tar.gz").is_none());
    }

    #[test]
    fn backup_listing_sorts_newest_first_and_ignores_partial_files() {
        let directory =
            std::env::temp_dir().join(format!("vgindex-backups-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("vgindex-backup-20260702T060000Z.tar.gz"),
            b"old",
        )
        .unwrap();
        std::fs::write(
            directory.join("vgindex-backup-20260703T060000Z.tar.gz"),
            b"new",
        )
        .unwrap();
        std::fs::write(
            directory.join(".vgindex-backup-20260704T060000Z.tar.gz.partial"),
            b"partial",
        )
        .unwrap();

        let backups = list_backup_files(&directory);
        assert_eq!(backups.len(), 2);
        assert_eq!(
            backups[0].filename,
            "vgindex-backup-20260703T060000Z.tar.gz"
        );
        assert_eq!(
            backups[1].filename,
            "vgindex-backup-20260702T060000Z.tar.gz"
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn maintenance_systems_form_includes_two_line_rows_and_csv_media_field() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        assert!(html.contains(r#"name="systems_payload""#));
        assert!(html.contains("maintenance-system-text-row"));
        assert!(html.contains("maintenance-system-flags-row"));
        assert!(html.contains(r#"data-system-field="code""#));
        assert!(html.contains(r#"data-system-field="media_types""#));
        assert!(html.contains(r#"maxlength="16""#));
        assert!(html.contains(r#"data-system-flag="has_title_foreign""#));
        assert!(html.contains(r#"data-system-flag="has_error_count""#));
        assert!(!html.contains("Add System"));
        assert!(!html.contains("<details"));
        assert!(!html.contains(r#"data-media-list"#));
    }

    #[test]
    fn new_systems_default_to_supporting_error_count() {
        let row = blank_system_editor_row();
        let error_count = row
            .flags
            .iter()
            .find(|flag| flag.field == "has_error_count")
            .unwrap();

        assert!(error_count.checked);
    }

    #[test]
    fn error_count_migration_defaults_enabled_except_for_audio_cd() {
        let migration = include_str!("../../migrations/021_add_system_error_count_flag.sql");

        assert!(migration.contains("has_error_count BOOLEAN NOT NULL DEFAULT TRUE"));
        assert!(migration.contains("WHERE code = 'AUDIO-CD'"));
        assert!(migration.contains("SET has_error_count = FALSE"));
    }

    #[test]
    fn maintenance_regions_and_languages_forms_include_one_line_rows_and_flag_options() {
        let html = maintenance_template_for_tests(false).render().unwrap();

        assert!(html.contains(r#"name="regions_payload""#));
        assert!(html.contains(r#"name="languages_payload""#));
        assert!(html.contains(r#"class="maintenance-simple-row""#));
        assert!(html.contains(r#"data-maintenance-field="flag_code""#));
        assert!(html.contains(r#"data-flag-select"#));
        assert!(html.contains(r#"data-flag-preview"#));
        assert!(html.contains(r#"/static/flags/us.svg"#));
        assert!(html.contains(r#"/static/flags/gb.svg"#));
        assert!(html.contains(r#"<option value="gb-eng""#));
        assert!(html.contains(r#"maxlength="2""#));
        assert!(html.contains(r#"maxlength="32""#));
        assert!(html.contains(r#"maxlength="16""#));
    }

    #[test]
    fn systems_validation_ignores_blank_add_row() {
        let mut blank = SystemPayloadRow::default();
        blank.flags = vec!["has_error_count".to_string()];
        let payload = SystemsPayload { rows: vec![blank] };

        let rows = validate_systems_payload(
            &payload,
            &[test_system("SYS", "System")],
            &test_media_types(),
        )
        .unwrap();

        assert!(rows.is_empty());
    }

    #[test]
    fn systems_validation_rejects_missing_required_fields() {
        let mut row = payload_row("", "", "");
        row.media_types = String::new();
        row.system_type = "Console".to_string();
        let errors = validation_errors(SystemsPayload { rows: vec![row] });

        assert_error_contains(&errors, "code is required");
        assert_error_contains(&errors, "name is required");
        assert_error_contains(&errors, "at least one media type is required");
    }

    #[test]
    fn systems_validation_rejects_invalid_and_duplicate_media() {
        let mut row = payload_row("", "NEW", "New System");
        row.media_types = "cd, bad, cd".to_string();
        let errors = validation_errors(SystemsPayload { rows: vec![row] });

        assert_error_contains(&errors, "media type bad does not exist");
        assert_error_contains(&errors, "media type cd is selected more than once");
    }

    #[test]
    fn systems_validation_rejects_duplicate_codes() {
        let errors = validation_errors(SystemsPayload {
            rows: vec![
                payload_row("", "NEW", "New System"),
                payload_row("", "NEW", "New System 2"),
            ],
        });

        assert_error_contains(&errors, "code duplicates");
    }

    #[test]
    fn systems_validation_rejects_rename_collision() {
        let errors = validation_errors(SystemsPayload {
            rows: vec![payload_row("SYS", "OTHER", "Renamed System")],
        });

        assert_error_contains(&errors, "code OTHER already exists");
    }

    #[test]
    fn lookup_validation_ignores_blank_add_row() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let payload = LookupPayload {
                rows: vec![LookupPayloadRow::default()],
            };
            let rows = validate_lookup_payload(
                &payload,
                &lookup_records(table),
                &test_flag_options(),
                table,
            )
            .unwrap();

            assert!(rows.is_empty());
        }
    }

    #[test]
    fn lookup_validation_rejects_missing_required_fields_and_invalid_sort_order() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let row = lookup_payload_row("", "", "", "", "abc");
            let errors = lookup_validation_errors(table, LookupPayload { rows: vec![row] });

            assert_error_contains(&errors, "code is required");
            assert_error_contains(&errors, "name is required");
            assert_error_contains(&errors, "flag_code is required");
            assert_error_contains(&errors, "sort_order must be a valid integer");
        }
    }

    #[test]
    fn lookup_validation_rejects_duplicate_codes_invalid_flags_and_rename_collisions() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let errors = lookup_validation_errors(
                table,
                LookupPayload {
                    rows: vec![
                        lookup_payload_row("", "zz", "Zulu", "missing", "1"),
                        lookup_payload_row("", "zz", "Zulu 2", "us", "2"),
                    ],
                },
            );

            assert_error_contains(&errors, "flag_code missing does not match");
            assert_error_contains(&errors, "code duplicates");
        }

        let region_errors = lookup_validation_errors(
            LookupTable::Regions,
            LookupPayload {
                rows: vec![lookup_payload_row("us", "jp", "Renamed", "jp", "1")],
            },
        );
        assert_error_contains(&region_errors, "code jp already exists");

        let language_errors = lookup_validation_errors(
            LookupTable::Languages,
            LookupPayload {
                rows: vec![lookup_payload_row("en", "ja", "Renamed", "jp", "1")],
            },
        );
        assert_error_contains(&language_errors, "code ja already exists");
    }

    #[test]
    fn lookup_validation_enforces_table_lengths() {
        let region_errors = lookup_validation_errors(
            LookupTable::Regions,
            LookupPayload {
                rows: vec![lookup_payload_row(
                    "",
                    "abc",
                    "A name that is definitely longer than thirty two characters",
                    "gb-eng-too-long-for-the-column",
                    "1",
                )],
            },
        );
        assert_error_contains(&region_errors, "code must be exactly 2 characters");
        assert_error_contains(&region_errors, "name must be 32 characters or less");
        assert_error_contains(&region_errors, "flag_code must be 16 characters or less");

        let language_errors = lookup_validation_errors(
            LookupTable::Languages,
            LookupPayload {
                rows: vec![lookup_payload_row(
                    "",
                    "zz",
                    "A very long language",
                    "us",
                    "1",
                )],
            },
        );
        assert_error_contains(&language_errors, "name must be 16 characters or less");
    }

    #[test]
    fn system_save_action_plans_unchanged_update_insert_and_rename() {
        let existing = test_system("SYS", "System");
        let existing_by_code = HashMap::from([(existing.code.clone(), existing.clone())]);
        let unchanged = validate_systems_payload(
            &SystemsPayload {
                rows: vec![payload_row("SYS", "SYS", "System")],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);
        let mut changed_payload = payload_row("SYS", "SYS", "System");
        changed_payload.media_types = "cd,dvd5".to_string();
        let update = validate_systems_payload(
            &SystemsPayload {
                rows: vec![changed_payload],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);
        let insert = validate_systems_payload(
            &SystemsPayload {
                rows: vec![payload_row("", "NEW", "New System")],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);
        let rename = validate_systems_payload(
            &SystemsPayload {
                rows: vec![payload_row("SYS", "NEW", "System")],
            },
            std::slice::from_ref(&existing),
            &test_media_types(),
        )
        .unwrap()
        .remove(0);

        assert_eq!(
            system_save_action(&unchanged, &existing_by_code),
            SystemSaveAction::Unchanged
        );
        assert_eq!(
            system_save_action(&update, &existing_by_code),
            SystemSaveAction::Update
        );
        assert_eq!(
            system_save_action(&insert, &existing_by_code),
            SystemSaveAction::Insert
        );
        assert_eq!(
            system_save_action(&rename, &existing_by_code),
            SystemSaveAction::Rename
        );
    }

    #[test]
    fn system_save_sql_marks_changed_rows_archives_dirty() {
        assert!(INSERT_SYSTEM_SQL.contains("archives_dirty"));
        assert!(INSERT_SYSTEM_SQL.contains("TRUE"));
        assert!(UPDATE_SYSTEM_SQL.contains("archives_dirty = TRUE"));
        assert!(RENAME_DISCS_SYSTEM_SQL.contains("UPDATE discs SET system_code"));
    }

    #[test]
    fn lookup_save_action_plans_unchanged_update_insert_and_rename() {
        for table in [LookupTable::Regions, LookupTable::Languages] {
            let existing = test_lookup_record("aa", "Alpha", "us", 1);
            let existing_by_code = HashMap::from([(existing.code.clone(), existing.clone())]);
            let unchanged = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("aa", "aa", "Alpha", "us", "1")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);
            let update = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("aa", "aa", "Alpha", "jp", "2")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);
            let insert = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("", "bb", "Beta", "gb", "2")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);
            let rename = validate_lookup_payload(
                &LookupPayload {
                    rows: vec![lookup_payload_row("aa", "cc", "Alpha", "us", "1")],
                },
                std::slice::from_ref(&existing),
                &test_flag_options(),
                table,
            )
            .unwrap()
            .remove(0);

            assert_eq!(
                lookup_save_action(&unchanged, &existing_by_code),
                LookupSaveAction::Unchanged
            );
            assert_eq!(
                lookup_save_action(&update, &existing_by_code),
                LookupSaveAction::Update
            );
            assert_eq!(
                lookup_save_action(&insert, &existing_by_code),
                LookupSaveAction::Insert
            );
            assert_eq!(
                lookup_save_action(&rename, &existing_by_code),
                LookupSaveAction::Rename
            );
        }
    }

    #[test]
    fn lookup_save_sql_renames_update_disc_junction_tables() {
        assert!(RENAME_DISC_REGIONS_SQL.contains("UPDATE disc_regions SET region_code"));
        assert!(RENAME_DISC_LANGUAGES_SQL.contains("UPDATE disc_languages SET language_code"));
    }

    #[tokio::test]
    async fn maintenance_page_rejects_guests() {
        let response = routes()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/maintenance")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn users_page_rejects_moderators() {
        let response = routes()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/maintenance?tab=users")
                    .extension(crate::auth::middleware::CurrentUser(Some(auth_user(
                        UserRole::Moderator,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn submissions_page_rejects_moderators() {
        let response = routes()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/maintenance?tab=submissions")
                    .extension(crate::auth::middleware::CurrentUser(Some(auth_user(
                        UserRole::Moderator,
                    ))))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn user_admin_routes_reject_moderators() {
        for uri in [
            "/maintenance/users/42/logout",
            "/maintenance/users/42/rename",
            "/maintenance/users/42/delete",
            "/maintenance/submissions/42",
        ] {
            let response = routes()
                .with_state(test_state())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("content-type", "application/x-www-form-urlencoded")
                        .extension(crate::auth::middleware::CurrentUser(Some(auth_user(
                            UserRole::Moderator,
                        ))))
                        .body(Body::from("_csrf=test-csrf-token&new_username=Renamed"))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn user_admin_routes_require_csrf() {
        for (uri, body) in [
            ("/maintenance/users/42/logout", "_csrf=wrong"),
            (
                "/maintenance/users/42/rename",
                "_csrf=wrong&new_username=Renamed",
            ),
            (
                "/maintenance/users/42/delete",
                "_csrf=wrong&confirm_username=Alice",
            ),
            ("/maintenance/submissions/42", "_csrf=wrong"),
        ] {
            let response = routes()
                .with_state(test_state())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("content-type", "application/x-www-form-urlencoded")
                        .extension(crate::auth::middleware::CurrentUser(Some(auth_user(
                            UserRole::Admin,
                        ))))
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }

    async fn migrated_test_pool() -> sqlx::PgPool {
        dotenvy::dotenv().ok();
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for database tests");
        sqlx::PgPool::connect(&database_url).await.unwrap()
    }

    async fn insert_test_disc(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        title: &str,
    ) -> i32 {
        sqlx::query_scalar(
            "INSERT INTO discs (system_code, media_type_code, category_id, title)
             SELECT s.code, mt.code, c.id, $1
             FROM systems s
             CROSS JOIN media_types mt
             CROSS JOIN categories c
             ORDER BY s.code, mt.code, c.id
             LIMIT 1
             RETURNING id",
        )
        .bind(title)
        .fetch_one(&mut **transaction)
        .await
        .unwrap()
    }

    async fn fetch_test_submission(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        submission_id: i32,
    ) -> MaintenanceSubmissionRow {
        sqlx::query_as(
            "SELECT id, submission_type, submitter_id, submission_comment, target_disc_id,
                    changes_original, changes, dump_log, extra_upload_url, submission_token,
                    submission_fingerprint, status, reviewer_id, review_comment, created_at,
                    reviewed_at, xmin::TEXT AS row_version
             FROM disc_submissions WHERE id = $1",
        )
        .bind(submission_id)
        .fetch_one(&mut **transaction)
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn submission_repair_updates_only_editable_submission_fields() {
        let pool = migrated_test_pool().await;
        let mut transaction = pool.begin().await.unwrap();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let submitter_id: i32 =
            sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
                .bind(format!("repair-submitter-{token}"))
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        let reviewer_id: i32 =
            sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
                .bind(format!("repair-reviewer-{token}"))
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        let disc_title = format!("Repair test {token}");
        let disc_id = insert_test_disc(&mut transaction, &disc_title).await;
        let submission_id: i32 = sqlx::query_scalar(
            "INSERT INTO disc_submissions
                (submission_type, submitter_id, changes, status, submission_token,
                 submission_fingerprint)
             VALUES ('Disc', $1, '{}'::jsonb, 'Pending', $2, $3)
             RETURNING id",
        )
        .bind(submitter_id)
        .bind(format!("token-{token}"))
        .bind(format!("fingerprint-{token}"))
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
        let original = fetch_test_submission(&mut transaction, submission_id).await;
        let created_at = DateTime::parse_from_rfc3339("2026-09-06T08:34:56-04:00")
            .unwrap()
            .with_timezone(&Utc);
        let reviewed_at = DateTime::parse_from_rfc3339("2026-09-07T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let update = ValidatedSubmissionUpdate {
            row_version: original.row_version.clone(),
            submission_type: SubmissionType::Edit,
            submitter_id: reviewer_id,
            submission_comment: Some("repaired submission".to_string()),
            target_disc_id: Some(disc_id),
            changes_original: Some(serde_json::json!(["original"])),
            changes: serde_json::Value::Null,
            dump_log: Some("repaired log".to_string()),
            extra_upload_url: Some("https://example.com/repaired".to_string()),
            status: SubmissionStatus::Rejected,
            reviewer_id: Some(submitter_id),
            review_comment: Some("repaired review".to_string()),
            created_at,
            reviewed_at: Some(reviewed_at),
        };

        let SubmissionUpdateOutcome::Updated(changed_fields) =
            update_maintenance_submission_on(&mut transaction, submission_id, &update)
                .await
                .unwrap()
        else {
            panic!("expected an updated submission");
        };
        assert_eq!(changed_fields.len(), 13);

        let repaired = fetch_test_submission(&mut transaction, submission_id).await;
        assert_eq!(repaired.submission_type, SubmissionType::Edit);
        assert_eq!(repaired.submitter_id, reviewer_id);
        assert_eq!(repaired.submission_comment, update.submission_comment);
        assert_eq!(repaired.target_disc_id, Some(disc_id));
        assert_eq!(repaired.changes_original, update.changes_original);
        assert_eq!(repaired.changes, serde_json::Value::Null);
        assert_eq!(repaired.dump_log, update.dump_log);
        assert_eq!(repaired.extra_upload_url, update.extra_upload_url);
        assert_eq!(repaired.status, SubmissionStatus::Rejected);
        assert_eq!(repaired.reviewer_id, Some(submitter_id));
        assert_eq!(repaired.review_comment, update.review_comment);
        assert_eq!(repaired.created_at, created_at);
        assert_eq!(repaired.reviewed_at, Some(reviewed_at));
        assert_eq!(repaired.submission_token, original.submission_token);
        assert_eq!(
            repaired.submission_fingerprint,
            original.submission_fingerprint
        );

        let stored_disc_title: String = sqlx::query_scalar("SELECT title FROM discs WHERE id = $1")
            .bind(disc_id)
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
        assert_eq!(stored_disc_title, disc_title);

        let unchanged = ValidatedSubmissionUpdate {
            row_version: repaired.row_version.clone(),
            submission_type: repaired.submission_type,
            submitter_id: repaired.submitter_id,
            submission_comment: repaired.submission_comment.clone(),
            target_disc_id: repaired.target_disc_id,
            changes_original: repaired.changes_original.clone(),
            changes: repaired.changes.clone(),
            dump_log: repaired.dump_log.clone(),
            extra_upload_url: repaired.extra_upload_url.clone(),
            status: repaired.status,
            reviewer_id: repaired.reviewer_id,
            review_comment: repaired.review_comment.clone(),
            created_at: repaired.created_at,
            reviewed_at: repaired.reviewed_at,
        };
        assert!(matches!(
            update_maintenance_submission_on(&mut transaction, submission_id, &unchanged)
                .await
                .unwrap(),
            SubmissionUpdateOutcome::Unchanged
        ));

        let mut stale = unchanged.clone();
        stale.row_version = "0".to_string();
        assert!(matches!(
            update_maintenance_submission_on(&mut transaction, submission_id, &stale)
                .await
                .unwrap(),
            SubmissionUpdateOutcome::Stale
        ));
        assert!(matches!(
            update_maintenance_submission_on(&mut transaction, i32::MAX, &stale)
                .await
                .unwrap(),
            SubmissionUpdateOutcome::NotFound
        ));

        transaction.rollback().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn deleting_user_creates_deleted_and_transfers_all_attribution() {
        let pool = migrated_test_pool().await;
        let mut transaction = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *transaction)
            .await
            .unwrap();

        let token = uuid::Uuid::new_v4().simple().to_string();
        sqlx::query("UPDATE users SET username = $1 WHERE username = $2")
            .bind(format!("Deleted-test-hidden-{token}"))
            .bind(DELETED_USERNAME)
            .execute(&mut *transaction)
            .await
            .unwrap();
        let source_name = format!("delete-source-{token}");
        let source_id: i32 =
            sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
                .bind(&source_name)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        let other_id: i32 =
            sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
                .bind(format!("delete-other-{token}"))
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        let disc_id = insert_test_disc(&mut transaction, &format!("Delete test {token}")).await;

        sqlx::query("INSERT INTO disc_dumpers (disc_id, user_id, position) VALUES ($1, $2, 3)")
            .bind(disc_id)
            .bind(source_id)
            .execute(&mut *transaction)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO disc_submissions
                (submission_type, submitter_id, changes, status)
             VALUES ('Disc', $1, '{}'::jsonb, 'Pending'),
                    ('Disc', $1, '{}'::jsonb, 'Draft')",
        )
        .bind(source_id)
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO disc_submissions
                (submission_type, submitter_id, changes, status, reviewer_id, reviewed_at)
             VALUES ('Edit', $1, '{}'::jsonb, 'Approved', $2, NOW())",
        )
        .bind(other_id)
        .bind(source_id)
        .execute(&mut *transaction)
        .await
        .unwrap();
        let session_id = format!("delete-session-{token}");
        sqlx::query("INSERT INTO sessions (id, user_id, role) VALUES ($1, $2, 'Admin')")
            .bind(&session_id)
            .bind(source_id)
            .execute(&mut *transaction)
            .await
            .unwrap();

        let result = delete_app_user_on(&mut transaction, source_id, &source_name)
            .await
            .unwrap();
        let DeleteUserResult::Deleted(summary) = result else {
            panic!("expected successful deletion");
        };
        assert_eq!(summary.disc_credits, 1);
        assert_eq!(summary.duplicate_disc_credits, 0);
        assert_eq!(summary.submission_attributions, 2);
        assert_eq!(summary.review_attributions, 1);
        assert_eq!(summary.sessions_revoked, 1);

        let deleted_id: i32 = sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
            .bind(DELETED_USERNAME)
            .fetch_one(&mut *transaction)
            .await
            .unwrap();
        let source_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
                .bind(source_id)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        assert!(!source_exists);
        let credit_position: i32 = sqlx::query_scalar(
            "SELECT position FROM disc_dumpers WHERE disc_id = $1 AND user_id = $2",
        )
        .bind(disc_id)
        .bind(deleted_id)
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
        assert_eq!(credit_position, 3);
        let active_statuses: Vec<String> = sqlx::query_scalar(
            "SELECT status::text FROM disc_submissions
             WHERE submitter_id = $1 AND status IN ('Pending', 'Draft')
             ORDER BY status::text",
        )
        .bind(deleted_id)
        .fetch_all(&mut *transaction)
        .await
        .unwrap();
        assert_eq!(active_statuses, vec!["Draft", "Pending"]);
        let review_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM disc_submissions WHERE reviewer_id = $1")
                .bind(deleted_id)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        assert_eq!(review_count, 1);
        let session_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = $1)")
                .bind(&session_id)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        assert!(!session_exists);
        assert!(matches!(
            delete_app_user_on(&mut transaction, deleted_id, DELETED_USERNAME)
                .await
                .unwrap(),
            DeleteUserResult::Protected
        ));

        transaction.rollback().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn deleting_user_merges_duplicate_disc_credit_at_earliest_position() {
        let pool = migrated_test_pool().await;
        let mut transaction = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *transaction)
            .await
            .unwrap();

        let token = uuid::Uuid::new_v4().simple().to_string();
        let deleted_id: i32 = sqlx::query_scalar(
            "INSERT INTO users (username) VALUES ($1)
             ON CONFLICT (username) DO UPDATE SET username = EXCLUDED.username
             RETURNING id",
        )
        .bind(DELETED_USERNAME)
        .fetch_one(&mut *transaction)
        .await
        .unwrap();
        let source_name = format!("delete-duplicate-{token}");
        let source_id: i32 =
            sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
                .bind(&source_name)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        let disc_id = insert_test_disc(&mut transaction, &format!("Delete merge {token}")).await;
        sqlx::query(
            "INSERT INTO disc_dumpers (disc_id, user_id, position)
             VALUES ($1, $2, 7), ($1, $3, 2)",
        )
        .bind(disc_id)
        .bind(deleted_id)
        .bind(source_id)
        .execute(&mut *transaction)
        .await
        .unwrap();

        let result = delete_app_user_on(&mut transaction, source_id, &source_name)
            .await
            .unwrap();
        let DeleteUserResult::Deleted(summary) = result else {
            panic!("expected successful deletion");
        };
        assert_eq!(summary.disc_credits, 1);
        assert_eq!(summary.duplicate_disc_credits, 1);
        let positions: Vec<i32> = sqlx::query_scalar(
            "SELECT position FROM disc_dumpers WHERE disc_id = $1 AND user_id = $2",
        )
        .bind(disc_id)
        .bind(deleted_id)
        .fetch_all(&mut *transaction)
        .await
        .unwrap();
        assert_eq!(positions, vec![2]);

        transaction.rollback().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a migrated PostgreSQL database"]
    async fn deletion_confirmation_failure_leaves_user_unchanged() {
        let pool = migrated_test_pool().await;
        let mut transaction = pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *transaction)
            .await
            .unwrap();

        let token = uuid::Uuid::new_v4().simple().to_string();
        sqlx::query("UPDATE users SET username = $1 WHERE username = $2")
            .bind(format!("Deleted-test-hidden-{token}"))
            .bind(DELETED_USERNAME)
            .execute(&mut *transaction)
            .await
            .unwrap();
        let source_name = format!("delete-confirm-{token}");
        let source_id: i32 =
            sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
                .bind(&source_name)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();

        let result = delete_app_user_on(&mut transaction, source_id, "wrong case")
            .await
            .unwrap();
        assert!(matches!(
            result,
            DeleteUserResult::ConfirmationMismatch { username } if username == source_name
        ));
        let source_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
                .bind(source_id)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        assert!(source_exists);
        let deleted_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE username = $1)")
                .bind(DELETED_USERNAME)
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        assert!(!deleted_exists);
        assert!(matches!(
            delete_app_user_on(&mut transaction, i32::MAX, "missing")
                .await
                .unwrap(),
            DeleteUserResult::NotFound
        ));

        transaction.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn backup_download_rejects_guests() {
        let response = routes()
            .with_state(test_state())
            .oneshot(
                Request::builder()
                    .uri("/maintenance/backups/vgindex-backup-20260703T060000Z.tar.gz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn maintenance_post_routes_reject_guests() {
        for uri in [
            "/maintenance/systems",
            "/maintenance/regions",
            "/maintenance/languages",
            "/maintenance/rebuild-cue",
            "/maintenance/trigger-archive-generation",
            "/maintenance/users/42/logout",
            "/maintenance/users/42/rename",
            "/maintenance/users/42/delete",
            "/maintenance/submissions/42",
        ] {
            let response = routes()
                .with_state(test_state())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}
