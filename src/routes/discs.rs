use askama::Template;
use axum::{
    extract::{rejection::QueryRejection, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use axum_extra::extract::Query as MultiQuery;
use serde::Deserialize;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};

use crate::auth::middleware::{AuthenticatedUser, CurrentUser};
use crate::config::SiteConfig;
use crate::filters;
use crate::db::models::{format_display_title, DiscStatus};
use crate::error::{AppError, AppResult};
use crate::services::disc_service::{
    self, disc_date_sort_sql, disc_order_by_sql, display_title_sort_sql,
};
use crate::services::queue_service;
use crate::AppState;

use super::compact_query_url;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/discs", get(discs_page))
        .route("/discs/export-csv", get(discs_export))
        .route("/api/disc-dumpers", get(disc_dumpers_directory))
        .route("/api/discs", get(discs_search))
}

/// List of disc IDs containing provided hash
async fn discs_search(
    State(state): State<AppState>,
    query: Result<Query<DiscHashQuery>, QueryRejection>,
) -> AppResult<Json<Vec<i32>>> {
    let Query(q) = query
        .map_err(|_| AppError::BadRequest("Invalid query parameters".into()))?;

    let (column, len, value) = match (&q.crc32, &q.md5, &q.sha1) {
        (Some(v), None, None) => ("crc32", 8, v),
        (None, Some(v), None) => ("md5", 32, v),
        (None, None, Some(v)) => ("sha1", 40, v),
        _ => {
            return Err(AppError::BadRequest(
                "Provide exactly one of 'crc32', 'md5' or 'sha1'".into(),
            ));
        }
    };

    let hash = value.trim().to_ascii_lowercase();
    if hash.len() != len || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(format!(
            "Invalid {column}: expected {len} hex characters"
        )));
    }

    let universal = if column == "sha1" {
        " UNION SELECT id FROM discs WHERE universal_hash = decode($1, 'hex')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT id FROM discs WHERE status <> 'Disabled' AND id IN \
         (SELECT disc_id FROM files WHERE track_number IS NOT NULL \
         AND LOWER({column}) = $1{universal}) ORDER BY id"
    );
    let ids = sqlx::query_scalar(&sql)
        .bind(&hash)
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(ids))
}

#[derive(serde::Deserialize)]
struct DiscHashQuery {
    crc32: Option<String>,
    md5: Option<String>,
    sha1: Option<String>,
}

pub const REFERENCE_CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);
pub const COUNT_CACHE_TTL: Duration = Duration::from_secs(60);
pub const DUMPER_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const COUNT_CACHE_CAPACITY: usize = 2_048;

#[derive(Clone)]
pub struct DiscsCache {
    references: ReferenceCache,
    counts: CountCache,
    dumpers: DumperDirectoryCache,
}

impl DiscsCache {
    pub fn new(reference_ttl: Duration, count_ttl: Duration, dumper_ttl: Duration) -> Self {
        Self {
            references: ReferenceCache::new(reference_ttl),
            counts: CountCache::new(count_ttl, COUNT_CACHE_CAPACITY),
            dumpers: DumperDirectoryCache::new(dumper_ttl),
        }
    }

    pub fn production() -> Self {
        Self::new(REFERENCE_CACHE_TTL, COUNT_CACHE_TTL, DUMPER_CACHE_TTL)
    }

    pub async fn invalidate_dumper_data(&self) {
        let _refresh = self.dumpers.refresh.lock().await;
        *self.dumpers.value.write().await = None;
        self.counts.values.write().await.clear();
    }

    pub async fn invalidate_tag_references(&self) {
        let _refresh = self.references.refresh.lock().await;
        *self.references.value.write().await = None;
        self.counts.values.write().await.clear();
    }
}

#[derive(Clone)]
struct ReferenceCache {
    value: Arc<RwLock<Option<TimedValue<Arc<DiscReferenceData>>>>>,
    refresh: Arc<Mutex<()>>,
    ttl: Duration,
}

impl ReferenceCache {
    fn new(ttl: Duration) -> Self {
        Self {
            value: Arc::new(RwLock::new(None)),
            refresh: Arc::new(Mutex::new(())),
            ttl,
        }
    }

    async fn get(&self, pool: &sqlx::PgPool) -> AppResult<Arc<DiscReferenceData>> {
        if let Some(value) = self.fresh_value().await {
            return Ok(value);
        }

        let _guard = self.refresh.lock().await;
        if let Some(value) = self.fresh_value().await {
            return Ok(value);
        }

        let value = Arc::new(load_disc_reference_data(pool).await?);
        *self.value.write().await = Some(TimedValue::new(value.clone()));
        Ok(value)
    }

    async fn fresh_value(&self) -> Option<Arc<DiscReferenceData>> {
        self.value
            .read()
            .await
            .as_ref()
            .filter(|entry| entry.loaded_at.elapsed() < self.ttl)
            .map(|entry| entry.value.clone())
    }
}

#[derive(Clone)]
struct CountCache {
    values: Arc<RwLock<HashMap<String, TimedValue<i64>>>>,
    loads: Arc<Vec<Arc<Mutex<()>>>>,
    ttl: Duration,
    capacity: usize,
}

impl CountCache {
    fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            values: Arc::new(RwLock::new(HashMap::new())),
            loads: Arc::new((0..64).map(|_| Arc::new(Mutex::new(()))).collect()),
            ttl,
            capacity,
        }
    }

    async fn get(&self, key: &str) -> Option<i64> {
        self.values
            .read()
            .await
            .get(key)
            .filter(|entry| entry.loaded_at.elapsed() < self.ttl)
            .map(|entry| entry.value)
    }

    async fn load_lock(&self, key: &str) -> Arc<Mutex<()>> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        self.loads[hasher.finish() as usize % self.loads.len()].clone()
    }

    async fn insert(&self, key: String, value: i64) {
        let mut values = self.values.write().await;
        values.retain(|_, entry| entry.loaded_at.elapsed() < self.ttl);
        if values.len() >= self.capacity {
            if let Some(oldest) = values
                .iter()
                .min_by_key(|(_, entry)| entry.loaded_at)
                .map(|(key, _)| key.clone())
            {
                values.remove(&oldest);
            }
        }
        values.insert(key, TimedValue::new(value));
    }
}

#[derive(Clone)]
struct DumperDirectoryCache {
    value: Arc<RwLock<Option<TimedValue<Arc<DumperDirectory>>>>>,
    refresh: Arc<Mutex<()>>,
    ttl: Duration,
}

impl DumperDirectoryCache {
    fn new(ttl: Duration) -> Self {
        Self {
            value: Arc::new(RwLock::new(None)),
            refresh: Arc::new(Mutex::new(())),
            ttl,
        }
    }

    async fn get(&self, pool: &sqlx::PgPool) -> AppResult<Arc<DumperDirectory>> {
        if let Some(value) = self.fresh_value().await {
            return Ok(value);
        }

        let _guard = self.refresh.lock().await;
        if let Some(value) = self.fresh_value().await {
            return Ok(value);
        }

        let names: Vec<String> = sqlx::query_scalar(
            "SELECT u.username
             FROM users u
             WHERE EXISTS (SELECT 1 FROM disc_dumpers dd WHERE dd.user_id = u.id)
             ORDER BY LOWER(u.username), u.username",
        )
        .fetch_all(pool)
        .await?;
        let body = serde_json::to_string(&names)
            .map_err(|error| crate::error::AppError::Internal(error.to_string()))?;
        let digest = <sha2::Sha256 as sha2::Digest>::digest(body.as_bytes());
        let value = Arc::new(DumperDirectory {
            etag: format!("\"{}\"", hex::encode(digest)),
            body,
        });
        *self.value.write().await = Some(TimedValue::new(value.clone()));
        Ok(value)
    }

    async fn fresh_value(&self) -> Option<Arc<DumperDirectory>> {
        self.value
            .read()
            .await
            .as_ref()
            .filter(|entry| entry.loaded_at.elapsed() < self.ttl)
            .map(|entry| entry.value.clone())
    }
}

struct TimedValue<T> {
    loaded_at: Instant,
    value: T,
}

impl<T> TimedValue<T> {
    fn new(value: T) -> Self {
        Self {
            loaded_at: Instant::now(),
            value,
        }
    }
}

struct DumperDirectory {
    body: String,
    etag: String,
}

async fn disc_dumpers_directory(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let directory = state.discs_cache.dumpers.get(&state.pool).await?;
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(directory.etag.as_str())
    {
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (
                    header::ETAG,
                    HeaderValue::from_str(&directory.etag).unwrap(),
                ),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=3600"),
                ),
            ],
        )
            .into_response());
    }

    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            ),
            (
                header::ETAG,
                HeaderValue::from_str(&directory.etag).unwrap(),
            ),
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            ),
        ],
        directory.body.clone(),
    )
        .into_response())
}

fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
    }
}

#[derive(Deserialize, Default)]
pub struct DiscsQuery {
    pub system: Option<String>,
    pub region: Option<String>,
    pub language: Option<String>,
    pub media: Option<String>,
    pub category: Option<String>,
    #[serde(default)]
    pub tag: Vec<String>,
    pub tag_mode: Option<String>,
    pub letter: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub title: Option<String>,
    pub title_mode: Option<String>,
    pub title_foreign: Option<String>,
    pub title_foreign_mode: Option<String>,
    pub disc_number: Option<String>,
    pub disc_title: Option<String>,
    pub disc_title_mode: Option<String>,
    pub disc_title_foreign: Option<String>,
    pub disc_title_foreign_mode: Option<String>,
    pub serial: Option<String>,
    pub serial_mode: Option<String>,
    pub filename_suffix: Option<String>,
    pub filename_suffix_mode: Option<String>,
    pub exe_date_min: Option<String>,
    pub exe_date_max: Option<String>,
    pub pvd_date_min: Option<String>,
    pub pvd_date_max: Option<String>,
    pub edition: Option<String>,
    pub edition_mode: Option<String>,
    pub barcode: Option<String>,
    pub barcode_mode: Option<String>,
    pub universal_hash: Option<String>,
    pub tracks_min: Option<String>,
    pub tracks_max: Option<String>,
    pub errors_min: Option<String>,
    pub errors_max: Option<String>,
    pub edc: Option<String>,
    pub protection: Option<String>,
    pub comments: Option<String>,
    pub contents: Option<String>,
    pub mastering_code: Option<String>,
    pub mastering_sid: Option<String>,
    pub toolstamp: Option<String>,
    pub mould_sid: Option<String>,
    pub additional_mould: Option<String>,
    pub offset_min: Option<String>,
    pub offset_max: Option<String>,
    pub volume_label: Option<String>,
    pub volume_label_mode: Option<String>,
    pub internal_serial: Option<String>,
    pub internal_serial_mode: Option<String>,
    pub version: Option<String>,
    pub version_mode: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub dumped_filter: Option<String>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub page: Option<i64>,
    pub dumper: Option<String>,
    pub advanced: Option<String>,
}

const LETTERS: &[&str] = &[
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum HashField {
    Crc32,
    Md5,
    Sha1,
}

impl HashField {
    fn column(self) -> &'static str {
        match self {
            Self::Crc32 => "crc32",
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
        }
    }
}

#[derive(sqlx::FromRow)]
struct HashCandidateRow {
    term: String,
    disc_id: i32,
}

async fn load_hash_candidates(
    pool: &sqlx::PgPool,
    terms: &[String],
) -> AppResult<HashMap<String, Vec<i32>>> {
    let Some(mut builder) = build_hash_candidate_query(terms) else {
        return Ok(HashMap::new());
    };

    let rows: Vec<HashCandidateRow> = builder.build_query_as().fetch_all(pool).await?;
    let mut candidates = HashMap::new();
    for row in rows {
        let disc_ids = candidates.entry(row.term).or_insert_with(Vec::new);
        if !disc_ids.contains(&row.disc_id) {
            disc_ids.push(row.disc_id);
        }
    }
    Ok(candidates)
}

fn build_hash_candidate_query(
    terms: &[String],
) -> Option<sqlx::QueryBuilder<'static, sqlx::Postgres>> {
    let mut by_field: HashMap<HashField, Vec<String>> = HashMap::new();
    let mut universal_hashes = Vec::new();
    for term in terms {
        if let Some(field) = hash_field_for_term(term) {
            let values = by_field.entry(field).or_default();
            if !values.contains(term) {
                values.push(term.clone());
            }
        }
        if let Some(hash) = queue_service::universal_hash_bytes_for_matching(Some(term)) {
            if !universal_hashes.contains(&hash) {
                universal_hashes.push(hash);
            }
        }
    }
    if by_field.is_empty() && universal_hashes.is_empty() {
        return None;
    }

    let mut builder = sqlx::QueryBuilder::<sqlx::Postgres>::new("");
    let mut first = true;
    for field in [HashField::Crc32, HashField::Md5, HashField::Sha1] {
        let Some(values) = by_field.get(&field) else {
            continue;
        };
        if !first {
            builder.push(" UNION ALL ");
        }
        first = false;
        builder.push(format!(
            "SELECT LOWER({column}) AS term, disc_id FROM files WHERE LOWER({column}) = ANY(",
            column = field.column()
        ));
        builder.push_bind(values.clone());
        builder.push(")");
    }

    if !universal_hashes.is_empty() {
        if !first {
            builder.push(" UNION ALL ");
        }
        builder.push(
            "SELECT encode(universal_hash, 'hex') AS term, id AS disc_id \
             FROM discs WHERE universal_hash IN (",
        );
        let mut separated = builder.separated(", ");
        for hash in universal_hashes {
            separated.push_bind(hash);
        }
        separated.push_unseparated(")");
    }

    Some(builder)
}

fn quick_search_terms(input: &str) -> Vec<String> {
    let mut normalized = input.trim().to_lowercase();
    normalized = normalized
        .chars()
        .map(|c| match c {
            ' ' | '_' | '/' | ':' | '&' => '-',
            other => other,
        })
        .collect();
    while normalized.contains("--") {
        normalized = normalized.replace("--", "-");
    }

    normalized
        .trim_matches('-')
        .split('-')
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect()
}

fn hash_field_for_term(term: &str) -> Option<HashField> {
    if !term.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    match term.len() {
        8 => Some(HashField::Crc32),
        32 => Some(HashField::Md5),
        40 => Some(HashField::Sha1),
        _ => None,
    }
}

