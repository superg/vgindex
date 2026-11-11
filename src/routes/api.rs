use axum::{
    extract::{rejection::PathRejection, DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::auth::{csrf, middleware::RequireAuth};
use crate::db::models::UserRole;
use crate::error::{AppError, AppResult};
use crate::services::{disc_service, redumper_log};
use crate::transliteration::{Script, TransliterationError};
use crate::AppState;

const MAX_API_BODY_BYTES: usize = 4 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/disc/{id}", get(disc_json))
        .route("/api/hash/{hash}", get(discs_by_hash))
        .route("/api/transliterate", post(transliterate))
        .route("/api/parse-redumper-log", post(parse_redumper_log))
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
}

// Machine-parseable JSON for a single disc's info
async fn disc_json(
    State(state): State<AppState>,
    id: Result<Path<i32>, PathRejection>,
) -> AppResult<Json<Value>> {
    let id = crate::routes::path_i32(id)?;
    let detail = disc_service::get_disc_detail(&state.pool, id).await?;
    disc_service::ensure_disc_status_visible(detail.disc.status, false)?;

    let d = &detail.disc;
    let hex = |b: &Option<Vec<u8>>| b.as_ref().map(hex::encode);
    let protection = crate::routes::disc_view::can_show_disc_view_protection(
        &detail.system.code,
        detail.system.has_protection,
        false,
    )
    .then(|| d.protection.clone());

    let ring_codes: Vec<Value> = detail
        .ring_entries
        .iter()
        .map(|e| {
            let layers: Vec<Value> = e
                .layers
                .iter()
                .map(|l| {
                    json!({
                        "layer": l.layer,
                        "mastering_code": l.mastering_code,
                        "mastering_sid": l.mastering_sid,
                        "mould_sids": l.mould_sids,
                        "toolstamps": l.toolstamps,
                        "additional_moulds": l.additional_moulds,
                    })
                })
                .collect();
            json!({
                "offset": e.offset_value,
                "offset_extra": e.offset_extra_value,
                "sample_data_start": e.sample_data_start,
                "comment": e.comment,
                "layers": layers,
            })
        })
        .collect();

    Ok(Json(json!({
        "id": d.id,
        "system": detail.system.code,
        "media_type": d.media_type.code(),
        "category": d.category,
        "status": d.status,
        "title": d.title,
        "title_foreign": d.title_foreign,
        "disc_title": d.disc_title,
        "disc_title_foreign": d.disc_title_foreign,
        "disc_number": d.disc_number,
        "filename_suffix": d.filename_suffix,
        "serial": d.serial,
        "version": d.version,
        "edition": d.edition,
        "barcode": d.barcode,
        "exe_date": d.exe_date,
        "error_count": d.error_count,
        "edc": d.edc,
        "layerbreaks": d.layerbreaks,
        "protection": protection.flatten(),
        "comments": d.comments,
        "contents": d.contents,
        "regions": detail.regions.iter().map(|r| &r.code).collect::<Vec<_>>(),
        "languages": detail.languages.iter().map(|l| &l.code).collect::<Vec<_>>(),
        "added_at": detail.added_at,
        "modified_at": detail.modified_at,
        "files": detail.files.iter().map(|f| json!({
            "track_number": f.track_number,
            "size": f.size,
            "crc32": f.crc32,
            "md5": f.md5,
            "sha1": f.sha1,
        })).collect::<Vec<_>>(),
        "extras": detail.extras.iter().map(|e| json!({
            "type": e.file_type,
            "size": e.size,
            "crc32": e.crc32,
            "md5": e.md5,
            "sha1": e.sha1,
        })).collect::<Vec<_>>(),
        "ring_codes": ring_codes,
        "sector_ranges": detail.sector_ranges.iter()
            .map(|r| [r.range_start, r.range_end]).collect::<Vec<_>>(),
        "cue": d.cue,
        "universal_hash": hex(&d.universal_hash),
        "pvd": hex(&d.pvd),
        "pic": hex(&d.pic),
        "header": hex(&d.header),
        "bca_mastering": detail.bca_mastering.iter().map(hex::encode).collect::<Vec<_>>(),
    })))
}

// List of disc IDs containing provided hash
async fn discs_by_hash(
    State(state): State<AppState>,
    Path(hash): Path<String>,
) -> AppResult<Json<Vec<i32>>> {
    let hash = hash.to_ascii_lowercase();
    let column = match hash.len() {
        8 => "crc32",
        32 => "md5",
        40 => "sha1",
        _ => "",
    };
    if column.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(
            "Invalid CRC32, MD5 or SHA-1 hex string".into(),
        ));
    }

    let universal = if column == "sha1" {
        " UNION SELECT id FROM discs WHERE universal_hash = decode($1, 'hex')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT id FROM discs WHERE status <> 'Disabled' AND id IN \
         (SELECT disc_id FROM files WHERE LOWER({column}) = $1{universal}) ORDER BY id"
    );
    let ids = sqlx::query_scalar(&sql)
        .bind(&hash)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(ids))
}