fn like_escaped(expr: &str) -> String {
    format!(r#"replace(replace(replace({expr}, '\', '\\'), '%', '\%'), '_', '\_')"#)
}

fn like_contains(expr: &str) -> String {
    format!(r#"LIKE '%' || {} || '%' ESCAPE '\'"#, like_escaped(expr))
}

fn accent_aware_contains(column: &str, bind: &str) -> String {
    format!(
        "(search_fold({column}) {} AND (search_fold({bind}) = LOWER({bind}) OR LOWER({column}) {}))",
        like_contains(&format!("search_fold({bind})")),
        like_contains(&format!("LOWER({bind})")),
    )
}

fn quick_search_clause(bind_idx: u32, hash_candidate_bind_idx: Option<u32>) -> String {
    let bind = format!("${bind_idx}");
    let title_match = accent_aware_contains("d.title", &bind);
    let title_foreign_match = accent_aware_contains("d.title_foreign", &bind);
    let serial_like = like_contains(&bind);
    let mut clause = format!(
        r#"({title_match}
             OR {title_foreign_match}
             OR compact_disc_array_search(d.serial) {serial_like}"#
    );

    if let Some(candidate_bind_idx) = hash_candidate_bind_idx {
        clause.push_str(&format!(
            r#"
             OR d.id = ANY(${candidate_bind_idx})"#
        ));
    }

    clause.push(')');
    clause
}

fn active_advanced_filter(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn advanced_panel_explicitly_open(value: Option<&str>) -> bool {
    value == Some("1")
}

fn active_verbatim_filter(value: Option<&String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty()).cloned()
}

fn array_text_bind_value(value: &str, exact: bool) -> String {
    if exact {
        value.to_string()
    } else {
        value
            .chars()
            .filter(|character| !character.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextMatchMode {
    Contains,
    Exact,
    IsNull,
    Any,
}

impl TextMatchMode {
    fn from_query(mode: Option<&str>) -> Self {
        match mode {
            Some("exact") => Self::Exact,
            Some("null") => Self::IsNull,
            Some("any") => Self::Any,
            _ => Self::Contains,
        }
    }

    fn as_query_value(self) -> &'static str {
        match self {
            Self::Contains => "",
            Self::Exact => "exact",
            Self::IsNull => "null",
            Self::Any => "any",
        }
    }

    fn is_exact(self) -> bool {
        self == Self::Exact
    }

    fn searches_text(self) -> bool {
        matches!(self, Self::Contains | Self::Exact)
    }
}

fn scalar_text_bind_value(value: &str, exact: bool) -> String {
    if exact {
        value.to_string()
    } else {
        value.trim().to_string()
    }
}

fn universal_hash_search_clause(bind_idx: u32) -> String {
    format!("d.universal_hash = ${bind_idx}")
}

fn add_universal_hash_clause(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    active: bool,
    hash_bytes: Option<&[u8]>,
) {
    if !active {
        return;
    }
    if hash_bytes.is_some() {
        *bind_idx += 1;
        where_clauses.push(universal_hash_search_clause(*bind_idx));
    } else {
        where_clauses.push("FALSE".to_owned());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarTextField {
    Title,
    ForeignTitle,
    DiscNumber,
    DiscTitle,
    ForeignDiscTitle,
    FilenameSuffix,
    VolumeLabel,
    InternalSerial,
    Version,
}

impl ScalarTextField {
    fn column(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::ForeignTitle => "title_foreign",
            Self::DiscNumber => "disc_number",
            Self::DiscTitle => "disc_title",
            Self::ForeignDiscTitle => "disc_title_foreign",
            Self::FilenameSuffix => "filename_suffix",
            Self::VolumeLabel => "volume_label",
            Self::InternalSerial => "internal_serial",
            Self::Version => "version",
        }
    }

    fn capability(self) -> Option<&'static str> {
        match self {
            Self::Title
            | Self::FilenameSuffix
            | Self::VolumeLabel
            | Self::InternalSerial
            | Self::Version => None,
            Self::ForeignTitle => Some("has_title_foreign"),
            Self::DiscNumber => Some("has_disc_number"),
            Self::DiscTitle => Some("has_disc_title"),
            Self::ForeignDiscTitle => Some("has_disc_title_foreign"),
        }
    }

    fn uses_accent_folding(self) -> bool {
        matches!(
            self,
            Self::Title | Self::ForeignTitle | Self::DiscTitle | Self::ForeignDiscTitle
        )
    }
}

fn scalar_text_search_clause(field: ScalarTextField, bind_idx: u32, exact: bool) -> String {
    let column = field.column();
    let predicate = if exact {
        format!("d.{column} = ${bind_idx}")
    } else if field.uses_accent_folding() {
        accent_aware_contains(&format!("d.{column}"), &format!("${bind_idx}"))
    } else {
        format!(
            "LOWER(d.{column}) {}",
            like_contains(&format!("LOWER(${bind_idx})"))
        )
    };

    match field.capability() {
        Some(capability) => format!("(s.{capability} AND {predicate})"),
        None => predicate,
    }
}

fn scalar_text_null_clause(field: ScalarTextField) -> String {
    format!("d.{} IS NULL", field.column())
}

fn scalar_text_any_clause(field: ScalarTextField) -> String {
    let predicate = format!("d.{} IS NOT NULL", field.column());
    match field.capability() {
        Some(capability) => format!("(s.{capability} AND {predicate})"),
        None => predicate,
    }
}

fn add_scalar_text_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    filters: [(ScalarTextField, bool, TextMatchMode); 9],
) {
    for (field, active, mode) in filters {
        match mode {
            TextMatchMode::Contains | TextMatchMode::Exact if active => {
                *bind_idx += 1;
                where_clauses.push(scalar_text_search_clause(field, *bind_idx, mode.is_exact()));
            }
            TextMatchMode::IsNull => where_clauses.push(scalar_text_null_clause(field)),
            TextMatchMode::Any => where_clauses.push(scalar_text_any_clause(field)),
            TextMatchMode::Contains | TextMatchMode::Exact => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayTextField {
    Serial,
    Edition,
    Barcode,
}

impl ArrayTextField {
    fn column(self) -> &'static str {
        match self {
            Self::Serial => "serial",
            Self::Edition => "edition",
            Self::Barcode => "barcode",
        }
    }

    fn capability(self) -> &'static str {
        match self {
            Self::Serial => "has_serial",
            Self::Edition => "has_edition",
            Self::Barcode => "has_barcode",
        }
    }
}

fn array_text_search_clause(field: ArrayTextField, bind_idx: u32, exact: bool) -> String {
    let column = field.column();
    let capability = field.capability();
    if exact {
        format!(
            "(s.{capability} AND compact_disc_array_search(d.{column}) LIKE '%' || compact_disc_array_search(ARRAY[${bind_idx}]::TEXT[]) || '%' AND EXISTS (SELECT 1 FROM unnest(d.{column}) AS filter_value(value) WHERE filter_value.value = ${bind_idx}))"
        )
    } else {
        let compact_like = like_contains(&format!("${bind_idx}"));
        let entry_like = like_contains(&format!("${bind_idx}"));
        format!(
            "(s.{capability} AND compact_disc_array_search(d.{column}) {compact_like} AND EXISTS (SELECT 1 FROM unnest(d.{column}) AS filter_value(value) WHERE LOWER(REGEXP_REPLACE(filter_value.value, '[[:space:]]+', '', 'g')) {entry_like}))"
        )
    }
}

fn array_text_empty_clause(field: ArrayTextField) -> String {
    format!("COALESCE(cardinality(d.{}), 0) = 0", field.column())
}

fn array_text_any_clause(field: ArrayTextField) -> String {
    format!(
        "(s.{} AND COALESCE(cardinality(d.{}), 0) > 0)",
        field.capability(),
        field.column()
    )
}

fn add_array_text_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    filters: [(ArrayTextField, bool, TextMatchMode); 3],
) {
    for (field, active, mode) in filters {
        match mode {
            TextMatchMode::Contains | TextMatchMode::Exact if active => {
                *bind_idx += 1;
                where_clauses.push(array_text_search_clause(field, *bind_idx, mode.is_exact()));
            }
            TextMatchMode::IsNull => where_clauses.push(array_text_empty_clause(field)),
            TextMatchMode::Any => where_clauses.push(array_text_any_clause(field)),
            TextMatchMode::Contains | TextMatchMode::Exact => {}
        }
    }
}

fn comments_search_clause(bind_idx: u32) -> String {
    accent_aware_contains("d.comments", &format!("${bind_idx}"))
}

fn contents_search_clause(bind_idx: u32) -> String {
    accent_aware_contains("d.contents", &format!("${bind_idx}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RingTextField {
    MasteringCode,
    MasteringSid,
    Toolstamp,
    MouldSid,
    AdditionalMould,
}

impl RingTextField {
    fn column(self) -> &'static str {
        match self {
            Self::MasteringCode => "mastering_code",
            Self::MasteringSid => "mastering_sid",
            Self::Toolstamp => "toolstamps",
            Self::MouldSid => "mould_sids",
            Self::AdditionalMould => "additional_moulds",
        }
    }
}

fn ring_text_search_clause(field: RingTextField, bind_idx: u32) -> String {
    let column = field.column();
    let ring_like = like_contains(&format!("LOWER(${bind_idx})"));
    format!(
        "EXISTS (SELECT 1 FROM disc_ring_code_entries ring_entry JOIN disc_ring_code_layers ring_layer ON ring_layer.entry_id = ring_entry.id WHERE ring_entry.disc_id = d.id AND public.ringcode_field_search_text(ring_layer.{column}) {ring_like})"
    )
}

fn normalize_offset_filter(value: Option<&str>) -> Option<i32> {
    crate::services::validation::validate_signed_int(value?).ok()
}

fn normalize_date_filter(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let bytes = value.as_bytes();
    match value.len() {
        4 if bytes.iter().all(|byte| byte.is_ascii_digit()) => Some(value.to_string()),
        7 if bytes[4] == b'-'
            && bytes[..4].iter().all(|byte| byte.is_ascii_digit())
            && bytes[5..].iter().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(value.to_string())
        }
        10 if bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[..4].iter().all(|byte| byte.is_ascii_digit())
            && bytes[5..7].iter().all(|byte| byte.is_ascii_digit())
            && bytes[8..].iter().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(value.to_string())
        }
        _ => None,
    }
}

fn pvd_date_bound_bytes(date: &str, fill: u8) -> Vec<u8> {
    let mut bytes: Vec<u8> = date.bytes().filter(|byte| *byte != b'-').collect();
    bytes.resize(8, fill);
    bytes
}

fn add_pvd_date_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    pvd_date_min: Option<&str>,
    pvd_date_max: Option<&str>,
) {
    const PVD_CREATION_DATE: &str = "CASE
        WHEN octet_length(d.pvd) = 70
        THEN substring(d.pvd from 7 for 8)
        ELSE substring(d.pvd from 14 for 8)
    END";

    if pvd_date_min.is_none() && pvd_date_max.is_none() {
        return;
    }

    where_clauses.push(format!(
        "encode(({PVD_CREATION_DATE}), 'hex') ~ '^([3-9a-f][0-9a-f]){{8}}$'"
    ));

    if pvd_date_min.is_some() {
        *bind_idx += 1;
        where_clauses.push(format!("({PVD_CREATION_DATE}) >= ${}", *bind_idx));
    }
    if pvd_date_max.is_some() {
        *bind_idx += 1;
        where_clauses.push(format!("({PVD_CREATION_DATE}) <= ${}", *bind_idx));
    }
}

fn offset_range_clause(min_bind: Option<u32>, max_bind: Option<u32>) -> String {
    let value_range = |column: &str| {
        let mut conditions = Vec::new();
        if let Some(bind_idx) = min_bind {
            conditions.push(format!("{column} >= ${bind_idx}"));
        }
        if let Some(bind_idx) = max_bind {
            conditions.push(format!("{column} <= ${bind_idx}"));
        }
        conditions.join(" AND ")
    };
    let primary_range = value_range("offset_entry.offset_value");
    let extra_range = value_range("offset_entry.offset_extra_value");
    format!(
        "EXISTS (SELECT 1 FROM disc_ring_code_entries offset_entry WHERE offset_entry.disc_id = d.id AND (({primary_range}) OR (s.has_offset_extra AND ({extra_range}))))"
    )
}

fn add_ring_filter_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    filters: [(RingTextField, bool); 5],
    offset_min_active: bool,
    offset_max_active: bool,
) {
    for (field, active) in filters {
        if active {
            *bind_idx += 1;
            where_clauses.push(ring_text_search_clause(field, *bind_idx));
        }
    }
    let min_bind = if offset_min_active {
        *bind_idx += 1;
        Some(*bind_idx)
    } else {
        None
    };
    let max_bind = if offset_max_active {
        *bind_idx += 1;
        Some(*bind_idx)
    } else {
        None
    };
    if min_bind.is_some() || max_bind.is_some() {
        where_clauses.push(offset_range_clause(min_bind, max_bind));
    }
}

fn protection_search_clause(bind_idx: u32, is_logged_in: bool) -> String {
    let visibility = if is_logged_in {
        "s.has_protection"
    } else {
        "s.has_protection AND s.code NOT IN ('BD-VIDEO', 'HDDVD-VIDEO')"
    };

    format!(
        "({visibility} AND LOWER(d.protection) {})",
        like_contains(&format!("LOWER(${bind_idx})"))
    )
}

fn normalize_status_filter(status: Option<&str>, can_view_disabled_discs: bool) -> String {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "all statuses" | "disabled" if !can_view_disabled_discs => String::new(),
        "all statuses" => "All Statuses".to_string(),
        "disabled" => "Disabled".to_string(),
        "questionable" => "Questionable".to_string(),
        "verified" => "Verified".to_string(),
        "unverified" => "Unverified".to_string(),
        "trusted" => "Trusted".to_string(),
        _ => String::new(),
    }
}

fn normalize_system_filter(system: Option<&str>) -> String {
    system.unwrap_or_default().trim().to_ascii_uppercase()
}

fn normalize_region_filter(region: Option<&str>) -> String {
    region.unwrap_or_default().trim().to_ascii_lowercase()
}

fn normalize_media_filter(media: Option<&str>) -> String {
    media.unwrap_or_default().trim().to_ascii_lowercase()
}

fn normalize_language_filter(language: Option<&str>) -> String {
    language.unwrap_or_default().trim().to_ascii_lowercase()
}

fn normalize_tag_mode(mode: Option<&str>) -> String {
    match mode.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "and" => "and".to_string(),
        "not" => "not".to_string(),
        _ => String::new(),
    }
}

fn normalize_non_negative_bound(value: Option<&str>) -> Option<i32> {
    value?
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|value| *value >= 0)
}

fn normalize_edc_filter(edc: Option<&str>) -> String {
    match edc.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "yes" => "yes".to_string(),
        "no" => "no".to_string(),
        _ => String::new(),
    }
}

fn normalize_letter_filter(letter: Option<&str>) -> String {
    let letter = letter.unwrap_or_default().trim();
    if letter == "#" {
        "#".to_string()
    } else if letter.len() == 1 && letter.chars().next().unwrap().is_ascii_alphabetic() {
        letter.to_ascii_uppercase()
    } else {
        letter.to_string()
    }
}

fn normalize_disc_sort(sort: Option<&str>) -> String {
    match sort.unwrap_or("title").trim().to_ascii_lowercase().as_str() {
        "region" | "title" | "system" | "version" | "edition" | "language" | "serial"
        | "status" | "added" | "modified" => sort.unwrap_or("title").trim().to_ascii_lowercase(),
        _ => "title".to_string(),
    }
}

fn normalize_sort_order(order: Option<&str>) -> String {
    if order
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("desc")
    {
        "desc".to_string()
    } else {
        "asc".to_string()
    }
}

fn normalize_dumped_filter(filter: Option<&str>) -> String {
    match filter
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "dumped" => "dumped".to_string(),
        "not_dumped" => "not_dumped".to_string(),
        _ => String::new(),
    }
}

#[derive(Template)]
#[template(path = "discs.html")]
struct DiscsTemplate {
    current_user: Option<AuthenticatedUser>,
    can_view_disabled_discs: bool,
    discs: Vec<DiscRow>,
    systems: Vec<SystemOption>,
    regions: Vec<RegionOption>,
    languages: Vec<LanguageOption>,
    media_types: Vec<MediaOption>,
    categories: Vec<CategoryOption>,
    tags: Vec<TagFilterOption>,
    systems_media_json: String,
    letters: Vec<(String, bool)>,
    filter_system: String,
    filter_region: String,
    filter_language: String,
    filter_media: String,
    filter_category: String,
    selected_tag_count: usize,
    filter_tags: Vec<String>,
    filter_tag_mode: String,
    filter_status: String,
    filter_letter: String,
    filter_q: String,
    filter_title: String,
    title_mode: String,
    filter_title_foreign: String,
    title_foreign_mode: String,
    filter_disc_number: String,
    filter_disc_title: String,
    disc_title_mode: String,
    filter_disc_title_foreign: String,
    disc_title_foreign_mode: String,
    filter_serial: String,
    serial_mode: String,
    filter_filename_suffix: String,
    filename_suffix_mode: String,
    filter_exe_date_min: String,
    filter_exe_date_max: String,
    filter_pvd_date_min: String,
    filter_pvd_date_max: String,
    filter_edition: String,
    edition_mode: String,
    filter_barcode: String,
    barcode_mode: String,
    filter_universal_hash: String,
    filter_tracks_min: String,
    filter_tracks_max: String,
    tracks_exact: bool,
    filter_errors_min: String,
    filter_errors_max: String,
    errors_exact: bool,
    filter_edc: String,
    filter_protection: String,
    filter_comments: String,
    filter_contents: String,
    filter_mastering_code: String,
    filter_mastering_sid: String,
    filter_toolstamp: String,
    filter_mould_sid: String,
    filter_additional_mould: String,
    filter_offset_min: String,
    filter_offset_max: String,
    offset_exact: bool,
    filter_volume_label: String,
    volume_label_mode: String,
    filter_internal_serial: String,
    internal_serial_mode: String,
    filter_version: String,
    version_mode: String,
    advanced_open: bool,
    advanced_explicit: bool,
    filter_dumper: String,
    filter_dumper_name: String,
    total_count: i64,
    show_csv_export: bool,
    page: i64,
    total_pages: i64,
    prev_page: i64,
    next_page: i64,
    sort_column: String,
    sort_order: String,
    dumped_filter: String,
    next_region_order: String,
    next_title_order: String,
    next_system_order: String,
    next_version_order: String,
    next_edition_order: String,
    next_language_order: String,
    next_serial_order: String,
    next_status_order: String,
}
impl SiteConfig for DiscsTemplate {}

impl DiscsTemplate {
    fn url(&self, page: i64, letter: &str, sort: &str, order: &str, dumped_filter: &str) -> String {
        build_discs_url(DiscsUrlOptions {
            system: &self.filter_system,
            region: &self.filter_region,
            language: &self.filter_language,
            media: &self.filter_media,
            category: &self.filter_category,
            tags: &self.filter_tags,
            tag_mode: &self.filter_tag_mode,
            status: &self.filter_status,
            letter,
            dumper: &self.filter_dumper,
            title: &self.filter_title,
            title_mode: &self.title_mode,
            title_foreign: &self.filter_title_foreign,
            title_foreign_mode: &self.title_foreign_mode,
            disc_number: &self.filter_disc_number,
            disc_title: &self.filter_disc_title,
            disc_title_mode: &self.disc_title_mode,
            disc_title_foreign: &self.filter_disc_title_foreign,
            disc_title_foreign_mode: &self.disc_title_foreign_mode,
            serial: &self.filter_serial,
            serial_mode: &self.serial_mode,
            filename_suffix: &self.filter_filename_suffix,
            filename_suffix_mode: &self.filename_suffix_mode,
            exe_date_min: &self.filter_exe_date_min,
            exe_date_max: &self.filter_exe_date_max,
            pvd_date_min: &self.filter_pvd_date_min,
            pvd_date_max: &self.filter_pvd_date_max,
            edition: &self.filter_edition,
            edition_mode: &self.edition_mode,
            barcode: &self.filter_barcode,
            barcode_mode: &self.barcode_mode,
            universal_hash: &self.filter_universal_hash,
            tracks_min: &self.filter_tracks_min,
            tracks_max: &self.filter_tracks_max,
            errors_min: &self.filter_errors_min,
            errors_max: &self.filter_errors_max,
            edc: &self.filter_edc,
            protection: &self.filter_protection,
            comments: &self.filter_comments,
            contents: &self.filter_contents,
            mastering_code: &self.filter_mastering_code,
            mastering_sid: &self.filter_mastering_sid,
            toolstamp: &self.filter_toolstamp,
            mould_sid: &self.filter_mould_sid,
            additional_mould: &self.filter_additional_mould,
            offset_min: &self.filter_offset_min,
            offset_max: &self.filter_offset_max,
            volume_label: &self.filter_volume_label,
            volume_label_mode: &self.volume_label_mode,
            internal_serial: &self.filter_internal_serial,
            internal_serial_mode: &self.internal_serial_mode,
            version: &self.filter_version,
            version_mode: &self.version_mode,
            q: &self.filter_q,
            sort,
            order,
            dumped_filter,
            page,
            advanced: self.advanced_explicit,
        })
    }

    fn page_url(&self, page: &i64) -> String {
        self.url(
            *page,
            &self.filter_letter,
            &self.sort_column,
            &self.sort_order,
            &self.dumped_filter,
        )
    }

    fn letter_url(&self, letter: &str) -> String {
        self.url(1, letter, &self.sort_column, &self.sort_order, &self.dumped_filter)
    }

    fn sort_url(&self, sort: &str, order: &str) -> String {
        self.url(1, &self.filter_letter, sort, order, &self.dumped_filter)
    }

    fn export_url(&self) -> String {
        self
            .url(1, &self.filter_letter, &self.sort_column, &self.sort_order, &self.dumped_filter)
            .replacen("/discs", "/discs/export-csv", 1)
    }

    fn dumped_filter_url(&self) -> String {
        let next_filter = match self.dumped_filter.as_str() {
            "dumped" => "not_dumped",
            "not_dumped" => "",
            _ => "dumped",
        };
        self.url(1, &self.filter_letter, &self.sort_column, &self.sort_order, next_filter)
    }
}

#[derive(Clone, Copy, Default)]
struct DiscsUrlOptions<'a> {
    system: &'a str,
    region: &'a str,
    language: &'a str,
    media: &'a str,
    category: &'a str,
    tags: &'a [String],
    tag_mode: &'a str,
    status: &'a str,
    letter: &'a str,
    dumper: &'a str,
    title: &'a str,
    title_mode: &'a str,
    title_foreign: &'a str,
    title_foreign_mode: &'a str,
    disc_number: &'a str,
    disc_title: &'a str,
    disc_title_mode: &'a str,
    disc_title_foreign: &'a str,
    disc_title_foreign_mode: &'a str,
    serial: &'a str,
    serial_mode: &'a str,
    filename_suffix: &'a str,
    filename_suffix_mode: &'a str,
    exe_date_min: &'a str,
    exe_date_max: &'a str,
    pvd_date_min: &'a str,
    pvd_date_max: &'a str,
    edition: &'a str,
    edition_mode: &'a str,
    barcode: &'a str,
    barcode_mode: &'a str,
    universal_hash: &'a str,
    tracks_min: &'a str,
    tracks_max: &'a str,
    errors_min: &'a str,
    errors_max: &'a str,
    edc: &'a str,
    protection: &'a str,
    comments: &'a str,
    contents: &'a str,
    mastering_code: &'a str,
    mastering_sid: &'a str,
    toolstamp: &'a str,
    mould_sid: &'a str,
    additional_mould: &'a str,
    offset_min: &'a str,
    offset_max: &'a str,
    volume_label: &'a str,
    volume_label_mode: &'a str,
    internal_serial: &'a str,
    internal_serial_mode: &'a str,
    version: &'a str,
    version_mode: &'a str,
    q: &'a str,
    sort: &'a str,
    order: &'a str,
    dumped_filter: &'a str,
    page: i64,
    advanced: bool,
}

fn build_discs_count_cache_key(
    mut options: DiscsUrlOptions<'_>,
    member_protection_visibility: bool,
    dumped_user_id: Option<i32>,
) -> String {
    options.sort = "title";
    options.order = "asc";
    options.page = 1;
    options.advanced = false;
    let dumped_user_scope = if options.dumped_filter.is_empty() {
        "none".to_string()
    } else {
        dumped_user_id
            .map(|user_id| user_id.to_string())
            .unwrap_or_else(|| "none".to_string())
    };
    format!(
        "protection_scope={}:dumped_user_scope={}:{}",
        if member_protection_visibility {
            "member"
        } else {
            "public"
        },
        dumped_user_scope,
        build_discs_url(options)
    )
}