/// Transliterate a non-Latin title into a Latin-script draft for the Main Title
/// field. Auth-gated: it's an editor helper.
async fn transliterate(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(req): Json<TransliterateRequest>,
) -> Result<Json<TransliterateResponse>, AppError> {
    csrf::verify_headers(&user, &headers)?;

    let result = state
        .transliteration
        .transliterate(&req.text, req.script)
        .map_err(|e| match e {
            // Client problems: empty, too long, or nothing transliterable.
            TransliterationError::EmptyInput
            | TransliterationError::TooLong
            | TransliterationError::UnsupportedScript => AppError::BadRequest(e.to_string()),
            TransliterationError::Backend(msg) => AppError::Internal(msg),
        })?;

    Ok(Json(TransliterateResponse {
        text: result.text,
        script: result.script,
        notes: result.notes,
    }))
}

#[derive(serde::Deserialize)]
struct TransliterateRequest {
    text: String,
    /// Optional explicit script; auto-detected when omitted.
    #[serde(default)]
    script: Option<Script>,
}

#[derive(serde::Serialize)]
struct TransliterateResponse {
    text: String,
    script: Script,
    notes: Vec<String>,
}

/// Parse a redumper log into an Add Disc form draft. Auth and CSRF checks match
/// the other editor helpers; parsing does not persist anything.
async fn parse_redumper_log(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Json(req): Json<ParseRedumperLogRequest>,
) -> Result<Response, AppError> {
    csrf::verify_headers(&user, &headers)?;

    if !can_use_redumper_autofill(user.role, &req.log) {
        return Ok(unsupported_redumper_build_response());
    }

    let systems = disc_service::get_all_systems(&state.pool).await?;
    let known_systems: Vec<(String, Vec<String>)> = systems
        .into_iter()
        .map(|system| (system.code, system.media_types))
        .collect();
    Ok(Json(redumper_log::parse_with_system_media(
        &req.log,
        &known_systems,
    ))
    .into_response())
}

fn can_use_redumper_autofill(role: UserRole, log: &str) -> bool {
    role.can_moderate() || redumper_log::has_supported_autofill_builds(log)
}

const UNSUPPORTED_REDUMPER_BUILD_MESSAGE: &str =
    "Autofill is not supported for this version of redumper. Please update to the latest redumper build.";

fn unsupported_redumper_build_response() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(ParseRedumperLogError {
            error: "unsupported_redumper_build",
            message: UNSUPPORTED_REDUMPER_BUILD_MESSAGE,
        }),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct ParseRedumperLogRequest {
    log: String,
}

#[derive(serde::Serialize)]
struct ParseRedumperLogError {
    error: &'static str,
    message: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redumper_parser_route_is_authenticated_and_csrf_protected() {
        let source = include_str!("api.rs");

        assert!(source.contains(r#".route("/api/parse-redumper-log", post(parse_redumper_log))"#));
        let handler = source
            .split_once("async fn parse_redumper_log")
            .unwrap()
            .1
            .split_once("#[derive(serde::Deserialize)]")
            .unwrap()
            .0;
        assert!(handler.contains("RequireAuth(user): RequireAuth"));
        assert!(handler.contains("csrf::verify_headers(&user, &headers)?"));
        assert!(handler.contains("redumper_log::parse_with_system_media("));
        assert!(
            handler.find("can_use_redumper_autofill").unwrap()
                < handler.find("get_all_systems").unwrap()
        );
    }

    #[test]
    fn moderators_bypass_the_redumper_build_requirement() {
        let unsupported = "redumper (build: LOCAL)";
        assert!(!can_use_redumper_autofill(UserRole::User, unsupported));
        assert!(!can_use_redumper_autofill(UserRole::UserPlus, unsupported));
        assert!(can_use_redumper_autofill(UserRole::Moderator, unsupported));
        assert!(can_use_redumper_autofill(UserRole::Admin, ""));
    }

    #[tokio::test]
    async fn unsupported_redumper_build_response_is_structured_422() {
        let response = unsupported_redumper_build_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"], "unsupported_redumper_build");
        assert_eq!(body["message"], UNSUPPORTED_REDUMPER_BUILD_MESSAGE);
    }
}