fn build_discs_url(options: DiscsUrlOptions<'_>) -> String {
    let page = (options.page > 1)
        .then(|| options.page.to_string())
        .unwrap_or_default();
    let sort = (options.sort != "title")
        .then_some(options.sort)
        .unwrap_or_default();
    let order = (options.order != "asc")
        .then_some(options.order)
        .unwrap_or_default();
    let advanced = if options.advanced { "1" } else { "" };

    let mut url = compact_query_url(
        "/discs",
        &[
            ("system", options.system),
            ("region", options.region),
            ("language", options.language),
            ("media", options.media),
            ("category", options.category),
            ("tag_mode", options.tag_mode),
            ("status", options.status),
            ("letter", options.letter),
            ("dumper", options.dumper),
            ("title", options.title),
            ("title_mode", options.title_mode),
            ("title_foreign", options.title_foreign),
            ("title_foreign_mode", options.title_foreign_mode),
            ("disc_number", options.disc_number),
            ("disc_title", options.disc_title),
            ("disc_title_mode", options.disc_title_mode),
            ("disc_title_foreign", options.disc_title_foreign),
            ("disc_title_foreign_mode", options.disc_title_foreign_mode),
            ("serial", options.serial),
            ("serial_mode", options.serial_mode),
            ("filename_suffix", options.filename_suffix),
            ("filename_suffix_mode", options.filename_suffix_mode),
            ("edition", options.edition),
            ("edition_mode", options.edition_mode),
            ("barcode", options.barcode),
            ("barcode_mode", options.barcode_mode),
            ("universal_hash", options.universal_hash),
            ("exe_date_min", options.exe_date_min),
            ("exe_date_max", options.exe_date_max),
            ("pvd_date_min", options.pvd_date_min),
            ("pvd_date_max", options.pvd_date_max),
            ("tracks_min", options.tracks_min),
            ("tracks_max", options.tracks_max),
            ("errors_min", options.errors_min),
            ("errors_max", options.errors_max),
            ("edc", options.edc),
            ("protection", options.protection),
            ("comments", options.comments),
            ("contents", options.contents),
            ("mastering_code", options.mastering_code),
            ("mastering_sid", options.mastering_sid),
            ("toolstamp", options.toolstamp),
            ("mould_sid", options.mould_sid),
            ("additional_mould", options.additional_mould),
            ("offset_min", options.offset_min),
            ("offset_max", options.offset_max),
            ("volume_label", options.volume_label),
            ("volume_label_mode", options.volume_label_mode),
            ("internal_serial", options.internal_serial),
            ("internal_serial_mode", options.internal_serial_mode),
            ("version", options.version),
            ("version_mode", options.version_mode),
            ("q", options.q),
            ("sort", sort),
            ("order", order),
            ("dumped_filter", options.dumped_filter),
            ("page", &page),
            ("advanced", advanced),
        ],
    );
    for tag in options.tags {
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str("tag=");
        url.push_str(&urlencoding::encode(tag));
    }
    url
}

struct DiscRow {
    id: i32,
    title: String,
    title_foreign: String,
    system_code: String,
    system_display: String,
    dumped_by_me: bool,
    version: String,
    edition_display: String,
    status_class: String,
    status_display: String,
    region_flags: Vec<RegionFlag>,
    language_flags: Vec<LangFlag>,
    serial: String,
}

struct RegionFlag {
    code: String,
    name: String,
}

struct LangFlag {
    code: String,
    name: String,
}

#[derive(sqlx::FromRow)]
struct DiscFlagRow {
    disc_id: i32,
    flag_kind: i32,
    code: String,
    name: String,
}

#[derive(Default)]
struct DiscFlags {
    regions: Vec<RegionFlag>,
    languages: Vec<LangFlag>,
}

async fn load_disc_flags(
    pool: &sqlx::PgPool,
    disc_ids: &[i32],
) -> AppResult<HashMap<i32, DiscFlags>> {
    if disc_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows: Vec<DiscFlagRow> = sqlx::query_as(
        "SELECT dr.disc_id, 0::INT AS flag_kind,
                TRIM(r.flag_code)::TEXT AS code, r.name, r.sort_order
         FROM disc_regions dr
         JOIN regions r ON r.code = dr.region_code
         WHERE dr.disc_id = ANY($1)
         UNION ALL
         SELECT dl.disc_id, 1::INT AS flag_kind,
                TRIM(l.flag_code)::TEXT AS code, l.name, l.sort_order
         FROM disc_languages dl
         JOIN languages l ON l.code = dl.language_code
         WHERE dl.disc_id = ANY($1)
           AND NOT EXISTS (
               SELECT 1 FROM disc_language_variants dlv
               WHERE dlv.disc_id = dl.disc_id
                 AND dlv.language_code = dl.language_code
           )
         UNION ALL
         SELECT dlv.disc_id, 1::INT AS flag_kind,
                TRIM(lv.flag_code)::TEXT AS code, lv.name, l.sort_order
         FROM disc_language_variants dlv
         JOIN language_variants lv
           ON lv.language_code = dlv.language_code AND lv.code = dlv.variant_code
         JOIN languages l ON l.code = lv.language_code
         WHERE dlv.disc_id = ANY($1)
         ORDER BY disc_id, flag_kind, sort_order, code",
    )
    .bind(disc_ids)
    .fetch_all(pool)
    .await?;

    Ok(group_disc_flags(rows))
}

fn group_disc_flags(rows: Vec<DiscFlagRow>) -> HashMap<i32, DiscFlags> {
    let mut grouped = HashMap::new();
    for row in rows {
        let flags = grouped
            .entry(row.disc_id)
            .or_insert_with(DiscFlags::default);
        if row.flag_kind == 0 {
            flags.regions.push(RegionFlag {
                code: row.code.to_lowercase(),
                name: row.name,
            });
        } else {
            flags.languages.push(LangFlag {
                code: row.code.to_lowercase(),
                name: row.name,
            });
        }
    }
    grouped
}

fn append_csv_row(output: &mut String, fields: &[&str]) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push('"');
        if field
            .trim_start()
            .chars()
            .next()
            .is_some_and(|character| matches!(character, '=' | '+' | '-' | '@'))
        {
            output.push('\'');
        }
        for character in field.chars() {
            if character == '"' {
                output.push_str("\"\"");
            } else {
                output.push(character);
            }
        }
        output.push('"');
    }
    output.push_str("\r\n");
}

fn build_disc_csv(rows: &[RawDiscRow], flags_by_disc: &HashMap<i32, DiscFlags>) -> String {
    let mut output = String::with_capacity(rows.len() * 128);
    append_csv_row(
        &mut output,
        &[
            "Dumped by me",
            "Region",
            "Title",
            "Foreign Title",
            "System",
            "Version",
            "Edition",
            "Language",
            "Serial",
            "Status",
        ],
    );

    for row in rows {
        let flags = flags_by_disc.get(&row.id);
        let regions = flags
            .map(|flags| {
                flags
                    .regions
                    .iter()
                    .map(|flag| flag.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let languages = flags
            .map(|flags| {
                flags
                    .languages
                    .iter()
                    .map(|flag| flag.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let title = format_display_title(
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
        );
        let foreign_title = if row.has_title_foreign {
            row.title_foreign.as_deref().unwrap_or_default()
        } else {
            ""
        };
        let system =
            crate::db::models::short_system_display(&row.system_short_name, &row.system_code);
        let version = row.version.as_deref().unwrap_or_default();
        let edition = if row.has_edition {
            row.edition.as_deref().unwrap_or_default()
        } else {
            ""
        };
        let serial = if row.has_serial {
            row.serial.as_deref().unwrap_or_default()
        } else {
            ""
        };
        let dumped_by_me = if row.dumped_by_me { "Yes" } else { "No" };
        let status = row.status.to_string();

        append_csv_row(
            &mut output,
            &[
                dumped_by_me,
                &regions,
                &title,
                foreign_title,
                &system,
                version,
                edition,
                &languages,
                serial,
                &status,
            ],
        );
    }

    output
}

struct SystemOption {
    code: String,
    name: String,
    selected: bool,
}

struct RegionOption {
    code: String,
    name: String,
    selected: bool,
}

struct MediaOption {
    code: String,
    name: String,
    selected: bool,
    hidden: bool,
}

struct LanguageOption {
    code: String,
    name: String,
    selected: bool,
}

struct CategoryOption {
    name: String,
    selected: bool,
}

struct TagFilterOption {
    name: String,
    selected: bool,
}

#[derive(Clone, sqlx::FromRow)]
struct MediaFilterRow {
    code: String,
    name: String,
}

#[derive(Clone, sqlx::FromRow)]
struct CategoryFilterRow {
    id: i32,
    name: String,
}

fn build_media_filter_options(
    rows: &[MediaFilterRow],
    allowed_media: Option<&[String]>,
    selected: &str,
) -> Vec<MediaOption> {
    let mut options = Vec::with_capacity(rows.len());
    let mut add_row = |row: &MediaFilterRow, hidden: bool| {
        if options
            .iter()
            .any(|option: &MediaOption| option.code == row.code)
        {
            return;
        }
        options.push(MediaOption {
            code: row.code.clone(),
            name: row.name.clone(),
            selected: row.code == selected,
            hidden,
        });
    };

    if let Some(allowed) = allowed_media {
        for code in allowed {
            if let Some(row) = rows.iter().find(|row| row.code == *code) {
                add_row(row, false);
            }
        }
        if !selected.is_empty() && !allowed.iter().any(|code| code == selected) {
            if let Some(row) = rows.iter().find(|row| row.code == selected) {
                add_row(row, false);
            }
        }
        for row in rows {
            add_row(row, true);
        }
    } else {
        for row in rows {
            add_row(row, false);
        }
    }

    options
}

fn resolve_category_filter(requested: &str, rows: &[CategoryFilterRow]) -> (String, Option<i32>) {
    if requested.is_empty() {
        return (String::new(), None);
    }

    rows.iter()
        .find(|row| row.name.eq_ignore_ascii_case(requested))
        .map(|row| (row.name.clone(), Some(row.id)))
        .unwrap_or_else(|| (requested.to_string(), None))
}

fn add_media_category_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    media: &str,
    category_id: Option<i32>,
    category_unknown: bool,
) {
    if !media.is_empty() {
        *bind_idx += 1;
        where_clauses.push(format!("d.media_type_code = ${}", *bind_idx));
    }
    if category_unknown {
        where_clauses.push("FALSE".to_string());
    } else if category_id.is_some() {
        *bind_idx += 1;
        where_clauses.push(format!("d.category_id = ${}", *bind_idx));
    }
}

fn add_language_clause(where_clauses: &mut Vec<String>, bind_idx: &mut u32, language: &str) {
    if language.is_empty() {
        return;
    }
    if language == "none" {
        where_clauses.push("NOT EXISTS (SELECT 1 FROM disc_languages dl_filter WHERE dl_filter.disc_id = d.id)".to_string());
        return;
    }

    let bytes = language.as_bytes();
    let is_base_language = bytes.len() == 2 && bytes.iter().all(|byte| byte.is_ascii_alphabetic());
    let is_language_variant = (4..=7).contains(&bytes.len())
        && bytes[..2].iter().all(|byte| byte.is_ascii_alphabetic())
        && bytes[2] == b'-'
        && bytes[3..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric());

    if !is_base_language && !is_language_variant {
        *bind_idx += 1;
        where_clauses.push(format!("FALSE AND ${}::text IS NULL", *bind_idx));
        return;
    }

    *bind_idx += 1;
    if is_language_variant {
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM disc_language_variants dlv_filter WHERE dlv_filter.disc_id = d.id AND lower(btrim(dlv_filter.language_code::text) || '-' || dlv_filter.variant_code) = ${})",
            *bind_idx
        ));
    } else {
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM disc_languages dl_filter WHERE dl_filter.disc_id = d.id AND dl_filter.language_code = ${})",
            *bind_idx
        ));
    }
}

const TRACK_COUNT_SQL: &str = "track_filter.track_count";
const TRACK_COUNT_JOIN_SQL: &str = " CROSS JOIN LATERAL (SELECT COUNT(*) AS track_count FROM files track_files WHERE track_files.disc_id = d.id AND track_files.track_number IS NOT NULL OFFSET 0) track_filter";

fn add_track_count_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    tracks_min: Option<i32>,
    tracks_max: Option<i32>,
) {
    match (tracks_min, tracks_max) {
        (Some(_), Some(_)) => {
            *bind_idx += 1;
            let min_bind = *bind_idx;
            *bind_idx += 1;
            where_clauses.push(format!(
                "{TRACK_COUNT_SQL} BETWEEN ${min_bind} AND ${}",
                *bind_idx
            ));
        }
        (Some(_), None) => {
            *bind_idx += 1;
            where_clauses.push(format!("{TRACK_COUNT_SQL} >= ${}", *bind_idx));
        }
        (None, Some(_)) => {
            *bind_idx += 1;
            where_clauses.push(format!("{TRACK_COUNT_SQL} <= ${}", *bind_idx));
        }
        (None, None) => {}
    }
}

const ERROR_COUNT_APPLICABLE_SQL: &str = "s.has_error_count AND d.media_type_code IN \
    (SELECT code FROM media_types WHERE LOWER(rom_extension) = 'bin')";

fn add_error_count_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    errors_min: Option<i32>,
    errors_max: Option<i32>,
) {
    if errors_min.is_some() {
        *bind_idx += 1;
        where_clauses.push(format!(
            "({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count >= ${})",
            *bind_idx
        ));
    }
    if errors_max.is_some() {
        *bind_idx += 1;
        where_clauses.push(format!(
            "({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count <= ${})",
            *bind_idx
        ));
    }
}

fn add_edc_clause(where_clauses: &mut Vec<String>, edc: &str) {
    match edc {
        "yes" => where_clauses.push("(s.has_edc AND d.edc)".to_string()),
        "no" => where_clauses.push("(s.has_edc AND NOT d.edc)".to_string()),
        _ => {}
    }
}

const PAGE_SIZE: i64 = 100;
const CSV_EXPORT_LIMIT: i64 = 5_000;

async fn discs_page(
    State(state): State<AppState>,
    user: CurrentUser,
    MultiQuery(query): MultiQuery<DiscsQuery>,
) -> AppResult<Response> {
    discs_page_response(state, user, query, false).await
}

async fn discs_export(
    State(state): State<AppState>,
    user: CurrentUser,
    MultiQuery(query): MultiQuery<DiscsQuery>,
) -> AppResult<Response> {
    if !user.is_logged_in() {
        return Err(AppError::Unauthorized);
    }
    discs_page_response(state, user, query, true).await
}

fn csv_export_limit_message() -> String {
    format!(
        "CSV exports are limited to {CSV_EXPORT_LIMIT} discs. \
         Narrow your filters and try again."
    )
}

fn csv_export_limit_response() -> Response {
    (StatusCode::PAYLOAD_TOO_LARGE, csv_export_limit_message()).into_response()
}

async fn discs_page_response(
    state: AppState,
    user: CurrentUser,
    query: DiscsQuery,
    export_csv: bool,
) -> AppResult<Response> {
    let page = if export_csv {
        1
    } else {
        query.page.unwrap_or(1).max(1)
    };
    let offset = (page - 1) * PAGE_SIZE;
    let can_view_disabled_discs = user.can_view_disabled_discs();

    let filter_system = normalize_system_filter(query.system.as_deref());
    let filter_region = normalize_region_filter(query.region.as_deref());
    let filter_language = normalize_language_filter(query.language.as_deref());
    let filter_media = normalize_media_filter(query.media.as_deref());
    let requested_category = query
        .category
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    let requested_tag_mode = normalize_tag_mode(query.tag_mode.as_deref());
    let requested_tags = query
        .tag
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    let filter_status = normalize_status_filter(query.status.as_deref(), can_view_disabled_discs);
    let filter_letter = normalize_letter_filter(query.letter.as_deref());
    let filter_q = query.q.clone().unwrap_or_default().trim().to_string();
    let quick_search_terms = quick_search_terms(&filter_q);
    let hash_candidates = load_hash_candidates(&state.pool, &quick_search_terms).await?;
    let requested_title = active_verbatim_filter(query.title.as_ref());
    let title_mode = match TextMatchMode::from_query(query.title_mode.as_deref()) {
        TextMatchMode::IsNull | TextMatchMode::Any => TextMatchMode::Contains,
        TextMatchMode::Exact if requested_title.is_none() => TextMatchMode::Contains,
        mode => mode,
    };
    let active_title = requested_title.filter(|_| title_mode.searches_text());
    let title_exact = title_mode.is_exact();
    let title_bind = active_title
        .as_deref()
        .map(|value| scalar_text_bind_value(value, title_exact));
    let filter_title = title_bind.clone().unwrap_or_default();
    let title_foreign_mode = TextMatchMode::from_query(query.title_foreign_mode.as_deref());
    let active_title_foreign = active_verbatim_filter(query.title_foreign.as_ref())
        .filter(|_| title_foreign_mode.searches_text());
    let title_foreign_exact = title_foreign_mode.is_exact();
    let title_foreign_bind = active_title_foreign
        .as_deref()
        .map(|value| scalar_text_bind_value(value, title_foreign_exact));
    let filter_title_foreign = title_foreign_bind.clone().unwrap_or_default();
    let active_disc_number = active_verbatim_filter(query.disc_number.as_ref());
    let disc_number_bind = active_disc_number
        .as_deref()
        .map(|value| scalar_text_bind_value(value, true));
    let filter_disc_number = disc_number_bind.clone().unwrap_or_default();
    let disc_title_mode = TextMatchMode::from_query(query.disc_title_mode.as_deref());
    let active_disc_title = active_verbatim_filter(query.disc_title.as_ref())
        .filter(|_| disc_title_mode.searches_text());
    let disc_title_exact = disc_title_mode.is_exact();
    let disc_title_bind = active_disc_title
        .as_deref()
        .map(|value| scalar_text_bind_value(value, disc_title_exact));
    let filter_disc_title = disc_title_bind.clone().unwrap_or_default();
    let disc_title_foreign_mode =
        TextMatchMode::from_query(query.disc_title_foreign_mode.as_deref());
    let active_disc_title_foreign = active_verbatim_filter(query.disc_title_foreign.as_ref())
        .filter(|_| disc_title_foreign_mode.searches_text());
    let disc_title_foreign_exact = disc_title_foreign_mode.is_exact();
    let disc_title_foreign_bind = active_disc_title_foreign
        .as_deref()
        .map(|value| scalar_text_bind_value(value, disc_title_foreign_exact));
    let filter_disc_title_foreign = disc_title_foreign_bind.clone().unwrap_or_default();
    let serial_mode = TextMatchMode::from_query(query.serial_mode.as_deref());
    let active_serial =
        active_verbatim_filter(query.serial.as_ref()).filter(|_| serial_mode.searches_text());
    let filter_serial = active_serial.clone().unwrap_or_default();
    let serial_exact = serial_mode.is_exact();
    let serial_bind = active_serial
        .as_deref()
        .map(|value| array_text_bind_value(value, serial_exact));
    let filename_suffix_mode = TextMatchMode::from_query(query.filename_suffix_mode.as_deref());
    let active_filename_suffix = active_advanced_filter(query.filename_suffix.as_ref())
        .filter(|_| filename_suffix_mode.searches_text());
    let filename_suffix_exact = filename_suffix_mode.is_exact();
    let filename_suffix_bind = active_filename_suffix
        .as_deref()
        .map(|value| scalar_text_bind_value(value, filename_suffix_exact));
    let filter_filename_suffix = filename_suffix_bind.clone().unwrap_or_default();
    let edition_mode = TextMatchMode::from_query(query.edition_mode.as_deref());
    let active_edition =
        active_verbatim_filter(query.edition.as_ref()).filter(|_| edition_mode.searches_text());
    let filter_edition = active_edition.clone().unwrap_or_default();
    let edition_exact = edition_mode.is_exact();
    let edition_bind = active_edition
        .as_deref()
        .map(|value| array_text_bind_value(value, edition_exact));
    let barcode_mode = TextMatchMode::from_query(query.barcode_mode.as_deref());
    let active_barcode =
        active_verbatim_filter(query.barcode.as_ref()).filter(|_| barcode_mode.searches_text());
    let filter_barcode = active_barcode.clone().unwrap_or_default();
    let barcode_exact = barcode_mode.is_exact();
    let barcode_bind = active_barcode
        .as_deref()
        .map(|value| array_text_bind_value(value, barcode_exact));
    let active_universal_hash = active_advanced_filter(query.universal_hash.as_ref());
    let filter_universal_hash = active_universal_hash.clone().unwrap_or_default();
    let universal_hash_bind =
        queue_service::universal_hash_bytes_for_matching(active_universal_hash.as_deref());
    let active_exe_date_min = normalize_date_filter(query.exe_date_min.as_deref());
    let active_exe_date_max = normalize_date_filter(query.exe_date_max.as_deref());
    let exe_date_range_inverted = matches!(
        (&active_exe_date_min, &active_exe_date_max),
        (Some(min), Some(max)) if {
            let shared = min.len().min(max.len());
            min[..shared] > max[..shared]
        }
    );
    let filter_exe_date_min = active_exe_date_min.clone().unwrap_or_default();
    let filter_exe_date_max = active_exe_date_max.clone().unwrap_or_default();
    let active_pvd_date_min = normalize_date_filter(query.pvd_date_min.as_deref());
    let active_pvd_date_max = normalize_date_filter(query.pvd_date_max.as_deref());
    let pvd_date_range_inverted = matches!(
        (&active_pvd_date_min, &active_pvd_date_max),
        (Some(min), Some(max)) if {
            let shared = min.len().min(max.len());
            min[..shared] > max[..shared]
        }
    );
    let filter_pvd_date_min = active_pvd_date_min.clone().unwrap_or_default();
    let filter_pvd_date_max = active_pvd_date_max.clone().unwrap_or_default();
    let filter_tracks_min_value = normalize_non_negative_bound(query.tracks_min.as_deref());
    let filter_tracks_max_value = normalize_non_negative_bound(query.tracks_max.as_deref());
    let filter_tracks_min = filter_tracks_min_value
        .map(|value| value.to_string())
        .unwrap_or_default();
    let filter_tracks_max = filter_tracks_max_value
        .map(|value| value.to_string())
        .unwrap_or_default();
    let tracks_exact =
        filter_tracks_min_value.is_some() && filter_tracks_min_value == filter_tracks_max_value;
    let filter_errors_min_value = normalize_non_negative_bound(query.errors_min.as_deref());
    let filter_errors_max_value = normalize_non_negative_bound(query.errors_max.as_deref());
    let filter_errors_min = filter_errors_min_value
        .map(|value| value.to_string())
        .unwrap_or_default();
    let filter_errors_max = filter_errors_max_value
        .map(|value| value.to_string())
        .unwrap_or_default();
    let errors_exact =
        filter_errors_min_value.is_some() && filter_errors_min_value == filter_errors_max_value;
    let filter_edc = normalize_edc_filter(query.edc.as_deref());
    let filter_protection = query
        .protection
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    let filter_comments = query
        .comments
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    let filter_contents = query
        .contents
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    let active_mastering_code = active_advanced_filter(query.mastering_code.as_ref());
    let filter_mastering_code = active_mastering_code.clone().unwrap_or_default();
    let mastering_code_bind = active_mastering_code
        .as_deref()
        .map(disc_service::normalize_ringcode_whitespace);
    let active_mastering_sid = active_advanced_filter(query.mastering_sid.as_ref());
    let filter_mastering_sid = active_mastering_sid.clone().unwrap_or_default();
    let mastering_sid_bind = active_mastering_sid
        .as_deref()
        .map(disc_service::normalize_ringcode_whitespace);
    let active_toolstamp = active_advanced_filter(query.toolstamp.as_ref());
    let filter_toolstamp = active_toolstamp.clone().unwrap_or_default();
    let toolstamp_bind = active_toolstamp
        .as_deref()
        .map(disc_service::normalize_ringcode_whitespace);
    let active_mould_sid = active_advanced_filter(query.mould_sid.as_ref());
    let filter_mould_sid = active_mould_sid.clone().unwrap_or_default();
    let mould_sid_bind = active_mould_sid
        .as_deref()
        .map(disc_service::normalize_ringcode_whitespace);
    let active_additional_mould = active_advanced_filter(query.additional_mould.as_ref());
    let filter_additional_mould = active_additional_mould.clone().unwrap_or_default();
    let additional_mould_bind = active_additional_mould
        .as_deref()
        .map(disc_service::normalize_ringcode_whitespace);
    let filter_offset_min_value = normalize_offset_filter(query.offset_min.as_deref());
    let filter_offset_max_value = normalize_offset_filter(query.offset_max.as_deref());
    let filter_offset_min = filter_offset_min_value
        .map(|value| value.to_string())
        .unwrap_or_default();
    let filter_offset_max = filter_offset_max_value
        .map(|value| value.to_string())
        .unwrap_or_default();
    let offset_exact = filter_offset_min_value.is_some() && filter_offset_min_value == filter_offset_max_value;
    let volume_label_mode = TextMatchMode::from_query(query.volume_label_mode.as_deref());
    let active_volume_label = active_advanced_filter(query.volume_label.as_ref())
        .filter(|_| volume_label_mode.searches_text());
    let volume_label_exact = volume_label_mode.is_exact();
    let volume_label_bind = active_volume_label
        .as_deref()
        .map(|value| scalar_text_bind_value(value, volume_label_exact));
    let filter_volume_label = volume_label_bind.clone().unwrap_or_default();
    let internal_serial_mode = TextMatchMode::from_query(query.internal_serial_mode.as_deref());
    let active_internal_serial = active_advanced_filter(query.internal_serial.as_ref())
        .filter(|_| internal_serial_mode.searches_text());
    let internal_serial_exact = internal_serial_mode.is_exact();
    let internal_serial_bind = active_internal_serial
        .as_deref()
        .map(|value| scalar_text_bind_value(value, internal_serial_exact));
    let filter_internal_serial = internal_serial_bind.clone().unwrap_or_default();
    let version_mode = TextMatchMode::from_query(query.version_mode.as_deref());
    let active_version =
        active_advanced_filter(query.version.as_ref()).filter(|_| version_mode.searches_text());
    let version_exact = version_mode.is_exact();
    let version_bind = active_version
        .as_deref()
        .map(|value| scalar_text_bind_value(value, version_exact));
    let filter_version = version_bind.clone().unwrap_or_default();
    let active_protection = active_advanced_filter(query.protection.as_ref());
    let active_comments = active_advanced_filter(query.comments.as_ref());
    let active_contents = active_advanced_filter(query.contents.as_ref());
    let requested_dumper = query.dumper.clone().unwrap_or_default().trim().to_string();
    let filter_dumper_lookup = if requested_dumper.is_empty() {
        None
    } else {
        sqlx::query_as::<_, DumperRow>(
            "SELECT id, username AS name FROM users
             WHERE LOWER(username) = LOWER($1) ORDER BY id LIMIT 1",
        )
        .bind(&requested_dumper)
        .fetch_optional(&state.pool)
        .await?
    };
    let filter_dumper_id = filter_dumper_lookup.as_ref().map(|d| d.id);
    let filter_dumper = filter_dumper_lookup
        .as_ref()
        .map(|d| d.name.clone())
        .unwrap_or(requested_dumper);
    let filter_dumper_name = if !filter_dumper.is_empty() {
        filter_dumper.clone()
    } else {
        String::new()
    };
    let filter_dumper_unknown = !filter_dumper.is_empty() && filter_dumper_id.is_none();
    let advanced_explicit = advanced_panel_explicitly_open(query.advanced.as_deref());
    let advanced_open = advanced_explicit;

    let reference_data = state.discs_cache.references.get(&state.pool).await?;
    let filter_tags = reference_data
        .tags
        .iter()
        .filter(|tag| {
            requested_tags
                .iter()
                .any(|requested| tag.eq_ignore_ascii_case(requested))
        })
        .cloned()
        .collect::<Vec<_>>();
    let filter_tag_mode = if filter_tags.is_empty() {
        String::new()
    } else {
        requested_tag_mode
    };
    let tag_options = reference_data
        .tags
        .iter()
        .map(|tag| TagFilterOption {
            name: tag.clone(),
            selected: filter_tags.iter().any(|selected| selected == tag),
        })
        .collect::<Vec<_>>();
    let sys_rows = reference_data.systems.clone();

    let systems_media_json = serde_json::to_string(
        &sys_rows
            .iter()
            .map(|system| (system.code.clone(), system.media_types.clone()))
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string());
    let selected_system_media = sys_rows
        .iter()
        .find(|system| system.code == filter_system)
        .map(|system| system.media_types.clone());

    let systems: Vec<SystemOption> = sys_rows
        .into_iter()
        .map(|s| SystemOption {
            selected: s.code == filter_system,
            name: crate::db::models::build_system_name(&s.manufacturer, &s.name),
            code: s.code,
        })
        .collect();

    let media_rows = &reference_data.media;
    let media_types =
        build_media_filter_options(media_rows, selected_system_media.as_deref(), &filter_media);

    let category_rows = &reference_data.categories;
    let (filter_category, filter_category_id) =
        resolve_category_filter(&requested_category, category_rows);
    let filter_category_unknown = !filter_category.is_empty() && filter_category_id.is_none();
    let categories = category_rows
        .iter()
        .map(|category| CategoryOption {
            name: category.name.clone(),
            selected: filter_category_id == Some(category.id),
        })
        .collect();

    let region_rows = reference_data.regions.clone();

    let regions: Vec<RegionOption> = region_rows
        .into_iter()
        .map(|r| RegionOption {
            selected: r.code.trim() == filter_region,
            code: r.code.trim().to_string(),
            name: r.name,
        })
        .collect();

    let language_rows = reference_data.languages.clone();

    let languages = language_rows
        .into_iter()
        .map(|language| LanguageOption {
            selected: language.code == filter_language,
            code: language.code,
            name: language.name,
        })
        .collect();

    let mut where_clauses = vec!["1=1".to_string()];
    let mut bind_idx = 0u32;

    if !filter_system.is_empty() {
        bind_idx += 1;
        where_clauses.push(format!("d.system_code = ${bind_idx}"));
    }
    add_media_category_clauses(
        &mut where_clauses,
        &mut bind_idx,
        &filter_media,
        filter_category_id,
        filter_category_unknown,
    );
    if !filter_tags.is_empty() {
        bind_idx += 1;
        let matching_tags = format!(
            "SELECT 1 FROM disc_tags dt_filter JOIN tags t_filter ON t_filter.id = dt_filter.tag_id WHERE dt_filter.disc_id = d.id AND t_filter.name = ANY(${bind_idx})"
        );
        let tag_clause = match filter_tag_mode.as_str() {
            "and" => format!(
                "EXISTS ({matching_tags} GROUP BY dt_filter.disc_id HAVING COUNT(DISTINCT t_filter.id) = cardinality(${bind_idx}))"
            ),
            "not" => format!("NOT EXISTS ({matching_tags})"),
            _ => format!("EXISTS ({matching_tags})"),
        };
        where_clauses.push(tag_clause);
    }
    if !filter_region.is_empty() {
        bind_idx += 1;
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM disc_regions dr WHERE dr.disc_id = d.id AND dr.region_code = ${bind_idx})"
        ));
    }
    add_language_clause(&mut where_clauses, &mut bind_idx, &filter_language);
    if filter_letter == "#" {
        where_clauses.push("d.title ~* '^[^a-zA-Z]'".to_string());
    } else if filter_letter.len() == 1
        && filter_letter.chars().next().unwrap().is_ascii_alphabetic()
    {
        bind_idx += 1;
        where_clauses.push(format!("upper(left(d.title, 1)) = upper(${bind_idx})"));
    }
    if filter_status == "Disabled" {
        where_clauses.push("d.status = 'Disabled'".to_string());
    } else if filter_status == "All Statuses" {
        // no filter — show both enabled and disabled
    } else if filter_status == "Questionable" {
        where_clauses.push("d.status = 'Questionable'".to_string());
    } else if filter_status == "Verified" {
        where_clauses.push("d.status = 'Verified'".to_string());
    } else if filter_status == "Unverified" {
        where_clauses.push("d.status = 'Unverified'".to_string());
    } else if filter_status == "Trusted" {
        where_clauses.push("d.status IN ('Verified', 'Unverified')".to_string());
    } else {
        where_clauses.push("d.status != 'Disabled'".to_string());
    }
    for term in &quick_search_terms {
        bind_idx += 1;
        let hash_candidate_bind_idx = if hash_field_for_term(term).is_some() {
            bind_idx += 1;
            Some(bind_idx)
        } else {
            None
        };
        where_clauses.push(quick_search_clause(
            bind_idx - u32::from(hash_candidate_bind_idx.is_some()),
            hash_candidate_bind_idx,
        ));
    }
    add_scalar_text_clauses(
        &mut where_clauses,
        &mut bind_idx,
        [
            (
                ScalarTextField::Title,
                active_title.is_some(),
                title_mode,
            ),
            (
                ScalarTextField::ForeignTitle,
                active_title_foreign.is_some(),
                title_foreign_mode,
            ),
            (
                ScalarTextField::DiscNumber,
                active_disc_number.is_some(),
                TextMatchMode::Exact,
            ),
            (
                ScalarTextField::DiscTitle,
                active_disc_title.is_some(),
                disc_title_mode,
            ),
            (
                ScalarTextField::ForeignDiscTitle,
                active_disc_title_foreign.is_some(),
                disc_title_foreign_mode,
            ),
            (
                ScalarTextField::VolumeLabel,
                active_volume_label.is_some(),
                volume_label_mode,
            ),
            (
                ScalarTextField::InternalSerial,
                active_internal_serial.is_some(),
                internal_serial_mode,
            ),
            (
                ScalarTextField::FilenameSuffix,
                active_filename_suffix.is_some(),
                filename_suffix_mode,
            ),
            (
                ScalarTextField::Version,
                active_version.is_some(),
                version_mode,
            ),
        ],
    );
    add_array_text_clauses(
        &mut where_clauses,
        &mut bind_idx,
        [
            (
                ArrayTextField::Serial,
                active_serial.is_some(),
                serial_mode,
            ),
            (
                ArrayTextField::Edition,
                active_edition.is_some(),
                edition_mode,
            ),
            (
                ArrayTextField::Barcode,
                active_barcode.is_some(),
                barcode_mode,
            ),
        ],
    );
    add_universal_hash_clause(
        &mut where_clauses,
        &mut bind_idx,
        active_universal_hash.is_some(),
        universal_hash_bind.as_deref(),
    );
    if active_exe_date_min.is_some() {
        bind_idx += 1;
        where_clauses.push(format!("d.exe_date >= LEFT(${bind_idx}, LENGTH(d.exe_date))"));
    }
    if active_exe_date_max.is_some() {
        bind_idx += 1;
        where_clauses.push(format!("LEFT(d.exe_date, LENGTH(${bind_idx})) <= ${bind_idx}"));
    }
    add_pvd_date_clauses(
        &mut where_clauses,
        &mut bind_idx,
        active_pvd_date_min.as_deref(),
        active_pvd_date_max.as_deref(),
    );
    if exe_date_range_inverted || pvd_date_range_inverted {
        where_clauses.push("FALSE".to_string());
    }
    add_track_count_clauses(
        &mut where_clauses,
        &mut bind_idx,
        filter_tracks_min_value,
        filter_tracks_max_value,
    );
    add_error_count_clauses(
        &mut where_clauses,
        &mut bind_idx,
        filter_errors_min_value,
        filter_errors_max_value,
    );
    add_edc_clause(&mut where_clauses, &filter_edc);
    if active_protection.is_some() {
        bind_idx += 1;
        where_clauses.push(protection_search_clause(bind_idx, user.is_logged_in()));
    }
    if active_comments.is_some() {
        bind_idx += 1;
        where_clauses.push(comments_search_clause(bind_idx));
    }
    if active_contents.is_some() {
        bind_idx += 1;
        where_clauses.push(contents_search_clause(bind_idx));
    }
    add_ring_filter_clauses(
        &mut where_clauses,
        &mut bind_idx,
        [
            (
                RingTextField::MasteringCode,
                active_mastering_code.is_some(),
            ),
            (RingTextField::MasteringSid, active_mastering_sid.is_some()),
            (RingTextField::Toolstamp, active_toolstamp.is_some()),
            (RingTextField::MouldSid, active_mould_sid.is_some()),
            (
                RingTextField::AdditionalMould,
                active_additional_mould.is_some(),
            ),
        ],
        filter_offset_min_value.is_some(),
        filter_offset_max_value.is_some(),
    );
    if filter_dumper_unknown {
        where_clauses.push("FALSE".to_string());
    } else if filter_dumper_id.is_some() {
        bind_idx += 1;
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM disc_dumpers dd2 WHERE dd2.disc_id = d.id AND dd2.user_id = ${bind_idx})"
        ));
    }

    let current_user_id = user.user().map(|u| u.id);
    let dumped_by_me_sql = if current_user_id.is_some() {
        bind_idx += 1;
        format!(
            "EXISTS (SELECT 1 FROM disc_dumpers dd_self WHERE dd_self.disc_id = d.id AND dd_self.user_id = ${bind_idx})"
        )
    } else {
        "FALSE".to_string()
    };
    let dumped_filter = if user.is_logged_in() {
        normalize_dumped_filter(query.dumped_filter.as_deref())
    } else {
        String::new()
    };
    match dumped_filter.as_str() {
        "dumped" => where_clauses.push(dumped_by_me_sql.clone()),
        "not_dumped" => where_clauses.push(format!("NOT ({dumped_by_me_sql})")),
        _ => {}
    }
    let where_sql = where_clauses.join(" AND ");

    let sort_column = normalize_disc_sort(query.sort.as_deref());
    let sort_order_str = normalize_sort_order(query.order.as_deref());

    let mut sort_cte = String::new();
    let mut sort_join = "";
    let sort_col = match sort_column.as_str() {
        "region" => {
            sort_cte = "WITH region_sort AS MATERIALIZED (
                SELECT dr.disc_id, ARRAY_AGG(r.sort_order ORDER BY r.sort_order) AS sort_value
                FROM disc_regions dr
                JOIN regions r ON r.code = dr.region_code
                GROUP BY dr.disc_id
            )"
            .to_string();
            sort_join = " LEFT JOIN region_sort ON region_sort.disc_id = d.id";
            "region_sort.sort_value"
        }
        "title" => display_title_sort_sql(),
        "system" => "LOWER(CONCAT_WS(' ', NULLIF(s.manufacturer, ''), s.name))",
        "version" => "LOWER(d.version)",
        "edition" => "LOWER(arr_to_str(d.edition, ', '))",
        "language" => {
            sort_cte = "WITH language_sort AS MATERIALIZED (
                SELECT dl.disc_id, ARRAY_AGG(l.sort_order ORDER BY l.sort_order) AS sort_value
                FROM disc_languages dl
                JOIN languages l ON l.code = dl.language_code
                GROUP BY dl.disc_id
            )"
            .to_string();
            sort_join = " LEFT JOIN language_sort ON language_sort.disc_id = d.id";
            "language_sort.sort_value"
        }
        "serial" => "LOWER(arr_to_str(d.serial, ', '))",
        "status" => {
            "CASE d.status WHEN 'Verified' THEN 1 WHEN 'Unverified' THEN 2 WHEN 'Questionable' THEN 3 ELSE 4 END"
        }
        "added" | "modified" => {
            let date_sort = disc_date_sort_sql(&sort_column).unwrap();
            sort_cte = date_sort.cte;
            sort_join = date_sort.join;
            date_sort.expression
        }
        _ => display_title_sort_sql(),
    };
    let sort_dir = if sort_order_str == "desc" {
        "DESC"
    } else {
        "ASC"
    };
    let order_by = disc_order_by_sql(&sort_column, sort_col, sort_dir);

    let count_key_title = title_bind
        .as_deref()
        .map(|value| {
            if title_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_title_foreign = title_foreign_bind
        .as_deref()
        .map(|value| {
            if title_foreign_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_disc_number = disc_number_bind.clone().unwrap_or_default();
    let count_key_disc_title = disc_title_bind
        .as_deref()
        .map(|value| {
            if disc_title_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_disc_title_foreign = disc_title_foreign_bind
        .as_deref()
        .map(|value| {
            if disc_title_foreign_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_serial = serial_bind.clone().unwrap_or_default();
    let count_key_filename_suffix = filename_suffix_bind
        .as_deref()
        .map(|value| {
            if filename_suffix_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_edition = edition_bind.clone().unwrap_or_default();
    let count_key_barcode = barcode_bind.clone().unwrap_or_default();
    let count_key_universal_hash = active_universal_hash
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_volume_label = volume_label_bind
        .as_deref()
        .map(|value| {
            if volume_label_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_internal_serial = internal_serial_bind
        .as_deref()
        .map(|value| {
            if internal_serial_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_version = version_bind
        .as_deref()
        .map(|value| {
            if version_exact {
                value.to_string()
            } else {
                value.to_lowercase()
            }
        })
        .unwrap_or_default();
    let count_key_protection = active_protection
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_comments = active_comments
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_contents = active_contents
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_mastering_code = mastering_code_bind
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_mastering_sid = mastering_sid_bind
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_toolstamp = toolstamp_bind
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_mould_sid = mould_sid_bind
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_additional_mould = additional_mould_bind
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let count_key_q = quick_search_terms.join("\u{1f}");
    let count_cache_key = build_discs_count_cache_key(
        DiscsUrlOptions {
            system: &filter_system,
            region: &filter_region,
            language: &filter_language,
            media: &filter_media,
            category: &filter_category,
            tags: &filter_tags,
            tag_mode: &filter_tag_mode,
            status: &filter_status,
            letter: &filter_letter,
            dumper: &filter_dumper,
            title: &count_key_title,
            title_mode: title_mode.as_query_value(),
            title_foreign: &count_key_title_foreign,
            title_foreign_mode: title_foreign_mode.as_query_value(),
            disc_number: &count_key_disc_number,
            disc_title: &count_key_disc_title,
            disc_title_mode: disc_title_mode.as_query_value(),
            disc_title_foreign: &count_key_disc_title_foreign,
            disc_title_foreign_mode: disc_title_foreign_mode.as_query_value(),
            serial: &count_key_serial,
            serial_mode: serial_mode.as_query_value(),
            filename_suffix: &count_key_filename_suffix,
            filename_suffix_mode: filename_suffix_mode.as_query_value(),
            edition: &count_key_edition,
            edition_mode: edition_mode.as_query_value(),
            barcode: &count_key_barcode,
            barcode_mode: barcode_mode.as_query_value(),
            universal_hash: &count_key_universal_hash,
            exe_date_min: &filter_exe_date_min,
            exe_date_max: &filter_exe_date_max,
            pvd_date_min: &filter_pvd_date_min,
            pvd_date_max: &filter_pvd_date_max,
            tracks_min: &filter_tracks_min,
            tracks_max: &filter_tracks_max,
            errors_min: &filter_errors_min,
            errors_max: &filter_errors_max,
            edc: &filter_edc,
            protection: &count_key_protection,
            comments: &count_key_comments,
            contents: &count_key_contents,
            mastering_code: &count_key_mastering_code,
            mastering_sid: &count_key_mastering_sid,
            toolstamp: &count_key_toolstamp,
            mould_sid: &count_key_mould_sid,
            additional_mould: &count_key_additional_mould,
            offset_min: &filter_offset_min,
            offset_max: &filter_offset_max,
            volume_label: &count_key_volume_label,
            volume_label_mode: volume_label_mode.as_query_value(),
            internal_serial: &count_key_internal_serial,
            internal_serial_mode: internal_serial_mode.as_query_value(),
            version: &count_key_version,
            version_mode: version_mode.as_query_value(),
            q: &count_key_q,
            sort: &sort_column,
            order: &sort_order_str,
            dumped_filter: &dumped_filter,
            page,
            advanced: advanced_explicit,
        },
        active_protection.is_some() && user.is_logged_in(),
        if dumped_filter.is_empty() {
            None
        } else {
            current_user_id
        },
    );
    let mut cached_total_count = if export_csv {
        None
    } else {
        state.discs_cache.counts.get(&count_cache_key).await
    };
    let count_load_lock = if !export_csv && cached_total_count.is_none() {
        Some(state.discs_cache.counts.load_lock(&count_cache_key).await)
    } else {
        None
    };
    let _count_load_guard = match &count_load_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    if !export_csv && cached_total_count.is_none() {
        cached_total_count = state.discs_cache.counts.get(&count_cache_key).await;
    }

    let count_requires_system_join = active_title_foreign.is_some()
        || title_foreign_mode == TextMatchMode::Any
        || active_disc_number.is_some()
        || active_disc_title.is_some()
        || disc_title_mode == TextMatchMode::Any
        || active_disc_title_foreign.is_some()
        || disc_title_foreign_mode == TextMatchMode::Any
        || active_serial.is_some()
        || serial_mode == TextMatchMode::Any
        || active_edition.is_some()
        || edition_mode == TextMatchMode::Any
        || active_barcode.is_some()
        || barcode_mode == TextMatchMode::Any
        || filter_errors_min_value.is_some()
        || filter_errors_max_value.is_some()
        || !filter_edc.is_empty()
        || active_protection.is_some()
        || filter_offset_min_value.is_some()
        || filter_offset_max_value.is_some();
    let count_system_join = if count_requires_system_join {
        " JOIN systems s ON s.code = d.system_code"
    } else {
        ""
    };
    let track_count_join = if filter_tracks_min_value.is_some() || filter_tracks_max_value.is_some()
    {
        TRACK_COUNT_JOIN_SQL
    } else {
        ""
    };
    let sql_count = format!(
        "SELECT COUNT(*) FROM discs d{count_system_join}{track_count_join} WHERE {where_sql}"
    );
    let select_limit = if export_csv {
        CSV_EXPORT_LIMIT + 1
    } else {
        PAGE_SIZE
    };
    let select_offset = if export_csv { 0 } else { offset };
    let sql_select = format!(
        "{sort_cte} SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix,
                d.title_foreign,
                s.has_title_foreign, s.has_disc_number, s.has_disc_title, s.has_edition, s.has_serial,
                s.code AS system_code,
                s.short_name AS system_short_name,
                array_to_string(d.serial, ', ') AS serial,
                d.version,
                array_to_string(d.edition, ', ') AS edition,
                d.status,
                {dumped_by_me_sql} AS dumped_by_me
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         {sort_join}
         {track_count_join}
         WHERE {where_sql}
         ORDER BY {order_by} LIMIT {select_limit} OFFSET {select_offset}"
    );

    let mut count_query = (!export_csv && cached_total_count.is_none())
        .then(|| sqlx::query_scalar::<_, i64>(&sql_count));
    let mut select_query = sqlx::query_as::<_, RawDiscRow>(&sql_select);

    macro_rules! bind_queries {
        ($value:expr) => {{
            let value = $value;
            if let Some(query) = count_query.take() {
                count_query = Some(query.bind(value.clone()));
            }
            select_query = select_query.bind(value);
        }};
    }

    if !filter_system.is_empty() {
        bind_queries!(filter_system.clone());
    }
    if !filter_media.is_empty() {
        bind_queries!(filter_media.clone());
    }
    if let Some(category_id) = filter_category_id {
        bind_queries!(category_id);
    }
    if !filter_tags.is_empty() {
        bind_queries!(filter_tags.clone());
    }
    if !filter_region.is_empty() {
        bind_queries!(filter_region.clone());
    }
    if !filter_language.is_empty() && filter_language != "none" {
        bind_queries!(filter_language.clone());
    }
    if filter_letter != "#"
        && filter_letter.len() == 1
        && filter_letter.chars().next().unwrap().is_ascii_alphabetic()
    {
        bind_queries!(filter_letter.clone());
    }
    for term in &quick_search_terms {
        bind_queries!(term.clone());
        if hash_field_for_term(term).is_some() {
            let candidates = hash_candidates.get(term).cloned().unwrap_or_default();
            bind_queries!(candidates);
        }
    }
    if let Some(title) = &title_bind {
        bind_queries!(title.clone());
    }
    if let Some(title_foreign) = &title_foreign_bind {
        bind_queries!(title_foreign.clone());
    }
    if let Some(disc_number) = &disc_number_bind {
        bind_queries!(disc_number.clone());
    }
    if let Some(disc_title) = &disc_title_bind {
        bind_queries!(disc_title.clone());
    }
    if let Some(disc_title_foreign) = &disc_title_foreign_bind {
        bind_queries!(disc_title_foreign.clone());
    }
    if let Some(volume_label) = &volume_label_bind {
        bind_queries!(volume_label.clone());
    }
    if let Some(internal_serial) = &internal_serial_bind {
        bind_queries!(internal_serial.clone());
    }
    if let Some(filename_suffix) = &filename_suffix_bind {
        bind_queries!(filename_suffix.clone());
    }
    if let Some(version) = &version_bind {
        bind_queries!(version.clone());
    }
    if let Some(serial) = &serial_bind {
        bind_queries!(serial.clone());
    }
    if let Some(edition) = &edition_bind {
        bind_queries!(edition.clone());
    }
    if let Some(barcode) = &barcode_bind {
        bind_queries!(barcode.clone());
    }
    if let Some(universal_hash) = &universal_hash_bind {
        bind_queries!(universal_hash.clone());
    }
    if let Some(exe_date_min) = &active_exe_date_min {
        bind_queries!(exe_date_min.clone());
    }
    if let Some(exe_date_max) = &active_exe_date_max {
        bind_queries!(exe_date_max.clone());
    }
    if let Some(pvd_date_min) = &active_pvd_date_min {
        bind_queries!(pvd_date_bound_bytes(pvd_date_min, b'0'));
    }
    if let Some(pvd_date_max) = &active_pvd_date_max {
        bind_queries!(pvd_date_bound_bytes(pvd_date_max, b'9'));
    }
    if let Some(tracks_min) = filter_tracks_min_value {
        bind_queries!(tracks_min);
    }
    if let Some(tracks_max) = filter_tracks_max_value {
        bind_queries!(tracks_max);
    }
    if let Some(errors_min) = filter_errors_min_value {
        bind_queries!(errors_min);
    }
    if let Some(errors_max) = filter_errors_max_value {
        bind_queries!(errors_max);
    }
    if let Some(protection) = &active_protection {
        bind_queries!(protection.clone());
    }
    if let Some(comments) = &active_comments {
        bind_queries!(comments.clone());
    }
    if let Some(contents) = &active_contents {
        bind_queries!(contents.clone());
    }
    if let Some(mastering_code) = &mastering_code_bind {
        bind_queries!(mastering_code.clone());
    }
    if let Some(mastering_sid) = &mastering_sid_bind {
        bind_queries!(mastering_sid.clone());
    }
    if let Some(toolstamp) = &toolstamp_bind {
        bind_queries!(toolstamp.clone());
    }
    if let Some(mould_sid) = &mould_sid_bind {
        bind_queries!(mould_sid.clone());
    }
    if let Some(additional_mould) = &additional_mould_bind {
        bind_queries!(additional_mould.clone());
    }
    if let Some(offset_min) = filter_offset_min_value {
        bind_queries!(offset_min);
    }
    if let Some(offset_max) = filter_offset_max_value {
        bind_queries!(offset_max);
    }
    if let Some(dumper_id) = filter_dumper_id {
        bind_queries!(dumper_id);
    }
    if let Some(current_user_id) = current_user_id {
        if dumped_filter.is_empty() {
            select_query = select_query.bind(current_user_id);
        } else {
            bind_queries!(current_user_id);
        }
    }

    let total_count = if export_csv {
        None
    } else if let Some(count_query) = count_query {
        let total_count = count_query.fetch_one(&state.pool).await?;
        state
            .discs_cache
            .counts
            .insert(count_cache_key, total_count)
            .await;
        Some(total_count)
    } else {
        cached_total_count
    };
    let total_pages = total_count
        .map(|count| (count + PAGE_SIZE - 1) / PAGE_SIZE)
        .unwrap_or_default();

    if !export_csv && page > total_pages.max(1) {
        return Ok(Redirect::to(&build_discs_url(DiscsUrlOptions {
            system: &filter_system,
            region: &filter_region,
            language: &filter_language,
            media: &filter_media,
            category: &filter_category,
            tags: &filter_tags,
            tag_mode: &filter_tag_mode,
            status: &filter_status,
            letter: &filter_letter,
            dumper: &filter_dumper,
            title: &filter_title,
            title_mode: title_mode.as_query_value(),
            title_foreign: &filter_title_foreign,
            title_foreign_mode: title_foreign_mode.as_query_value(),
            disc_number: &filter_disc_number,
            disc_title: &filter_disc_title,
            disc_title_mode: disc_title_mode.as_query_value(),
            disc_title_foreign: &filter_disc_title_foreign,
            disc_title_foreign_mode: disc_title_foreign_mode.as_query_value(),
            serial: &filter_serial,
            serial_mode: serial_mode.as_query_value(),
            filename_suffix: &filter_filename_suffix,
            filename_suffix_mode: filename_suffix_mode.as_query_value(),
            edition: &filter_edition,
            edition_mode: edition_mode.as_query_value(),
            barcode: &filter_barcode,
            barcode_mode: barcode_mode.as_query_value(),
            universal_hash: &filter_universal_hash,
            exe_date_min: &filter_exe_date_min,
            exe_date_max: &filter_exe_date_max,
            pvd_date_min: &filter_pvd_date_min,
            pvd_date_max: &filter_pvd_date_max,
            tracks_min: &filter_tracks_min,
            tracks_max: &filter_tracks_max,
            errors_min: &filter_errors_min,
            errors_max: &filter_errors_max,
            edc: &filter_edc,
            protection: &filter_protection,
            comments: &filter_comments,
            contents: &filter_contents,
            mastering_code: &filter_mastering_code,
            mastering_sid: &filter_mastering_sid,
            toolstamp: &filter_toolstamp,
            mould_sid: &filter_mould_sid,
            additional_mould: &filter_additional_mould,
            offset_min: &filter_offset_min,
            offset_max: &filter_offset_max,
            volume_label: &filter_volume_label,
            volume_label_mode: volume_label_mode.as_query_value(),
            internal_serial: &filter_internal_serial,
            internal_serial_mode: internal_serial_mode.as_query_value(),
            version: &filter_version,
            version_mode: version_mode.as_query_value(),
            q: &filter_q,
            sort: &sort_column,
            order: &sort_order_str,
            dumped_filter: &dumped_filter,
            page: total_pages.max(1),
            advanced: advanced_explicit,
        }))
        .into_response());
    }

    let raw_rows: Vec<RawDiscRow> = select_query.fetch_all(&state.pool).await?;

    if export_csv {
        if raw_rows.len() as i64 > CSV_EXPORT_LIMIT {
            return Ok(csv_export_limit_response());
        }

        let disc_ids = raw_rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let flags_by_disc = load_disc_flags(&state.pool, &disc_ids).await?;
        let csv = build_disc_csv(&raw_rows, &flags_by_disc);
        return Ok((
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/csv; charset=utf-8"),
                ),
                (
                    header::CONTENT_DISPOSITION,
                    HeaderValue::from_static("attachment; filename=\"discs.csv\""),
                ),
                (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            ],
            csv,
        )
            .into_response());
    }

    let total_count = total_count.expect("disc page requests load a total count");

    if !quick_search_terms.is_empty() && total_count == 1 {
        if let Some(row) = raw_rows.first() {
            return Ok(Redirect::to(&format!("/disc/{}", row.id)).into_response());
        }
    }

    let disc_ids = raw_rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let mut flags_by_disc = load_disc_flags(&state.pool, &disc_ids).await?;
    let mut discs = Vec::with_capacity(raw_rows.len());
    for r in raw_rows {
        let flags = flags_by_disc.remove(&r.id).unwrap_or_default();
        let status = r.status;
        discs.push(DiscRow {
            id: r.id,
            title: format_display_title(
                &r.title,
                if r.has_disc_number {
                    r.disc_number.as_deref()
                } else {
                    None
                },
                if r.has_disc_title {
                    r.disc_title.as_deref()
                } else {
                    None
                },
                r.filename_suffix.as_deref(),
            ),
            title_foreign: if r.has_title_foreign {
                r.title_foreign.unwrap_or_default()
            } else {
                String::new()
            },
            system_display: crate::db::models::short_system_display(
                &r.system_short_name,
                &r.system_code,
            ),
            system_code: r.system_code,
            dumped_by_me: r.dumped_by_me,
            version: r.version.unwrap_or_default(),
            edition_display: if r.has_edition {
                r.edition.unwrap_or_default()
            } else {
                String::new()
            },
            status_class: status.css_class().to_string(),
            status_display: status.to_string(),
            region_flags: flags.regions,
            language_flags: flags.languages,
            serial: if r.has_serial {
                r.serial.unwrap_or_default()
            } else {
                String::new()
            },
        });
    }

    let is_asc = sort_order_str != "desc";
    let next_order = |col: &str| -> String {
        if sort_column == col && is_asc {
            "desc"
        } else {
            "asc"
        }
        .to_string()
    };

    Ok(Html(
        DiscsTemplate {
            current_user: user.user().cloned(),
            can_view_disabled_discs,
            discs,
            systems,
            regions,
            languages,
            media_types,
            categories,
            tags: tag_options,
            systems_media_json,
            letters: LETTERS
                .iter()
                .map(|s| (s.to_string(), filter_letter == *s))
                .collect(),
            filter_system,
            filter_region,
            filter_language,
            filter_media,
            filter_category,
            selected_tag_count: filter_tags.len(),
            filter_tags,
            filter_tag_mode,
            filter_status,
            filter_letter,
            filter_q,
            filter_title,
            title_mode: title_mode.as_query_value().to_string(),
            filter_title_foreign,
            title_foreign_mode: title_foreign_mode.as_query_value().to_string(),
            filter_disc_number,
            filter_disc_title,
            disc_title_mode: disc_title_mode.as_query_value().to_string(),
            filter_disc_title_foreign,
            disc_title_foreign_mode: disc_title_foreign_mode.as_query_value().to_string(),
            filter_serial,
            serial_mode: serial_mode.as_query_value().to_string(),
            filter_filename_suffix,
            filename_suffix_mode: filename_suffix_mode.as_query_value().to_string(),
            filter_exe_date_min,
            filter_exe_date_max,
            filter_pvd_date_min,
            filter_pvd_date_max,
            filter_edition,
            edition_mode: edition_mode.as_query_value().to_string(),
            filter_barcode,
            barcode_mode: barcode_mode.as_query_value().to_string(),
            filter_universal_hash,
            filter_tracks_min,
            filter_tracks_max,
            tracks_exact,
            filter_errors_min,
            filter_errors_max,
            errors_exact,
            filter_edc,
            filter_protection,
            filter_comments,
            filter_contents,
            filter_mastering_code,
            filter_mastering_sid,
            filter_toolstamp,
            filter_mould_sid,
            filter_additional_mould,
            filter_offset_min,
            filter_offset_max,
            offset_exact,
            filter_volume_label,
            volume_label_mode: volume_label_mode.as_query_value().to_string(),
            filter_internal_serial,
            internal_serial_mode: internal_serial_mode.as_query_value().to_string(),
            filter_version,
            version_mode: version_mode.as_query_value().to_string(),
            advanced_open,
            advanced_explicit,
            filter_dumper,
            filter_dumper_name,
            total_count,
            show_csv_export: (1..=CSV_EXPORT_LIMIT).contains(&total_count),
            page,
            total_pages,
            prev_page: page - 1,
            next_page: page + 1,
            sort_column: sort_column.clone(),
            sort_order: sort_order_str,
            dumped_filter,
            next_region_order: next_order("region"),
            next_title_order: next_order("title"),
            next_system_order: next_order("system"),
            next_version_order: next_order("version"),
            next_edition_order: next_order("edition"),
            next_language_order: next_order("language"),
            next_serial_order: next_order("serial"),
            next_status_order: next_order("status"),
        }
        .render()
        .unwrap(),
    )
    .into_response())
}

#[derive(Clone, sqlx::FromRow)]
struct SysRow {
    code: String,
    name: String,
}

#[derive(sqlx::FromRow)]
struct RawDiscRow {
    id: i32,
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    title_foreign: Option<String>,
    has_title_foreign: bool,
    has_disc_number: bool,
    has_disc_title: bool,
    has_edition: bool,
    has_serial: bool,
    system_code: String,
    system_short_name: String,
    serial: Option<String>,
    version: Option<String>,
    edition: Option<String>,
    status: DiscStatus,
    dumped_by_me: bool,
}

#[derive(Clone, sqlx::FromRow)]
struct SystemDropdownRow {
    code: String,
    manufacturer: String,
    name: String,
    media_types: Vec<String>,
}

#[derive(Clone, sqlx::FromRow)]
struct LangRow {
    code: String,
    name: String,
}

#[derive(Clone, sqlx::FromRow)]
struct DumperRow {
    id: i32,
    name: String,
}

struct DiscReferenceData {
    systems: Vec<SystemDropdownRow>,
    media: Vec<MediaFilterRow>,
    categories: Vec<CategoryFilterRow>,
    tags: Vec<String>,
    regions: Vec<SysRow>,
    languages: Vec<SysRow>,
}

async fn load_disc_reference_data(pool: &sqlx::PgPool) -> AppResult<DiscReferenceData> {
    let systems = sqlx::query_as(
        "SELECT code, manufacturer, name, media_types FROM systems
         ORDER BY LOWER(CONCAT_WS(' ', NULLIF(manufacturer, ''), name))",
    )
    .fetch_all(pool);
    let media =
        sqlx::query_as("SELECT code, name FROM media_types ORDER BY LOWER(name)").fetch_all(pool);
    let categories =
        sqlx::query_as("SELECT id, name FROM categories ORDER BY LOWER(name)").fetch_all(pool);
    let tags = sqlx::query_scalar::<_, String>("SELECT name FROM tags ORDER BY sort_order NULLS LAST, LOWER(name), name",)
        .fetch_all(pool);
    let regions =
        sqlx::query_as("SELECT code, name FROM regions ORDER BY LOWER(name)").fetch_all(pool);
    let languages = sqlx::query_as(
        "SELECT TRIM(code) AS code, name FROM languages ORDER BY sort_order, LOWER(name)",
    )
    .fetch_all(pool);

    let (systems, media, categories, tags, regions, languages) =
        tokio::try_join!(systems, media, categories, tags, regions, languages)?;

    Ok(DiscReferenceData {
        systems,
        media,
        categories,
        tags,
        regions,
        languages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media_row(code: &str, name: &str) -> MediaFilterRow {
        MediaFilterRow {
            code: code.to_string(),
            name: name.to_string(),
        }
    }

    fn category_row(id: i32, name: &str) -> CategoryFilterRow {
        CategoryFilterRow {
            id,
            name: name.to_string(),
        }
    }

    #[test]
    fn media_filter_options_follow_system_preference_and_keep_incompatible_selection_visible() {
        let rows = vec![
            media_row("bd25", "BD-25"),
            media_row("cd", "CD"),
            media_row("dvd5", "DVD-5"),
            media_row("gdrom", "GD-ROM"),
        ];
        let allowed = vec!["dvd5".to_string(), "cd".to_string()];
        let options = build_media_filter_options(&rows, Some(&allowed), "bd25");

        assert_eq!(
            options
                .iter()
                .map(|option| option.code.as_str())
                .collect::<Vec<_>>(),
            vec!["dvd5", "cd", "bd25", "gdrom"]
        );
        assert!(options[2].selected);
        assert!(!options[2].hidden);
        assert!(options[3].hidden);

        let all_options = build_media_filter_options(&rows, None, "cd");
        assert!(all_options.iter().all(|option| !option.hidden));
        assert_eq!(
            all_options
                .iter()
                .map(|option| option.code.as_str())
                .collect::<Vec<_>>(),
            vec!["bd25", "cd", "dvd5", "gdrom"]
        );
    }

    #[test]
    fn category_filter_uses_canonical_case_and_rejects_unknown_names() {
        let rows = vec![category_row(1, "Games"), category_row(2, "Bonus Discs")];

        assert_eq!(
            resolve_category_filter("bonus discs", &rows),
            ("Bonus Discs".to_string(), Some(2))
        );
        assert_eq!(
            resolve_category_filter("Unknown", &rows),
            ("Unknown".to_string(), None)
        );
        assert_eq!(resolve_category_filter("", &rows), (String::new(), None));
    }

    #[test]
    fn media_category_and_language_clauses_keep_binding_order_and_unknowns_match_nothing() {
        let mut clauses = vec!["s.code = $1".to_string()];
        let mut bind_idx = 1;
        add_media_category_clauses(&mut clauses, &mut bind_idx, "dvd5", Some(7), false);
        clauses.push("region = $4".to_string());
        bind_idx += 1;
        add_language_clause(&mut clauses, &mut bind_idx, "en");

        assert_eq!(bind_idx, 5);
        assert_eq!(
            clauses,
            vec![
                "s.code = $1",
                "d.media_type_code = $2",
                "d.category_id = $3",
                "region = $4",
                "EXISTS (SELECT 1 FROM disc_languages dl_filter WHERE dl_filter.disc_id = d.id AND dl_filter.language_code = $5)"
            ]
        );

        let mut clauses = Vec::new();
        let mut bind_idx = 0;
        add_media_category_clauses(&mut clauses, &mut bind_idx, "", None, true);
        add_language_clause(&mut clauses, &mut bind_idx, "");
        assert_eq!(bind_idx, 0);
        assert_eq!(clauses, vec!["FALSE"]);

        let mut clauses = Vec::new();
        let mut bind_idx = 0;
        add_language_clause(&mut clauses, &mut bind_idx, "none");
        assert_eq!(bind_idx, 0);
        assert_eq!(clauses, vec!["NOT EXISTS (SELECT 1 FROM disc_languages dl_filter WHERE dl_filter.disc_id = d.id)"]);
    }

    #[test]
    fn count_bounds_accept_only_non_negative_integers() {
        assert_eq!(normalize_non_negative_bound(None), None);
        assert_eq!(normalize_non_negative_bound(Some("")), None);
        assert_eq!(normalize_non_negative_bound(Some("abc")), None);
        assert_eq!(normalize_non_negative_bound(Some("1.5")), None);
        assert_eq!(normalize_non_negative_bound(Some("-1")), None);
        assert_eq!(normalize_non_negative_bound(Some("0")), Some(0));
        assert_eq!(normalize_non_negative_bound(Some(" 42 ")), Some(42));
    }

    #[test]
    fn track_count_clauses_support_open_bounded_exact_and_reversed_ranges() {
        let cases = [
            (None, None, Vec::<String>::new()),
            (Some(0), None, vec![format!("{TRACK_COUNT_SQL} >= $5")]),
            (None, Some(9), vec![format!("{TRACK_COUNT_SQL} <= $5")]),
            (
                Some(3),
                Some(9),
                vec![format!("{TRACK_COUNT_SQL} BETWEEN $5 AND $6")],
            ),
            (
                Some(7),
                Some(7),
                vec![format!("{TRACK_COUNT_SQL} BETWEEN $5 AND $6")],
            ),
            (
                Some(9),
                Some(3),
                vec![format!("{TRACK_COUNT_SQL} BETWEEN $5 AND $6")],
            ),
        ];

        for (tracks_min, tracks_max, expected) in cases {
            let mut clauses = Vec::new();
            let mut bind_idx = 4;
            add_track_count_clauses(&mut clauses, &mut bind_idx, tracks_min, tracks_max);

            assert_eq!(clauses, expected);
            let expected_binds = u32::from(tracks_min.is_some()) + u32::from(tracks_max.is_some());
            assert_eq!(bind_idx, 4 + expected_binds);
        }
    }

    #[test]
    fn track_count_requires_a_non_null_track_index() {
        assert_eq!(TRACK_COUNT_SQL, "track_filter.track_count");
        assert!(TRACK_COUNT_JOIN_SQL.contains("track_files.track_number IS NOT NULL"));
        assert!(TRACK_COUNT_JOIN_SQL.contains("OFFSET 0"));
    }

    #[test]
    fn track_bounds_precede_error_bounds_in_binding_order() {
        let mut clauses = Vec::new();
        let mut bind_idx = 4;

        add_track_count_clauses(&mut clauses, &mut bind_idx, Some(2), Some(8));
        add_error_count_clauses(&mut clauses, &mut bind_idx, Some(1), Some(3));

        assert_eq!(
            clauses,
            vec![
                format!("{TRACK_COUNT_SQL} BETWEEN $5 AND $6"),
                format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count >= $7)"),
                format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count <= $8)"),
            ]
        );
        assert_eq!(bind_idx, 8);
    }

    #[test]
    fn error_count_clauses_support_open_bounded_exact_and_reversed_ranges() {
        let cases = [
            (None, None, Vec::<String>::new()),
            (
                Some(3),
                None,
                vec![format!(
                    "({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count >= $5)"
                )],
            ),
            (
                None,
                Some(9),
                vec![format!(
                    "({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count <= $5)"
                )],
            ),
            (
                Some(3),
                Some(9),
                vec![
                    format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count >= $5)"),
                    format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count <= $6)"),
                ],
            ),
            (
                Some(7),
                Some(7),
                vec![
                    format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count >= $5)"),
                    format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count <= $6)"),
                ],
            ),
            (
                Some(9),
                Some(3),
                vec![
                    format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count >= $5)"),
                    format!("({ERROR_COUNT_APPLICABLE_SQL} AND d.error_count <= $6)"),
                ],
            ),
        ];

        for (errors_min, errors_max, expected) in cases {
            let mut clauses = Vec::new();
            let mut bind_idx = 4;
            add_error_count_clauses(&mut clauses, &mut bind_idx, errors_min, errors_max);

            assert_eq!(clauses, expected);
            assert_eq!(bind_idx, 4 + expected.len() as u32);
        }
    }

    #[test]
    fn edc_filter_normalizes_readable_values_and_ignores_unknowns() {
        assert_eq!(normalize_edc_filter(None), "");
        assert_eq!(normalize_edc_filter(Some("")), "");
        assert_eq!(normalize_edc_filter(Some(" YES ")), "yes");
        assert_eq!(normalize_edc_filter(Some("No")), "no");
        assert_eq!(normalize_edc_filter(Some("true")), "");
        assert_eq!(normalize_edc_filter(Some("unknown")), "");
    }

    #[test]
    fn edc_clauses_require_applicable_systems_without_bind_parameters() {
        let cases = [
            ("", Vec::<&str>::new()),
            ("yes", vec!["(s.has_edc AND d.edc)"]),
            ("no", vec!["(s.has_edc AND NOT d.edc)"]),
            ("unknown", Vec::<&str>::new()),
        ];

        for (edc, expected) in cases {
            let mut clauses = Vec::new();
            add_edc_clause(&mut clauses, edc);
            assert_eq!(clauses, expected);
            assert!(clauses.iter().all(|clause| !clause.contains('$')));
        }
    }

    #[test]
    fn advanced_selectors_render_in_order_and_are_preserved() {
        let template = include_str!("../../templates/discs.html");
        let region = template.find("<span>Region</span>").unwrap();
        let language = template.find("<span>Language</span>").unwrap();
        let media = template.find("<span>Media</span>").unwrap();
        let category = template.find("<span>Category</span>").unwrap();
        let dumper = template.find("<span>Dumper</span>").unwrap();
        let title = template
            .find("id=\"title-filter-label\">Title</span>")
            .unwrap();
        let title_foreign = template
            .find("id=\"title-foreign-filter-label\">Foreign Title</span>")
            .unwrap();
        let serial = template
            .find("id=\"serial-filter-label\">Disc Serial</span>")
            .unwrap();
        let edition = template
            .find("id=\"edition-filter-label\">Edition</span>")
            .unwrap();
        let barcode = template
            .find("id=\"barcode-filter-label\">Barcode</span>")
            .unwrap();
        let universal_hash = template.find("<span>Universal Hash</span>").unwrap();
        let tracks = template.find("<span>Tracks</span>").unwrap();
        let errors = template.find("<span>Errors</span>").unwrap();
        let edc = template.find("id=\"edc-filter-label\">EDC</span>").unwrap();
        let protection = template.find("<span>Protection</span>").unwrap();
        let comments = template.find("<span>Comments</span>").unwrap();
        let contents = template.find("<span>Contents</span>").unwrap();
        let mastering_code = template.find("<span>Mastering Code</span>").unwrap();
        let mastering_sid = template.find("<span>Mastering SID</span>").unwrap();
        let toolstamp = template.find("<span>Toolstamp</span>").unwrap();
        let mould_sid = template.find("<span>Mould SID</span>").unwrap();
        let additional_mould = template.find("<span>Additional Mould</span>").unwrap();
        let offset = template.find("<span>Offset</span>").unwrap();

        assert!(region < language && language < media && media < category);
        assert!(
            dumper < title
                && title < title_foreign
                && title_foreign < serial
                && serial < edition
                && edition < barcode
                && barcode < tracks
                && tracks < errors
                && errors < offset
                && offset < edc
                && edc < protection
                && protection < comments
                && comments < contents
                && contents < mastering_code
                && mastering_code < mastering_sid
                && mastering_sid < toolstamp
                && toolstamp < mould_sid
                && mould_sid < additional_mould
                && additional_mould < universal_hash
        );
        assert!(template.contains("<option value=\"\">All Languages</option>"));
        assert!(template.contains("<option value=\"none\""));
        assert!(template.contains(">No Language</option>"));
        assert!(template.contains("<option value=\"\">All Media</option>"));
        assert!(template.contains("<option value=\"\">All Categories</option>"));
        assert_eq!(template.matches("name=\"language\"").count(), 3);
        assert_eq!(template.matches("name=\"media\"").count(), 3);
        assert_eq!(template.matches("name=\"category\"").count(), 3);
        assert_eq!(template.matches("name=\"title\"").count(), 3);
        assert_eq!(template.matches("name=\"title_mode\"").count(), 3);
        assert_eq!(template.matches("name=\"title_foreign\"").count(), 3);
        assert_eq!(template.matches("name=\"title_foreign_mode\"").count(), 3);
        assert_eq!(template.matches("name=\"serial\"").count(), 3);
        assert_eq!(template.matches("name=\"serial_mode\"").count(), 3);
        assert_eq!(template.matches("name=\"edition\"").count(), 3);
        assert_eq!(template.matches("name=\"edition_mode\"").count(), 3);
        assert_eq!(template.matches("name=\"barcode\"").count(), 3);
        assert_eq!(template.matches("name=\"barcode_mode\"").count(), 3);
        assert_eq!(template.matches("name=\"universal_hash\"").count(), 3);
        assert_eq!(template.matches("name=\"tracks_min\"").count(), 3);
        assert_eq!(template.matches("name=\"tracks_max\"").count(), 4);
        assert_eq!(template.matches("name=\"errors_min\"").count(), 3);
        assert_eq!(template.matches("name=\"errors_max\"").count(), 4);
        assert_eq!(template.matches("name=\"edc\"").count(), 3);
        assert_eq!(template.matches("name=\"protection\"").count(), 3);
        assert_eq!(template.matches("name=\"comments\"").count(), 3);
        assert_eq!(template.matches("name=\"contents\"").count(), 3);
        assert_eq!(template.matches("name=\"mastering_code\"").count(), 3);
        assert_eq!(template.matches("name=\"mastering_sid\"").count(), 3);
        assert_eq!(template.matches("name=\"toolstamp\"").count(), 3);
        assert_eq!(template.matches("name=\"mould_sid\"").count(), 3);
        assert_eq!(template.matches("name=\"additional_mould\"").count(), 3);
        assert_eq!(template.matches("name=\"offset_min\"").count(), 3);
        assert_eq!(template.matches("name=\"offset_max\"").count(), 4);
        assert!(!template.contains("name=\"ringcode\""));
        assert!(!template.contains("edition_q"));
        assert!(!template.contains("comments_q"));
        assert!(template.contains("filterMediaOptions(true)"));
        assert!(template.contains("allowed.indexOf(current) === -1"));
    }

    #[test]
    fn exact_numeric_filter_controls_mirror_independently() {
        let template = include_str!("../../templates/discs.html");

        for prefix in ["tracks", "errors", "offset"] {
            assert!(template.contains(&format!("id=\"{prefix}-min\"")));
            assert!(template.contains(&format!("id=\"{prefix}-max\"")));
            assert!(template.contains(&format!("id=\"{prefix}-max-mirror\"")));
            assert!(template.contains(&format!("id=\"{prefix}-exact\"")));
            assert!(template.contains(&format!("initExactRange('{prefix}')")));
        }
        assert_eq!(template.matches("<span>Exact</span>").count(), 3);
        assert!(template.contains("{% if tracks_exact %}checked{% endif %}"));
        assert!(template.contains("{% if errors_exact %}checked{% endif %}"));
        assert!(template.contains("{% if offset_exact %}checked{% endif %}"));
        assert!(template.contains("max.value = min.value"));
        assert!(template.contains("maxMirror.value = min.value"));
        assert!(template.contains("max.disabled = true"));
        assert!(template.contains("maxMirror.disabled = false"));
    }

    #[test]
    fn text_match_modes_preserve_null_filters_without_text() {
        let template = include_str!("../../templates/discs.html");

        for (field, id) in [
            ("title_foreign", "title-foreign"),
            ("disc_title", "disc-title"),
            ("disc_title_foreign", "disc-title-foreign"),
            ("filename_suffix", "filename-suffix"),
            ("serial", "serial"),
            ("edition", "edition"),
            ("barcode", "barcode"),
            ("volume_label", "volume-label"),
            ("internal_serial", "internal-serial"),
        ] {
            assert!(template.contains(&format!("id=\"{id}-filter\" name=\"{field}\"")));
            assert!(template.contains(&format!(
                "name=\"{field}_mode\" class=\"text-match-mode\" data-target-input=\"{id}-filter\""
            )));
            assert!(template.contains(&format!(
                "<option value=\"null\" {{% if {field}_mode == \"null\" %}}selected{{% endif %}}>"
            )));
            assert!(template.contains(&format!(
                "<option value=\"any\" {{% if {field}_mode == \"any\" %}}selected{{% endif %}}>"
            )));
        }
        let title_row = template.split_once("id=\"title-filter-label\"").unwrap().1.split_once("</div>").unwrap().0;
        assert!(title_row.contains("value=\"exact\""));
        assert!(!title_row.contains("value=\"null\""));
        assert!(!title_row.contains("value=\"any\""));
        assert!(template.contains("input.disabled = select.value === 'null' || select.value === 'any'"));
    }

    #[test]
    fn edc_checkboxes_toggle_one_canonical_hidden_value() {
        let template = include_str!("../../templates/discs.html");

        assert!(template.contains("id=\"edc-filter-value\" name=\"edc\""));
        assert!(template.contains("id=\"edc-yes\" data-edc-value=\"yes\""));
        assert!(template.contains("id=\"edc-no\" data-edc-value=\"no\""));
        assert!(template.contains("{% if filter_edc == \"yes\" %}checked{% endif %}"));
        assert!(template.contains("{% if filter_edc == \"no\" %}checked{% endif %}"));
        assert!(template.contains("if (option !== changed) option.checked = false"));
        assert!(template.contains("edcValue.value = changed.dataset.edcValue"));
        assert!(template.contains("edcValue.value = ''"));
    }

    #[test]
    fn visible_sort_controls_follow_letters_and_keep_all_options() {
        let template = include_str!("../../templates/discs.html");
        let css = include_str!("../../static/css/app.css").replace("\r\n", "\n");

        for sort in [
            "title", "region", "system", "version", "edition", "language", "serial", "status",
            "added", "modified",
        ] {
            assert!(
                template.contains(&format!("<option value=\"{sort}\"")),
                "missing visible sort option {sort}"
            );
        }

        let sort_row = template
            .find("class=\"advanced-search-row disc-sort-row\"")
            .unwrap();
        let filter_form_end = template.find("    </form>").unwrap();
        let letter_picker = template
            .find("        <div class=\"letter-picker\">")
            .unwrap();
        assert!(letter_picker < sort_row && sort_row < filter_form_end);
        assert!(!template.contains("title|asc"));
        assert!(!template.contains("added|desc"));
        assert!(template.contains("name=\"sort\""));
        assert!(template.contains("aria-label=\"Sort direction\""));
        assert!(template.contains("aria-label=\"Ascending\""));
        assert!(template.contains("aria-label=\"Descending\""));
        assert!(template.contains("this.form.order.value='asc'"));
        assert!(template.contains("this.form.order.value='desc'"));
        assert!(css.contains(
            "#filter-form > .disc-sort-row {\n    grid-column: 1 / -1;\n    justify-self: center;"
        ));
        assert!(css.contains(
            "#filter-form > .letter-picker {\n    grid-column: 1 / -1;\n    justify-self: center;"
        ));
        assert_eq!(
            css.matches("#filter-form .disc-sort-controls .sort-select {")
                .count(),
            2
        );
        assert!(
            !css.contains("#filter-form.advanced-search-visible .disc-sort-controls .sort-select")
        );
    }

    #[test]
    fn main_filter_form_preserves_the_letter_and_resets_pagination() {
        let template = include_str!("../../templates/discs.html");
        let filter_form = template
            .split_once("<form method=\"get\" action=\"/discs\" id=\"filter-form\"")
            .unwrap()
            .1
            .split_once("</form>")
            .unwrap()
            .0;

        assert!(filter_form.contains("name=\"letter\" value=\"{{ filter_letter }}\""));
        assert!(filter_form.contains("name=\"sort\""));
        assert!(filter_form.contains("this.form.requestSubmit()"));
        assert!(!filter_form.contains("name=\"page\""));
        assert!(filter_form.contains("self.letter_url(\"\")"));
    }

    #[test]
    fn advanced_filter_labels_use_a_wide_consistent_column() {
        let css = include_str!("../../static/css/app.css");

        assert!(css.contains("--advanced-filter-label-width: 7rem;"));
        assert_eq!(
            css.matches("grid-template-columns: var(--advanced-filter-label-width)")
                .count(),
            3
        );
        assert!(css.contains(
            "calc(var(--advanced-filter-label-width) + 0.35rem + var(--advanced-system-control-width, 21.875rem))"
        ));
    }

    #[test]
    fn language_column_is_compact_and_wraps_against_available_width() {
        let css = include_str!("../../static/css/app.css").replace("\r\n", "\n");
        let template = include_str!("../../templates/discs.html");
        let language_rule = css
            .split_once(".disc-table .language-col {")
            .unwrap()
            .1
            .split_once('}')
            .unwrap()
            .0;
        let flags_rule = css
            .split_once(".disc-table .language-flags {")
            .unwrap()
            .1
            .split_once('}')
            .unwrap()
            .0;

        assert!(language_rule.contains("white-space: normal;"));
        assert!(language_rule.contains("line-height: 1;"));
        assert!(!language_rule
            .lines()
            .any(|line| line.trim_start().starts_with("max-width:")));
        assert!(language_rule
            .lines()
            .any(|line| line.trim() == "width: 1%;"));
        assert!(css.contains(".disc-table-scaler {\n    container-type: inline-size;"));
        assert!(flags_rule.contains("width: max-content;"));
        assert!(flags_rule.contains("max-width: 40vw;"));
        assert!(flags_rule.contains("max-width: 40cqi;"));
        assert!(flags_rule.contains("flex-wrap: wrap;"));
        assert!(flags_rule.contains("justify-content: flex-start;"));
        assert!(css.contains(".disc-table td.language-col {\n    text-align: left;"));
        assert!(template
            .contains("<span class=\"language-flags\">{% for lang in disc.language_flags %}"));
    }

    #[test]
    fn compact_disc_urls_omit_empty_and_default_state() {
        assert_eq!(
            build_discs_url(DiscsUrlOptions {
                system: "3DO",
                q: "A",
                sort: "title",
                order: "asc",
                page: 1,
                ..Default::default()
            }),
            "/discs?system=3DO&q=A"
        );
        assert_eq!(
            build_discs_url(DiscsUrlOptions {
                sort: "title",
                order: "asc",
                page: 1,
                ..Default::default()
            }),
            "/discs"
        );
    }

    #[test]
    fn compact_disc_urls_preserve_active_filters_navigation_and_encoding() {
        assert_eq!(
            build_discs_url(DiscsUrlOptions {
                system: "PS2",
                region: "us",
                language: "en",
                media: "dvd9",
                category: "Bonus Discs",
                tags: &[],
                tag_mode: "",
                status: "Verified",
                letter: "#",
                dumper: "A/B",
                title: "Game Title!",
                title_mode: "exact",
                title_foreign: "Foreign Title",
                title_foreign_mode: "exact",
                disc_number: "2",
                disc_title: "Bonus Disc",
                disc_title_mode: "exact",
                disc_title_foreign: "Foreign Disc Title",
                disc_title_foreign_mode: "exact",
                serial: "SLUS 12345",
                serial_mode: "exact",
                filename_suffix: "",
                filename_suffix_mode: "",
                exe_date_min: "",
                exe_date_max: "",
                pvd_date_min: "",
                pvd_date_max: "",
                edition: "Limited Edition",
                edition_mode: "exact",
                barcode: "0 12345 67890",
                barcode_mode: "exact",
                universal_hash: "AABBCCDDEEFF00112233445566778899AABBCCDD",
                tracks_min: "2",
                tracks_max: "8",
                errors_min: "3",
                errors_max: "12",
                edc: "no",
                protection: "SecuROM 7+",
                comments: "Disc & manual",
                contents: "Game data & extras",
                mastering_code: "MASTER  L0",
                mastering_sid: "IFPI L123",
                toolstamp: "A 1",
                mould_sid: "IFPI 1234",
                additional_mould: "A2",
                offset_min: "123",
                offset_max: "123",
                volume_label: "GAME_DISC",
                volume_label_mode: "exact",
                internal_serial: "SLUS-12345",
                internal_serial_mode: "exact",
                version: "",
                version_mode: "",
                q: "Game Name",
                sort: "status",
                order: "desc",
                dumped_filter: "",
                page: 2,
                advanced: true,
            }),
            "/discs?system=PS2&region=us&language=en&media=dvd9&category=Bonus%20Discs&status=Verified&letter=%23&dumper=A%2FB&title=Game%20Title%21&title_mode=exact&title_foreign=Foreign%20Title&title_foreign_mode=exact&disc_number=2&disc_title=Bonus%20Disc&disc_title_mode=exact&disc_title_foreign=Foreign%20Disc%20Title&disc_title_foreign_mode=exact&serial=SLUS%2012345&serial_mode=exact&edition=Limited%20Edition&edition_mode=exact&barcode=0%2012345%2067890&barcode_mode=exact&universal_hash=AABBCCDDEEFF00112233445566778899AABBCCDD&tracks_min=2&tracks_max=8&errors_min=3&errors_max=12&edc=no&protection=SecuROM%207%2B&comments=Disc%20%26%20manual&contents=Game%20data%20%26%20extras&mastering_code=MASTER%20%20L0&mastering_sid=IFPI%20L123&toolstamp=A%201&mould_sid=IFPI%201234&additional_mould=A2&offset_min=123&offset_max=123&volume_label=GAME_DISC&volume_label_mode=exact&internal_serial=SLUS-12345&internal_serial_mode=exact&q=Game%20Name&sort=status&order=desc&page=2&advanced=1"
        );

        assert_eq!(
            build_discs_url(DiscsUrlOptions {
                tracks_min: "7",
                tracks_max: "7",
                sort: "title",
                order: "asc",
                page: 1,
                ..Default::default()
            }),
            "/discs?tracks_min=7&tracks_max=7"
        );

        assert_eq!(
            build_discs_url(DiscsUrlOptions {
                title_mode: "exact",
                title_foreign_mode: "exact",
                serial_mode: "exact",
                edition_mode: "exact",
                barcode_mode: "exact",
                sort: "title",
                order: "asc",
                page: 1,
                ..Default::default()
            }),
            "/discs?title_mode=exact&title_foreign_mode=exact&serial_mode=exact&edition_mode=exact&barcode_mode=exact"
        );
    }

    #[test]
    fn quick_search_terms_match_old_redump_normalization() {
        assert_eq!(
            quick_search_terms(" Final Fantasy/VII: Disc & Serial "),
            vec!["final", "fantasy", "vii", "disc", "serial"]
        );
        assert_eq!(quick_search_terms("SCES-00894"), vec!["sces", "00894"]);
        assert_eq!(
            quick_search_terms("foo__bar//baz::qux&&zap"),
            vec!["foo", "bar", "baz", "qux", "zap"]
        );
        assert!(quick_search_terms(" --  /  ").is_empty());
    }

    #[test]
    fn hash_field_for_term_accepts_only_full_hex_hashes() {
        assert_eq!(hash_field_for_term("deadbeef"), Some(HashField::Crc32));
        assert_eq!(
            hash_field_for_term("0123456789abcdef0123456789abcdef"),
            Some(HashField::Md5)
        );
        assert_eq!(
            hash_field_for_term("0123456789abcdef0123456789abcdef01234567"),
            Some(HashField::Sha1)
        );
        assert_eq!(hash_field_for_term("deadbee"), None);
        assert_eq!(hash_field_for_term("deadbeef00"), None);
        assert_eq!(hash_field_for_term("nothex!!"), None);
    }

    #[test]
    fn status_filter_hides_disabled_choices_without_permission() {
        assert_eq!(normalize_status_filter(None, false), "");
        assert_eq!(normalize_status_filter(Some(""), false), "");
        assert_eq!(normalize_status_filter(Some("Verified"), false), "Verified");
        assert_eq!(normalize_status_filter(Some("verified"), false), "Verified");
        assert_eq!(
            normalize_status_filter(Some("Unverified"), false),
            "Unverified"
        );
        assert_eq!(
            normalize_status_filter(Some("Questionable"), false),
            "Questionable"
        );
        assert_eq!(normalize_status_filter(Some("Disabled"), false), "");
        assert_eq!(normalize_status_filter(Some("All Statuses"), false), "");
        assert_eq!(normalize_status_filter(Some("nope"), false), "");
    }

    #[test]
    fn status_filter_preserves_disabled_choices_with_permission() {
        assert_eq!(normalize_status_filter(Some("Disabled"), true), "Disabled");
        assert_eq!(normalize_status_filter(Some("disabled"), true), "Disabled");
        assert_eq!(
            normalize_status_filter(Some("All Statuses"), true),
            "All Statuses"
        );
        assert_eq!(
            normalize_status_filter(Some("all statuses"), true),
            "All Statuses"
        );
    }

    #[test]
    fn url_filters_are_case_insensitive() {
        assert_eq!(normalize_system_filter(Some("ps3")), "PS3");
        assert_eq!(normalize_system_filter(Some("pc-98")), "PC-98");
        assert_eq!(normalize_region_filter(Some("US")), "us");
        assert_eq!(normalize_language_filter(Some(" EN ")), "en");
        assert_eq!(normalize_media_filter(Some(" DVD9 ")), "dvd9");
        assert_eq!(normalize_letter_filter(Some("b")), "B");
        assert_eq!(normalize_disc_sort(Some("STATUS")), "status");
        assert_eq!(normalize_disc_sort(Some("MODIFIED")), "modified");
        assert_eq!(normalize_disc_sort(Some("updated")), "title");
        assert_eq!(normalize_sort_order(Some("DESC")), "desc");
    }

    #[test]
    fn quick_search_clause_for_text_terms_uses_only_indexed_title_foreign_title_and_serial() {
        let clause = quick_search_clause(3, None);

        assert!(clause.contains("search_fold(d.title) LIKE"));
        assert!(clause.contains("search_fold(d.title_foreign) LIKE"));
        assert!(clause.contains("replace(search_fold($3)"));
        assert!(clause.contains("compact_disc_array_search(d.serial) LIKE"));
        assert!(!clause.contains("disc_title"));
        assert!(!clause.contains("barcode"));
        assert!(!clause.contains("FROM files"));
    }

    #[test]
    fn quick_search_clause_for_hash_terms_adds_prefetched_disc_candidates() {
        let clause = quick_search_clause(4, Some(5));

        assert!(clause.contains("d.id = ANY($5)"));
        assert!(!clause.contains("FROM files"));
        assert!(!clause.contains("barcode"));
        assert!(!clause.contains("disc_title"));
    }

    #[test]
    fn quick_search_prefetches_file_sha1_and_disc_universal_hash_candidates() {
        let sha1 = "0123456789abcdef0123456789abcdef01234567".to_owned();
        let query = build_hash_candidate_query(&[sha1]).unwrap();
        let sql = query.sql();

        assert!(
            sql.contains("SELECT LOWER(sha1) AS term, disc_id FROM files WHERE LOWER(sha1) = ANY(")
        );
        assert!(sql.contains("UNION ALL"));
        assert!(sql.contains(
            "SELECT encode(universal_hash, 'hex') AS term, id AS disc_id FROM discs WHERE universal_hash IN ("
        ));

        let crc_query = build_hash_candidate_query(&["deadbeef".to_owned()]).unwrap();
        assert!(crc_query.sql().contains("LOWER(crc32)"));
        assert!(!crc_query.sql().contains("universal_hash"));
        assert!(build_hash_candidate_query(&["ordinary".to_owned()]).is_none());
    }

    #[test]
    fn active_advanced_filter_uses_non_empty_text() {
        let text = "  Original Edition  ".to_string();
        let empty = "   ".to_string();

        assert_eq!(
            active_advanced_filter(Some(&text)),
            Some("Original Edition".to_string())
        );
        assert_eq!(active_advanced_filter(Some(&empty)), None);
        assert_eq!(active_advanced_filter(None), None);
    }

    #[test]
    fn advanced_panel_opens_only_for_explicit_state() {
        assert!(advanced_panel_explicitly_open(Some("1")));
        assert!(!advanced_panel_explicitly_open(None));
        assert!(!advanced_panel_explicitly_open(Some("0")));
        assert!(!advanced_panel_explicitly_open(Some("true")));
    }

    #[test]
    fn array_text_filters_preserve_verbatim_values_and_ignore_blank_input() {
        let text = "  SLUS 12345  ".to_string();
        let empty = " \t\n ".to_string();

        assert_eq!(active_verbatim_filter(Some(&text)), Some(text.clone()));
        assert_eq!(active_verbatim_filter(Some(&empty)), None);
        assert_eq!(active_verbatim_filter(None), None);
    }

    #[test]
    fn universal_hash_filter_matches_native_bytes_and_rejects_invalid_text() {
        assert_eq!(universal_hash_search_clause(9), "d.universal_hash = $9");

        let mut clauses = Vec::new();
        let mut bind_idx = 8;
        add_universal_hash_clause(&mut clauses, &mut bind_idx, true, Some(&[0; 20]));
        assert_eq!(bind_idx, 9);
        assert_eq!(clauses, ["d.universal_hash = $9".to_owned()]);

        let mut invalid_clauses = Vec::new();
        add_universal_hash_clause(&mut invalid_clauses, &mut bind_idx, true, None);
        assert_eq!(bind_idx, 9);
        assert_eq!(invalid_clauses, ["FALSE".to_owned()]);

        add_universal_hash_clause(&mut invalid_clauses, &mut bind_idx, false, None);
        assert_eq!(invalid_clauses, ["FALSE".to_owned()]);
    }

    #[test]
    fn array_text_bind_values_ignore_whitespace_and_case_only_when_not_exact() {
        let value = " SlUs\t12\n345-Ä ";

        assert_eq!(array_text_bind_value(value, false), "slus12345-ä");
        assert_eq!(array_text_bind_value(value, true), value);
    }

    #[test]
    fn scalar_title_bind_values_trim_only_when_not_exact() {
        let value = "  Game  Title!  ";

        assert_eq!(scalar_text_bind_value(value, false), "Game  Title!");
        assert_eq!(scalar_text_bind_value(value, true), value);
    }

    #[test]
    fn text_match_modes_accept_known_values_only() {
        assert_eq!(
            TextMatchMode::from_query(Some("exact")),
            TextMatchMode::Exact
        );
        assert_eq!(
            TextMatchMode::from_query(Some("null")),
            TextMatchMode::IsNull
        );
        assert_eq!(
            TextMatchMode::from_query(Some("any")),
            TextMatchMode::Any
        );
        assert_eq!(
            TextMatchMode::from_query(Some("1")),
            TextMatchMode::Contains
        );
        assert_eq!(TextMatchMode::from_query(None), TextMatchMode::Contains);
    }

    #[test]
    fn array_text_search_clauses_match_individual_entries_and_require_capabilities() {
        let fields = [
            (ArrayTextField::Serial, "serial", "has_serial"),
            (ArrayTextField::Edition, "edition", "has_edition"),
            (ArrayTextField::Barcode, "barcode", "has_barcode"),
        ];

        for (offset, (field, column, capability)) in fields.into_iter().enumerate() {
            let bind_idx = offset as u32 + 5;
            assert_eq!(
                array_text_search_clause(field, bind_idx, false),
                format!(
                    "(s.{capability} AND compact_disc_array_search(d.{column}) {compact} AND EXISTS (SELECT 1 FROM unnest(d.{column}) AS filter_value(value) WHERE LOWER(REGEXP_REPLACE(filter_value.value, '[[:space:]]+', '', 'g')) {entry}))",
                    compact = like_contains(&format!("${bind_idx}")),
                    entry = like_contains(&format!("${bind_idx}")),
                )
            );
            assert_eq!(
                array_text_search_clause(field, bind_idx, true),
                format!(
                    "(s.{capability} AND compact_disc_array_search(d.{column}) LIKE '%' || compact_disc_array_search(ARRAY[${bind_idx}]::TEXT[]) || '%' AND EXISTS (SELECT 1 FROM unnest(d.{column}) AS filter_value(value) WHERE filter_value.value = ${bind_idx}))"
                )
            );
        }
    }

    #[test]
    fn scalar_text_clauses_use_field_specific_search_and_null_rules() {
        assert_eq!(
            scalar_text_search_clause(ScalarTextField::Title, 5, false),
            accent_aware_contains("d.title", "$5")
        );
        assert_eq!(
            scalar_text_search_clause(ScalarTextField::Title, 5, true),
            "d.title = $5"
        );
        assert_eq!(
            scalar_text_search_clause(ScalarTextField::ForeignTitle, 6, false),
            format!(
                "(s.has_title_foreign AND {})",
                accent_aware_contains("d.title_foreign", "$6")
            )
        );
        assert_eq!(
            scalar_text_search_clause(ScalarTextField::ForeignTitle, 6, true),
            "(s.has_title_foreign AND d.title_foreign = $6)"
        );
        for (field, column) in [
            (ScalarTextField::VolumeLabel, "volume_label"),
            (ScalarTextField::InternalSerial, "internal_serial"),
        ] {
            assert_eq!(
                scalar_text_search_clause(field, 7, false),
                format!(
                    "LOWER(d.{column}) {}",
                    like_contains("LOWER($7)")
                )
            );
            assert_eq!(
                scalar_text_null_clause(field),
                format!("d.{column} IS NULL")
            );
        }
    }

    #[test]
    fn scalar_text_filters_precede_array_text_filters_in_binding_order() {
        let mut clauses = Vec::new();
        let mut bind_idx = 4;

        add_scalar_text_clauses(
            &mut clauses,
            &mut bind_idx,
            [
                (ScalarTextField::Title, true, TextMatchMode::Contains),
                (ScalarTextField::ForeignTitle, true, TextMatchMode::Exact),
                (ScalarTextField::DiscNumber, false, TextMatchMode::Contains),
                (ScalarTextField::DiscTitle, false, TextMatchMode::Contains),
                (ScalarTextField::ForeignDiscTitle, false, TextMatchMode::Contains),
                (ScalarTextField::VolumeLabel, true, TextMatchMode::Contains),
                (ScalarTextField::InternalSerial, true, TextMatchMode::Contains),
                (ScalarTextField::FilenameSuffix, false, TextMatchMode::Contains),
                (ScalarTextField::Version, false, TextMatchMode::Contains),
            ],
        );
        add_array_text_clauses(
            &mut clauses,
            &mut bind_idx,
            [
                (ArrayTextField::Serial, true, TextMatchMode::Contains),
                (ArrayTextField::Edition, false, TextMatchMode::Contains),
                (ArrayTextField::Barcode, false, TextMatchMode::Contains),
            ],
        );

        assert_eq!(bind_idx, 9);
        assert_eq!(
            clauses[0],
            accent_aware_contains("d.title", "$5")
        );
        assert_eq!(clauses[1], "(s.has_title_foreign AND d.title_foreign = $6)");
        assert!(clauses[2].contains("LOWER(d.volume_label)"));
        assert!(clauses[2].contains("$7"));
        assert!(clauses[3].contains("LOWER(d.internal_serial)"));
        assert!(clauses[3].contains("$8"));
        assert!(clauses[4].contains("unnest(d.serial)"));
        assert!(clauses[4].contains("$9"));
    }

    #[test]
    fn array_text_clauses_keep_serial_edition_barcode_binding_order() {
        let mut clauses = Vec::new();
        let mut bind_idx = 4;

        add_array_text_clauses(
            &mut clauses,
            &mut bind_idx,
            [
                (ArrayTextField::Serial, true, TextMatchMode::Contains),
                (ArrayTextField::Edition, true, TextMatchMode::Exact),
                (ArrayTextField::Barcode, true, TextMatchMode::Contains),
            ],
        );

        assert_eq!(bind_idx, 7);
        assert!(clauses[0].contains("unnest(d.serial)"));
        assert!(clauses[0].contains("$5"));
        assert!(clauses[1].contains("unnest(d.edition)"));
        assert!(clauses[1].contains("$6"));
        assert!(clauses[1].contains("filter_value.value = $6"));
        assert!(clauses[2].contains("unnest(d.barcode)"));
        assert!(clauses[2].contains("$7"));
    }

    #[test]
    fn advanced_scalar_search_clauses_use_case_insensitive_substring_matching() {
        let comments_clause = comments_search_clause(7);
        let contents_clause = contents_search_clause(8);

        assert_eq!(
            comments_clause,
            accent_aware_contains("d.comments", "$7")
        );
        assert_eq!(
            contents_clause,
            accent_aware_contains("d.contents", "$8")
        );
    }

    #[test]
    fn offset_filter_normalizes_signed_integers_and_ignores_invalid_values() {
        assert_eq!(normalize_offset_filter(None), None);
        assert_eq!(normalize_offset_filter(Some("")), None);
        assert_eq!(normalize_offset_filter(Some("   ")), None);
        assert_eq!(normalize_offset_filter(Some("+123")), Some(123));
        assert_eq!(normalize_offset_filter(Some(" 123 ")), Some(123));
        assert_eq!(normalize_offset_filter(Some("-123")), Some(-123));
        assert_eq!(normalize_offset_filter(Some("0")), Some(0));
        assert_eq!(normalize_offset_filter(Some("abc")), None);
        assert_eq!(normalize_offset_filter(Some("2147483648")), None);
    }

    #[test]
    fn ring_text_clauses_search_only_the_selected_field_on_any_layer() {
        let cases = [
            (RingTextField::MasteringCode, "mastering_code"),
            (RingTextField::MasteringSid, "mastering_sid"),
            (RingTextField::Toolstamp, "toolstamps"),
            (RingTextField::MouldSid, "mould_sids"),
            (RingTextField::AdditionalMould, "additional_moulds"),
        ];

        for (field, expected_column) in cases {
            let clause = ring_text_search_clause(field, 8);

            assert!(clause.contains("FROM disc_ring_code_entries ring_entry"));
            assert!(clause.contains("JOIN disc_ring_code_layers ring_layer"));
            assert!(clause.contains("ring_layer.entry_id = ring_entry.id"));
            assert!(clause.contains("ring_entry.disc_id = d.id"));
            assert!(clause.contains(&format!(
                "public.ringcode_field_search_text(ring_layer.{expected_column})"
            )));
            assert_eq!(clause.matches("ringcode_field_search_text(").count(), 1);
            assert_eq!(clause.matches("LOWER($8)").count(), 1);
        }
    }

    #[test]
    fn offset_range_clause_matches_primary_or_applicable_extra_offset() {
        assert_eq!(
            offset_range_clause(Some(9), Some(10)),
            "EXISTS (SELECT 1 FROM disc_ring_code_entries offset_entry WHERE offset_entry.disc_id = d.id AND ((offset_entry.offset_value >= $9 AND offset_entry.offset_value <= $10) OR (s.has_offset_extra AND (offset_entry.offset_extra_value >= $9 AND offset_entry.offset_extra_value <= $10))))"
        );
    }

    #[test]
    fn contents_ring_fields_and_offset_follow_comments_in_binding_order() {
        let mut clauses = vec![comments_search_clause(7), contents_search_clause(8)];
        let mut bind_idx = 8;

        add_ring_filter_clauses(
            &mut clauses,
            &mut bind_idx,
            [
                (RingTextField::MasteringCode, true),
                (RingTextField::MasteringSid, true),
                (RingTextField::Toolstamp, true),
                (RingTextField::MouldSid, true),
                (RingTextField::AdditionalMould, true),
            ],
            true,
            true,
        );

        assert_eq!(bind_idx, 15);
        assert_eq!(clauses.len(), 8);
        assert!(clauses[1].contains("search_fold($8)"));
        assert!(clauses[2].contains("LOWER($9)"));
        assert!(clauses[3].contains("LOWER($10)"));
        assert!(clauses[4].contains("LOWER($11)"));
        assert!(clauses[5].contains("LOWER($12)"));
        assert!(clauses[6].contains("LOWER($13)"));
        assert!(clauses[7].contains("offset_entry.offset_value >= $14"));
        assert!(clauses[7].contains("offset_entry.offset_value <= $15"));
    }

    #[test]
    fn protection_search_respects_system_and_guest_visibility() {
        assert_eq!(
            protection_search_clause(6, true),
            format!(
                "(s.has_protection AND LOWER(d.protection) {})",
                like_contains("LOWER($6)")
            )
        );
        assert_eq!(
            protection_search_clause(6, false),
            format!(
                "(s.has_protection AND s.code NOT IN ('BD-VIDEO', 'HDDVD-VIDEO') AND LOWER(d.protection) {})",
                like_contains("LOWER($6)")
            )
        );
    }

    #[test]
    fn disc_query_uses_unsuffixed_advanced_text_parameters_only() {
        let uri = "/discs?title=Game&title_mode=exact&title_foreign=Foreign&title_foreign_mode=exact&serial=SLUS-12345&serial_mode=exact&edition=Limited&edition_mode=exact&barcode=012345&barcode_mode=exact&universal_hash=aabbccddeeff00112233445566778899aabbccdd&protection=SecuROM&comments=Note&contents=Bonus%20videos&mastering_code=MASTER-L0&mastering_sid=IFPI-L123&toolstamp=A1&mould_sid=IFPI-1234&additional_mould=A2&offset_min=-123&offset_max=456&ringcode=OLD&edition_q=Old&comments_q=Old"
                .parse()
                .unwrap();
        let MultiQuery(query) = MultiQuery::<DiscsQuery>::try_from_uri(&uri).unwrap();

        assert_eq!(query.title.as_deref(), Some("Game"));
        assert_eq!(query.title_mode.as_deref(), Some("exact"));
        assert_eq!(query.title_foreign.as_deref(), Some("Foreign"));
        assert_eq!(query.title_foreign_mode.as_deref(), Some("exact"));
        assert_eq!(query.serial.as_deref(), Some("SLUS-12345"));
        assert_eq!(query.serial_mode.as_deref(), Some("exact"));
        assert_eq!(query.edition.as_deref(), Some("Limited"));
        assert_eq!(query.edition_mode.as_deref(), Some("exact"));
        assert_eq!(query.barcode.as_deref(), Some("012345"));
        assert_eq!(query.barcode_mode.as_deref(), Some("exact"));
        assert_eq!(
            query.universal_hash.as_deref(),
            Some("aabbccddeeff00112233445566778899aabbccdd")
        );
        assert_eq!(query.protection.as_deref(), Some("SecuROM"));
        assert_eq!(query.comments.as_deref(), Some("Note"));
        assert_eq!(query.contents.as_deref(), Some("Bonus videos"));
        assert_eq!(query.mastering_code.as_deref(), Some("MASTER-L0"));
        assert_eq!(query.mastering_sid.as_deref(), Some("IFPI-L123"));
        assert_eq!(query.toolstamp.as_deref(), Some("A1"));
        assert_eq!(query.mould_sid.as_deref(), Some("IFPI-1234"));
        assert_eq!(query.additional_mould.as_deref(), Some("A2"));
        assert_eq!(query.offset_min.as_deref(), Some("-123"));
        assert_eq!(query.offset_max.as_deref(), Some("456"));

        let legacy_uri = "/discs?edition_q=Old&comments_q=Old".parse().unwrap();
        let MultiQuery(legacy_query) = MultiQuery::<DiscsQuery>::try_from_uri(&legacy_uri).unwrap();
        assert!(legacy_query.edition.is_none());
        assert!(legacy_query.protection.is_none());
        assert!(legacy_query.comments.is_none());
        assert!(legacy_query.contents.is_none());
    }

    #[test]
    fn title_sort_uses_the_indexed_display_title_key() {
        assert_eq!(display_title_sort_sql(), "d.display_title_sort_key");
    }

    #[test]
    fn modified_sort_uses_every_public_history_row() {
        let sql = disc_date_sort_sql("modified").unwrap().cte;
        assert!(sql.contains(disc_service::DISC_HISTORY_MODIFIED_AT_SQL));
        assert!(sql.contains(disc_service::PUBLIC_DISC_HISTORY_PREDICATE));
        assert!(!sql.contains("first_public_submission"));
        assert!(!sql.contains("submission_type"));
        assert!(!sql.contains("changes"));
        assert!(!sql.contains("review_comment"));
    }

    #[test]
    fn added_and_modified_sorts_only_use_public_review_times() {
        for sort_column in ["added", "modified"] {
            let sql = disc_date_sort_sql(sort_column).unwrap().cte;
            assert!(sql.contains(disc_service::PUBLIC_DISC_HISTORY_PREDICATE));
            assert!(sql.contains("ds.reviewed_at"));
            assert!(!sql.contains("created_at"));
        }
    }

    #[test]
    fn non_title_sorts_use_title_and_id_tiebreakers_in_the_same_direction() {
        for sort_column in [
            "region", "system", "version", "edition", "language", "serial", "status", "added",
            "modified",
        ] {
            for direction in ["ASC", "DESC"] {
                let order_by = disc_order_by_sql(sort_column, "PRIMARY_SORT", direction);
                let nulls = if matches!(sort_column, "added" | "modified") {
                    " NULLS LAST"
                } else {
                    ""
                };

                assert_eq!(
                    order_by,
                    format!(
                        "PRIMARY_SORT {direction}{nulls}, {} {direction}, d.id {direction}",
                        display_title_sort_sql()
                    ),
                    "unexpected ordering for {sort_column} {direction}"
                );
            }
        }
    }

    #[test]
    fn title_sort_uses_id_as_its_only_tiebreaker() {
        for direction in ["ASC", "DESC"] {
            assert_eq!(
                disc_order_by_sql("title", display_title_sort_sql(), direction),
                format!("{} {direction}, d.id {direction}", display_title_sort_sql())
            );
        }
    }

    #[test]
    fn count_cache_key_ignores_navigation_but_separates_protection_visibility() {
        let first = build_discs_count_cache_key(
            DiscsUrlOptions {
                system: "PS2",
                title: "Game",
                sort: "modified",
                order: "desc",
                page: 9,
                advanced: true,
                ..Default::default()
            },
            false,
            None,
        );
        let navigated = build_discs_count_cache_key(
            DiscsUrlOptions {
                system: "PS2",
                title: "Game",
                sort: "title",
                order: "asc",
                page: 1,
                ..Default::default()
            },
            false,
            None,
        );
        let member = build_discs_count_cache_key(
            DiscsUrlOptions {
                system: "PS2",
                title: "Game",
                ..Default::default()
            },
            true,
            None,
        );

        assert_eq!(first, navigated);
        assert_ne!(first, member);
        assert!(!first.contains("sort="));
        assert!(!first.contains("page="));
        assert!(!first.contains("advanced="));
    }

    #[tokio::test]
    async fn count_cache_expires_and_is_bounded() {
        let cache = CountCache::new(Duration::from_millis(200), 2);
        cache.insert("one".to_string(), 1).await;
        assert_eq!(cache.get("one").await, Some(1));
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(cache.get("one").await, None);

        cache.insert("two".to_string(), 2).await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        cache.insert("three".to_string(), 3).await;
        cache.insert("four".to_string(), 4).await;
        assert_eq!(cache.values.read().await.len(), 2);
        assert_eq!(cache.get("two").await, None);
    }

    #[tokio::test]
    async fn invalidating_dumper_data_clears_directory_and_count_caches() {
        let cache = DiscsCache::new(
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(60),
        );
        cache.counts.insert("dumper=Alice".to_string(), 3).await;
        *cache.dumpers.value.write().await = Some(TimedValue::new(Arc::new(DumperDirectory {
            body: r#"["Alice"]"#.to_string(),
            etag: "test-etag".to_string(),
        })));

        cache.invalidate_dumper_data().await;

        assert!(cache.counts.values.read().await.is_empty());
        assert!(cache.dumpers.value.read().await.is_none());
    }

    #[test]
    fn disc_flags_are_grouped_without_per_disc_queries() {
        let rows = vec![
            DiscFlagRow {
                disc_id: 7,
                flag_kind: 0,
                code: "US".to_string(),
                name: "USA".to_string(),
            },
            DiscFlagRow {
                disc_id: 7,
                flag_kind: 1,
                code: "EN".to_string(),
                name: "English".to_string(),
            },
            DiscFlagRow {
                disc_id: 8,
                flag_kind: 0,
                code: "JP".to_string(),
                name: "Japan".to_string(),
            },
        ];

        let grouped = group_disc_flags(rows);
        assert_eq!(grouped[&7].regions[0].code, "us");
        assert_eq!(grouped[&7].languages[0].code, "en");
        assert_eq!(grouped[&8].regions[0].name, "Japan");

        let source = include_str!("discs.rs");
        assert!(source.contains("WHERE dr.disc_id = ANY($1)"));
        assert!(source.contains("WHERE dl.disc_id = ANY($1)"));
    }

    #[test]
    fn cache_ttls_and_dumper_hydration_match_delivery_policy() {
        assert_eq!(REFERENCE_CACHE_TTL, Duration::from_secs(86_400));
        assert_eq!(COUNT_CACHE_TTL, Duration::from_secs(60));
        assert_eq!(DUMPER_CACHE_TTL, Duration::from_secs(3_600));

        let template = include_str!("../../templates/discs.html");
        assert!(template.contains("fetch('/api/disc-dumpers'"));
        assert!(template.contains("data-selected-dumper"));
        assert!(!template.contains("{% for d in dumpers %}"));

        let caddyfile = include_str!("../../Caddyfile");
        assert!(caddyfile.contains("encode zstd"));
        assert!(!caddyfile.contains("encode zstd gzip"));
    }

    #[test]
    fn migration_defines_search_indexes_and_sort_key_triggers() {
        let migration = include_str!("../../migrations/012_optimize_discs_page.sql");
        for expected in [
            "compact_disc_array_search",
            "ringcode_layer_search_text",
            "display_title_sort_key",
            "idx_disc_regions_region_disc",
            "idx_disc_languages_language_disc",
            "idx_disc_dumpers_user_disc",
            "idx_files_indexed_disc",
            "idx_discs_protection_trgm",
            "DROP COLUMN search_vector",
        ] {
            assert!(migration.contains(expected), "missing {expected}");
        }

        let old_modification_index =
            include_str!("../../migrations/013_index_disc_modification_sort.sql");
        assert!(old_modification_index.contains("idx_submissions_genuine_change_target_time"));
        assert!(old_modification_index
            .contains("DROP INDEX idx_submissions_public_target_created_desc"));

        let public_history_index =
            include_str!("../../migrations/019_index_public_disc_history_modified.sql");
        assert!(public_history_index.contains("idx_submissions_public_history_target_time"));
        assert!(public_history_index.contains("INCLUDE (reviewed_at, created_at)"));
        assert!(public_history_index.contains("status IN ('Approved', 'Legacy')"));
        assert!(
            public_history_index.contains("DROP INDEX idx_submissions_genuine_change_target_time")
        );
        assert!(!public_history_index.contains("submission_type"));
        assert!(!public_history_index.contains("changes"));
        assert!(!public_history_index.contains("review_comment"));

        let reviewed_dates =
            include_str!("../../migrations/022_use_reviewed_at_for_public_dates.sql");
        assert!(reviewed_dates.contains("SET reviewed_at = created_at"));
        assert!(reviewed_dates.contains("disc_submissions_public_reviewed_at_check"));
        assert!(reviewed_dates.contains("DROP INDEX idx_submissions_target_created"));
        assert!(reviewed_dates.contains("DROP INDEX idx_submissions_public_history_target_time"));
        assert!(reviewed_dates.contains("ON disc_submissions (target_disc_id, reviewed_at)"));
        assert!(reviewed_dates.contains("status IN ('Approved', 'Legacy')"));

        let contents_index = include_str!("../../migrations/014_index_disc_contents_search.sql");
        assert!(contents_index.contains("idx_discs_contents_trgm"));
        assert!(contents_index.contains("LOWER(contents) gin_trgm_ops"));

        let fold_indexes =
            include_str!("../../migrations/036_unaccent_search.sql");
        for column in [
            "title",
            "title_foreign",
            "disc_title",
            "disc_title_foreign",
            "comments",
            "contents",
        ] {
            assert!(
                fold_indexes.contains(&format!("public.search_fold({column}) gin_trgm_ops")),
                "missing search_fold index for {column}"
            );
        }

        let ring_field_indexes = include_str!("../../migrations/020_index_ring_code_fields.sql");
        assert!(ring_field_indexes.contains("ringcode_field_search_text(TEXT)"));
        for (index, column) in [
            ("idx_ring_layers_mastering_code_trgm", "mastering_code"),
            ("idx_ring_layers_mastering_sid_trgm", "mastering_sid"),
            ("idx_ring_layers_toolstamps_trgm", "toolstamps"),
            ("idx_ring_layers_mould_sids_trgm", "mould_sids"),
            (
                "idx_ring_layers_additional_moulds_trgm",
                "additional_moulds",
            ),
        ] {
            assert!(ring_field_indexes.contains(index), "missing {index}");
            assert!(
                ring_field_indexes.contains(&format!(
                    "public.ringcode_field_search_text({column}) gin_trgm_ops"
                )),
                "missing index expression for {column}"
            );
        }
        assert!(ring_field_indexes.contains("DROP INDEX idx_ring_layers_search_trgm"));
        assert!(ring_field_indexes
            .contains("DROP FUNCTION public.ringcode_layer_search_text(TEXT, TEXT)"));
    }

    #[tokio::test]
    #[ignore = "requires the local PostgreSQL fixture"]
    async fn local_discs_query_paths_smoke() {
        use std::sync::Arc;

        dotenvy::dotenv().ok();
        let config = crate::config::Config::from_env();
        crate::config::init_site_config(&config);
        let pool = crate::db::create_pool(&config.database_url).await.unwrap();
        let state = AppState {
            pool,
            config: Arc::new(config),
            http: reqwest::Client::new(),
            edition_suggestions: crate::services::disc_service::EditionSuggestionsCache::new(
                Duration::from_secs(60),
            ),
            news_cache: crate::services::news_service::NewsCache::new(Duration::from_secs(60)),
            homepage_cache: crate::routes::main_page::HomepageCache::new(Duration::from_secs(60)),
            discs_cache: DiscsCache::new(
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
        };

        for query in [
            DiscsQuery::default(),
            DiscsQuery {
                sort: Some("region".to_string()),
                ..Default::default()
            },
            DiscsQuery {
                sort: Some("language".to_string()),
                ..Default::default()
            },
            DiscsQuery {
                sort: Some("modified".to_string()),
                order: Some("desc".to_string()),
                ..Default::default()
            },
            DiscsQuery {
                tracks_min: Some("10".to_string()),
                tracks_max: Some("10".to_string()),
                ..Default::default()
            },
            DiscsQuery {
                serial: Some("SPI 061".to_string()),
                ..Default::default()
            },
            DiscsQuery {
                mastering_sid: Some("IFPI L557".to_string()),
                ..Default::default()
            },
            DiscsQuery {
                contents: Some("game".to_string()),
                ..Default::default()
            },
            DiscsQuery {
                q: Some("91cad2df".to_string()),
                ..Default::default()
            },
        ] {
            let response =
                discs_page(State(state.clone()), CurrentUser(None), MultiQuery(query))
                .await
                .unwrap();
            assert!(
                matches!(response.status(), StatusCode::OK | StatusCode::SEE_OTHER),
                "unexpected status {}",
                response.status()
            );
        }

        let response = disc_dumpers_directory(State(state.clone()), HeaderMap::new())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=3600"
        );
        let etag = response.headers()[header::ETAG].clone();
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag);
        let not_modified = disc_dumpers_directory(State(state), headers).await.unwrap();
        assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    }
}
