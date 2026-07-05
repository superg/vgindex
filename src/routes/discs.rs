use askama::Template;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
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
use crate::db::models::{format_display_title, DiscStatus};
use crate::error::AppResult;
use crate::services::disc_service;
use crate::AppState;

use super::compact_query_url;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/discs", get(discs_page))
        .route("/api/disc-dumpers", get(disc_dumpers_directory))
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
    pub letter: Option<String>,
    pub status: Option<String>,
    pub q: Option<String>,
    pub title: Option<String>,
    pub title_exact: Option<String>,
    pub title_foreign: Option<String>,
    pub title_foreign_exact: Option<String>,
    pub serial: Option<String>,
    pub serial_exact: Option<String>,
    pub edition: Option<String>,
    pub edition_exact: Option<String>,
    pub barcode: Option<String>,
    pub barcode_exact: Option<String>,
    pub tracks_min: Option<String>,
    pub tracks_max: Option<String>,
    pub errors_min: Option<String>,
    pub errors_max: Option<String>,
    pub edc: Option<String>,
    pub protection: Option<String>,
    pub comments: Option<String>,
    pub contents: Option<String>,
    pub ringcode: Option<String>,
    pub offset: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
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
    let mut by_field: HashMap<HashField, Vec<String>> = HashMap::new();
    for term in terms {
        if let Some(field) = hash_field_for_term(term) {
            let values = by_field.entry(field).or_default();
            if !values.contains(term) {
                values.push(term.clone());
            }
        }
    }
    if by_field.is_empty() {
        return Ok(HashMap::new());
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

    let rows: Vec<HashCandidateRow> = builder.build_query_as().fetch_all(pool).await?;
    let mut candidates = HashMap::new();
    for row in rows {
        candidates
            .entry(row.term)
            .or_insert_with(Vec::new)
            .push(row.disc_id);
    }
    Ok(candidates)
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

fn quick_search_clause(bind_idx: u32, hash_candidate_bind_idx: Option<u32>) -> String {
    let bind = format!("${bind_idx}");
    let mut clause = format!(
        r#"(LOWER(d.title) LIKE '%' || {bind} || '%'
             OR LOWER(d.title_foreign) LIKE '%' || {bind} || '%'
             OR compact_disc_array_search(d.serial) LIKE '%' || {bind} || '%'"#
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

fn exact_filter_enabled(active_value: Option<&String>, exact: Option<&str>) -> bool {
    active_value.is_some() && exact == Some("1")
}

fn scalar_text_bind_value(value: &str, exact: bool) -> String {
    if exact {
        value.to_string()
    } else {
        value.trim().to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitleTextField {
    Title,
    ForeignTitle,
}

fn title_text_search_clause(field: TitleTextField, bind_idx: u32, exact: bool) -> String {
    let (column, capability) = match field {
        TitleTextField::Title => ("title", None),
        TitleTextField::ForeignTitle => ("title_foreign", Some("has_title_foreign")),
    };
    let predicate = if exact {
        format!("d.{column} = ${bind_idx}")
    } else {
        format!("LOWER(d.{column}) LIKE '%' || LOWER(${bind_idx}) || '%'")
    };

    match capability {
        Some(capability) => format!("(s.{capability} AND {predicate})"),
        None => predicate,
    }
}

fn add_title_text_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    filters: [(TitleTextField, bool, bool); 2],
) {
    for (field, active, exact) in filters {
        if active {
            *bind_idx += 1;
            where_clauses.push(title_text_search_clause(field, *bind_idx, exact));
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
        format!(
            "(s.{capability} AND compact_disc_array_search(d.{column}) LIKE '%' || ${bind_idx} || '%' AND EXISTS (SELECT 1 FROM unnest(d.{column}) AS filter_value(value) WHERE LOWER(REGEXP_REPLACE(filter_value.value, '[[:space:]]+', '', 'g')) LIKE '%' || ${bind_idx} || '%'))"
        )
    }
}

fn add_array_text_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    filters: [(ArrayTextField, bool, bool); 3],
) {
    for (field, active, exact) in filters {
        if active {
            *bind_idx += 1;
            where_clauses.push(array_text_search_clause(field, *bind_idx, exact));
        }
    }
}

fn comments_search_clause(bind_idx: u32) -> String {
    format!("LOWER(d.comments) LIKE '%' || LOWER(${bind_idx}) || '%'")
}

fn contents_search_clause(bind_idx: u32) -> String {
    format!("LOWER(d.contents) LIKE '%' || LOWER(${bind_idx}) || '%'")
}

fn ringcode_search_clause(bind_idx: u32) -> String {
    format!(
        "EXISTS (SELECT 1 FROM disc_ring_code_entries ring_entry JOIN disc_ring_code_layers ring_layer ON ring_layer.entry_id = ring_entry.id WHERE ring_entry.disc_id = d.id AND ringcode_layer_search_text(ring_layer.mastering_code, ring_layer.mastering_sid) LIKE '%' || LOWER(${bind_idx}) || '%' AND (LOWER(REGEXP_REPLACE(COALESCE(ring_layer.mastering_code, ''), '[[:blank:]]{{2,}}', CHR(9), 'g')) LIKE '%' || LOWER(${bind_idx}) || '%' OR LOWER(REGEXP_REPLACE(COALESCE(ring_layer.mastering_sid, ''), '[[:blank:]]{{2,}}', CHR(9), 'g')) LIKE '%' || LOWER(${bind_idx}) || '%'))"
    )
}

fn normalize_offset_filter(value: Option<&str>) -> Option<i32> {
    crate::services::validation::validate_signed_int(value?).ok()
}

fn offset_search_clause(bind_idx: u32) -> String {
    format!(
        "EXISTS (SELECT 1 FROM disc_ring_code_entries offset_entry WHERE offset_entry.disc_id = d.id AND (offset_entry.offset_value = ${bind_idx} OR (s.has_offset_extra AND offset_entry.offset_extra_value = ${bind_idx})))"
    )
}

fn add_ring_filter_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    ringcode_active: bool,
    offset_active: bool,
) {
    if ringcode_active {
        *bind_idx += 1;
        where_clauses.push(ringcode_search_clause(*bind_idx));
    }
    if offset_active {
        *bind_idx += 1;
        where_clauses.push(offset_search_clause(*bind_idx));
    }
}

fn protection_search_clause(bind_idx: u32, is_logged_in: bool) -> String {
    let visibility = if is_logged_in {
        "s.has_protection"
    } else {
        "s.has_protection AND s.code NOT IN ('BD-VIDEO', 'HDDVD-VIDEO')"
    };

    format!("({visibility} AND LOWER(d.protection) LIKE '%' || LOWER(${bind_idx}) || '%')")
}

fn display_title_sort_sql() -> &'static str {
    "d.display_title_sort_key"
}

fn disc_order_by_sql(sort_column: &str, sort_expression: &str, sort_direction: &str) -> String {
    let nulls_clause = match sort_column {
        "added" | "modified" => " NULLS LAST",
        _ => "",
    };
    let secondary_sort = if sort_column == "title" {
        String::new()
    } else {
        format!(", {} {sort_direction}", display_title_sort_sql())
    };

    format!(
        "{sort_expression} {sort_direction}{nulls_clause}{secondary_sort}, d.id {sort_direction}"
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
    systems_media_json: String,
    letters: Vec<(String, bool)>,
    filter_system: String,
    filter_region: String,
    filter_language: String,
    filter_media: String,
    filter_category: String,
    filter_status: String,
    filter_letter: String,
    filter_q: String,
    filter_title: String,
    title_exact: bool,
    filter_title_foreign: String,
    title_foreign_exact: bool,
    filter_serial: String,
    serial_exact: bool,
    filter_edition: String,
    edition_exact: bool,
    filter_barcode: String,
    barcode_exact: bool,
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
    filter_ringcode: String,
    filter_offset: String,
    advanced_open: bool,
    advanced_explicit: bool,
    filter_dumper: String,
    filter_dumper_name: String,
    total_count: i64,
    page: i64,
    total_pages: i64,
    prev_page: i64,
    next_page: i64,
    sort_column: String,
    sort_order: String,
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
    fn url(&self, page: i64, letter: &str, sort: &str, order: &str) -> String {
        build_discs_url(DiscsUrlOptions {
            system: &self.filter_system,
            region: &self.filter_region,
            language: &self.filter_language,
            media: &self.filter_media,
            category: &self.filter_category,
            status: &self.filter_status,
            letter,
            dumper: &self.filter_dumper,
            title: &self.filter_title,
            title_exact: self.title_exact,
            title_foreign: &self.filter_title_foreign,
            title_foreign_exact: self.title_foreign_exact,
            serial: &self.filter_serial,
            serial_exact: self.serial_exact,
            edition: &self.filter_edition,
            edition_exact: self.edition_exact,
            barcode: &self.filter_barcode,
            barcode_exact: self.barcode_exact,
            tracks_min: &self.filter_tracks_min,
            tracks_max: &self.filter_tracks_max,
            errors_min: &self.filter_errors_min,
            errors_max: &self.filter_errors_max,
            edc: &self.filter_edc,
            protection: &self.filter_protection,
            comments: &self.filter_comments,
            contents: &self.filter_contents,
            ringcode: &self.filter_ringcode,
            offset: &self.filter_offset,
            q: &self.filter_q,
            sort,
            order,
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
        )
    }

    fn letter_url(&self, letter: &str) -> String {
        self.url(1, letter, &self.sort_column, &self.sort_order)
    }

    fn sort_url(&self, sort: &str, order: &str) -> String {
        self.url(1, &self.filter_letter, sort, order)
    }
}

#[derive(Clone, Copy, Default)]
struct DiscsUrlOptions<'a> {
    system: &'a str,
    region: &'a str,
    language: &'a str,
    media: &'a str,
    category: &'a str,
    status: &'a str,
    letter: &'a str,
    dumper: &'a str,
    title: &'a str,
    title_exact: bool,
    title_foreign: &'a str,
    title_foreign_exact: bool,
    serial: &'a str,
    serial_exact: bool,
    edition: &'a str,
    edition_exact: bool,
    barcode: &'a str,
    barcode_exact: bool,
    tracks_min: &'a str,
    tracks_max: &'a str,
    errors_min: &'a str,
    errors_max: &'a str,
    edc: &'a str,
    protection: &'a str,
    comments: &'a str,
    contents: &'a str,
    ringcode: &'a str,
    offset: &'a str,
    q: &'a str,
    sort: &'a str,
    order: &'a str,
    page: i64,
    advanced: bool,
}

fn build_discs_count_cache_key(
    mut options: DiscsUrlOptions<'_>,
    member_protection_visibility: bool,
) -> String {
    options.sort = "title";
    options.order = "asc";
    options.page = 1;
    options.advanced = false;
    format!(
        "protection_scope={}:{}",
        if member_protection_visibility {
            "member"
        } else {
            "public"
        },
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
    let exact_param = |enabled: bool, value: &str| {
        if enabled && !value.trim().is_empty() {
            "1"
        } else {
            ""
        }
    };
    let title_exact = exact_param(options.title_exact, options.title);
    let title_foreign_exact = exact_param(options.title_foreign_exact, options.title_foreign);
    let serial_exact = exact_param(options.serial_exact, options.serial);
    let edition_exact = exact_param(options.edition_exact, options.edition);
    let barcode_exact = exact_param(options.barcode_exact, options.barcode);

    compact_query_url(
        "/discs",
        &[
            ("system", options.system),
            ("region", options.region),
            ("language", options.language),
            ("media", options.media),
            ("category", options.category),
            ("status", options.status),
            ("letter", options.letter),
            ("dumper", options.dumper),
            ("title", options.title),
            ("title_exact", title_exact),
            ("title_foreign", options.title_foreign),
            ("title_foreign_exact", title_foreign_exact),
            ("serial", options.serial),
            ("serial_exact", serial_exact),
            ("edition", options.edition),
            ("edition_exact", edition_exact),
            ("barcode", options.barcode),
            ("barcode_exact", barcode_exact),
            ("tracks_min", options.tracks_min),
            ("tracks_max", options.tracks_max),
            ("errors_min", options.errors_min),
            ("errors_max", options.errors_max),
            ("edc", options.edc),
            ("protection", options.protection),
            ("comments", options.comments),
            ("contents", options.contents),
            ("ringcode", options.ringcode),
            ("offset", options.offset),
            ("q", options.q),
            ("sort", sort),
            ("order", order),
            ("page", &page),
            ("advanced", advanced),
        ],
    )
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

    *bind_idx += 1;
    where_clauses.push(format!(
        "EXISTS (SELECT 1 FROM disc_languages dl_filter WHERE dl_filter.disc_id = d.id AND dl_filter.language_code = ${})",
        *bind_idx
    ));
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

fn add_error_count_clauses(
    where_clauses: &mut Vec<String>,
    bind_idx: &mut u32,
    errors_min: Option<i32>,
    errors_max: Option<i32>,
) {
    if errors_min.is_some() {
        *bind_idx += 1;
        where_clauses.push(format!("d.error_count >= ${}", *bind_idx));
    }
    if errors_max.is_some() {
        *bind_idx += 1;
        where_clauses.push(format!("d.error_count <= ${}", *bind_idx));
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

async fn discs_page(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(query): Query<DiscsQuery>,
) -> AppResult<Response> {
    let page = query.page.unwrap_or(1).max(1);
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
    let filter_status = normalize_status_filter(query.status.as_deref(), can_view_disabled_discs);
    let filter_letter = normalize_letter_filter(query.letter.as_deref());
    let filter_q = query.q.clone().unwrap_or_default().trim().to_string();
    let quick_search_terms = quick_search_terms(&filter_q);
    let hash_candidates = load_hash_candidates(&state.pool, &quick_search_terms).await?;
    let active_title = active_verbatim_filter(query.title.as_ref());
    let title_exact = exact_filter_enabled(active_title.as_ref(), query.title_exact.as_deref());
    let title_bind = active_title
        .as_deref()
        .map(|value| scalar_text_bind_value(value, title_exact));
    let filter_title = title_bind.clone().unwrap_or_default();
    let active_title_foreign = active_verbatim_filter(query.title_foreign.as_ref());
    let title_foreign_exact = exact_filter_enabled(
        active_title_foreign.as_ref(),
        query.title_foreign_exact.as_deref(),
    );
    let title_foreign_bind = active_title_foreign
        .as_deref()
        .map(|value| scalar_text_bind_value(value, title_foreign_exact));
    let filter_title_foreign = title_foreign_bind.clone().unwrap_or_default();
    let active_serial = active_verbatim_filter(query.serial.as_ref());
    let filter_serial = active_serial.clone().unwrap_or_default();
    let serial_exact = exact_filter_enabled(active_serial.as_ref(), query.serial_exact.as_deref());
    let serial_bind = active_serial
        .as_deref()
        .map(|value| array_text_bind_value(value, serial_exact));
    let active_edition = active_verbatim_filter(query.edition.as_ref());
    let filter_edition = active_edition.clone().unwrap_or_default();
    let edition_exact =
        exact_filter_enabled(active_edition.as_ref(), query.edition_exact.as_deref());
    let edition_bind = active_edition
        .as_deref()
        .map(|value| array_text_bind_value(value, edition_exact));
    let active_barcode = active_verbatim_filter(query.barcode.as_ref());
    let filter_barcode = active_barcode.clone().unwrap_or_default();
    let barcode_exact =
        exact_filter_enabled(active_barcode.as_ref(), query.barcode_exact.as_deref());
    let barcode_bind = active_barcode
        .as_deref()
        .map(|value| array_text_bind_value(value, barcode_exact));
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
    let active_ringcode = active_advanced_filter(query.ringcode.as_ref());
    let filter_ringcode = active_ringcode.clone().unwrap_or_default();
    let ringcode_bind = active_ringcode
        .as_deref()
        .map(disc_service::normalize_ringcode_whitespace);
    let filter_offset_value = normalize_offset_filter(query.offset.as_deref());
    let filter_offset = filter_offset_value
        .map(|value| value.to_string())
        .unwrap_or_default();
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
    add_title_text_clauses(
        &mut where_clauses,
        &mut bind_idx,
        [
            (TitleTextField::Title, active_title.is_some(), title_exact),
            (
                TitleTextField::ForeignTitle,
                active_title_foreign.is_some(),
                title_foreign_exact,
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
                serial_exact,
            ),
            (
                ArrayTextField::Edition,
                active_edition.is_some(),
                edition_exact,
            ),
            (
                ArrayTextField::Barcode,
                active_barcode.is_some(),
                barcode_exact,
            ),
        ],
    );
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
        active_ringcode.is_some(),
        filter_offset_value.is_some(),
    );
    if filter_dumper_unknown {
        where_clauses.push("FALSE".to_string());
    } else if filter_dumper_id.is_some() {
        bind_idx += 1;
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM disc_dumpers dd2 WHERE dd2.disc_id = d.id AND dd2.user_id = ${bind_idx})"
        ));
    }

    let where_sql = where_clauses.join(" AND ");
    let current_user_id = user.user().map(|u| u.id);
    let dumped_by_me_sql = if current_user_id.is_some() {
        bind_idx += 1;
        format!(
            "EXISTS (SELECT 1 FROM disc_dumpers dd_self WHERE dd_self.disc_id = d.id AND dd_self.user_id = ${bind_idx})"
        )
    } else {
        "FALSE".to_string()
    };

    let sort_column = normalize_disc_sort(query.sort.as_deref());
    let sort_order_str = normalize_sort_order(query.order.as_deref());

    let mut sort_cte = String::new();
    let mut sort_join = "";
    let sort_col = match sort_column.as_str() {
        "region" => {
            sort_cte = "WITH region_sort AS MATERIALIZED (
                SELECT dr.disc_id, MIN(r.sort_order) AS sort_value
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
                SELECT dl.disc_id, MIN(l.sort_order) AS sort_value
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
        "added" => {
            sort_cte = "WITH added_sort AS MATERIALIZED (
                SELECT target_disc_id AS disc_id, MIN(created_at) AS sort_value
                FROM disc_submissions
                WHERE target_disc_id IS NOT NULL
                GROUP BY target_disc_id
            )"
            .to_string();
            sort_join = " LEFT JOIN added_sort ON added_sort.disc_id = d.id";
            "added_sort.sort_value"
        }
        "modified" => {
            sort_cte = "WITH first_public_submission AS NOT MATERIALIZED (
                SELECT target_disc_id AS disc_id, MIN(id) AS first_id
                FROM disc_submissions
                WHERE target_disc_id IS NOT NULL
                  AND status IN ('Approved', 'Legacy')
                GROUP BY target_disc_id
            ), modified_sort AS MATERIALIZED (
                SELECT ds.target_disc_id AS disc_id,
                       MAX(COALESCE(ds.reviewed_at, ds.created_at)) AS sort_value
                FROM disc_submissions ds
                JOIN first_public_submission first_submission
                  ON first_submission.disc_id = ds.target_disc_id
                WHERE ds.status IN ('Approved', 'Legacy')
                  AND (
                    (
                      ds.submission_type = 'Edit'
                      AND (
                        ds.changes <> '{}'::jsonb
                        OR COALESCE(ds.review_comment, '') <> ALL (
                            ARRAY['added-backfill', 'no-added-sentinel']::TEXT[]
                        )
                      )
                    )
                    OR ds.submission_type = 'Disc'
                  )
                  AND (
                    ds.submission_type <> 'Disc'
                    OR ds.id <> first_submission.first_id
                  )
                GROUP BY ds.target_disc_id
            )"
            .to_string();
            sort_join = " LEFT JOIN modified_sort ON modified_sort.disc_id = d.id";
            "modified_sort.sort_value"
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
    let count_key_serial = serial_bind.clone().unwrap_or_default();
    let count_key_edition = edition_bind.clone().unwrap_or_default();
    let count_key_barcode = barcode_bind.clone().unwrap_or_default();
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
    let count_key_ringcode = ringcode_bind
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
            status: &filter_status,
            letter: &filter_letter,
            dumper: &filter_dumper,
            title: &count_key_title,
            title_exact,
            title_foreign: &count_key_title_foreign,
            title_foreign_exact,
            serial: &count_key_serial,
            serial_exact,
            edition: &count_key_edition,
            edition_exact,
            barcode: &count_key_barcode,
            barcode_exact,
            tracks_min: &filter_tracks_min,
            tracks_max: &filter_tracks_max,
            errors_min: &filter_errors_min,
            errors_max: &filter_errors_max,
            edc: &filter_edc,
            protection: &count_key_protection,
            comments: &count_key_comments,
            contents: &count_key_contents,
            ringcode: &count_key_ringcode,
            offset: &filter_offset,
            q: &count_key_q,
            sort: &sort_column,
            order: &sort_order_str,
            page,
            advanced: advanced_explicit,
        },
        active_protection.is_some() && user.is_logged_in(),
    );
    let mut cached_total_count = state.discs_cache.counts.get(&count_cache_key).await;
    let count_load_lock = if cached_total_count.is_none() {
        Some(state.discs_cache.counts.load_lock(&count_cache_key).await)
    } else {
        None
    };
    let _count_load_guard = match &count_load_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    if cached_total_count.is_none() {
        cached_total_count = state.discs_cache.counts.get(&count_cache_key).await;
    }

    let count_requires_system_join = active_title_foreign.is_some()
        || active_serial.is_some()
        || active_edition.is_some()
        || active_barcode.is_some()
        || !filter_edc.is_empty()
        || active_protection.is_some()
        || filter_offset_value.is_some();
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
         ORDER BY {order_by} LIMIT {PAGE_SIZE} OFFSET {offset}"
    );

    let mut count_query = cached_total_count
        .is_none()
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
    if !filter_region.is_empty() {
        bind_queries!(filter_region.clone());
    }
    if !filter_language.is_empty() {
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
    if let Some(serial) = &serial_bind {
        bind_queries!(serial.clone());
    }
    if let Some(edition) = &edition_bind {
        bind_queries!(edition.clone());
    }
    if let Some(barcode) = &barcode_bind {
        bind_queries!(barcode.clone());
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
    if let Some(ringcode) = &ringcode_bind {
        bind_queries!(ringcode.clone());
    }
    if let Some(offset) = filter_offset_value {
        bind_queries!(offset);
    }
    if let Some(dumper_id) = filter_dumper_id {
        bind_queries!(dumper_id);
    }
    if let Some(current_user_id) = current_user_id {
        select_query = select_query.bind(current_user_id);
    }

    let total_count = if let Some(count_query) = count_query {
        let total_count = count_query.fetch_one(&state.pool).await?;
        state
            .discs_cache
            .counts
            .insert(count_cache_key, total_count)
            .await;
        total_count
    } else {
        cached_total_count.unwrap()
    };
    let total_pages = (total_count + PAGE_SIZE - 1) / PAGE_SIZE;

    let raw_rows: Vec<RawDiscRow> = select_query.fetch_all(&state.pool).await?;

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
            filter_status,
            filter_letter,
            filter_q,
            filter_title,
            title_exact,
            filter_title_foreign,
            title_foreign_exact,
            filter_serial,
            serial_exact,
            filter_edition,
            edition_exact,
            filter_barcode,
            barcode_exact,
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
            filter_ringcode,
            filter_offset,
            advanced_open,
            advanced_explicit,
            filter_dumper,
            filter_dumper_name,
            total_count,
            page,
            total_pages,
            prev_page: page - 1,
            next_page: page + 1,
            sort_column: sort_column.clone(),
            sort_order: sort_order_str,
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
    let regions =
        sqlx::query_as("SELECT code, name FROM regions ORDER BY LOWER(name)").fetch_all(pool);
    let languages = sqlx::query_as(
        "SELECT TRIM(code) AS code, name FROM languages ORDER BY sort_order, LOWER(name)",
    )
    .fetch_all(pool);

    let (systems, media, categories, regions, languages) =
        tokio::try_join!(systems, media, categories, regions, languages)?;

    Ok(DiscReferenceData {
        systems,
        media,
        categories,
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
                "d.error_count >= $7".to_string(),
                "d.error_count <= $8".to_string(),
            ]
        );
        assert_eq!(bind_idx, 8);
    }

    #[test]
    fn error_count_clauses_support_open_bounded_exact_and_reversed_ranges() {
        let cases = [
            (None, None, Vec::<&str>::new()),
            (Some(3), None, vec!["d.error_count >= $5"]),
            (None, Some(9), vec!["d.error_count <= $5"]),
            (
                Some(3),
                Some(9),
                vec!["d.error_count >= $5", "d.error_count <= $6"],
            ),
            (
                Some(7),
                Some(7),
                vec!["d.error_count >= $5", "d.error_count <= $6"],
            ),
            (
                Some(9),
                Some(3),
                vec!["d.error_count >= $5", "d.error_count <= $6"],
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
        let tracks = template.find("<span>Tracks</span>").unwrap();
        let errors = template.find("<span>Errors</span>").unwrap();
        let edc = template.find("id=\"edc-filter-label\">EDC</span>").unwrap();
        let protection = template.find("<span>Protection</span>").unwrap();
        let comments = template.find("<span>Comments</span>").unwrap();
        let contents = template.find("<span>Contents</span>").unwrap();
        let ringcode = template.find("<span>Ringcode</span>").unwrap();
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
                && errors < edc
                && edc < protection
                && protection < comments
                && comments < contents
                && contents < ringcode
                && ringcode < offset
        );
        assert!(template.contains("<option value=\"\">All Languages</option>"));
        assert!(template.contains("<option value=\"\">All Media</option>"));
        assert!(template.contains("<option value=\"\">All Categories</option>"));
        assert_eq!(template.matches("name=\"language\"").count(), 3);
        assert_eq!(template.matches("name=\"media\"").count(), 3);
        assert_eq!(template.matches("name=\"category\"").count(), 3);
        assert_eq!(template.matches("name=\"title\"").count(), 3);
        assert_eq!(template.matches("name=\"title_exact\"").count(), 3);
        assert_eq!(template.matches("name=\"title_foreign\"").count(), 3);
        assert_eq!(template.matches("name=\"title_foreign_exact\"").count(), 3);
        assert_eq!(template.matches("name=\"serial\"").count(), 3);
        assert_eq!(template.matches("name=\"serial_exact\"").count(), 3);
        assert_eq!(template.matches("name=\"edition\"").count(), 3);
        assert_eq!(template.matches("name=\"edition_exact\"").count(), 3);
        assert_eq!(template.matches("name=\"barcode\"").count(), 3);
        assert_eq!(template.matches("name=\"barcode_exact\"").count(), 3);
        assert_eq!(template.matches("name=\"tracks_min\"").count(), 3);
        assert_eq!(template.matches("name=\"tracks_max\"").count(), 4);
        assert_eq!(template.matches("name=\"errors_min\"").count(), 3);
        assert_eq!(template.matches("name=\"errors_max\"").count(), 4);
        assert_eq!(template.matches("name=\"edc\"").count(), 3);
        assert_eq!(template.matches("name=\"protection\"").count(), 3);
        assert_eq!(template.matches("name=\"comments\"").count(), 3);
        assert_eq!(template.matches("name=\"contents\"").count(), 3);
        assert_eq!(template.matches("name=\"ringcode\"").count(), 3);
        assert_eq!(template.matches("name=\"offset\"").count(), 3);
        assert!(!template.contains("edition_q"));
        assert!(!template.contains("comments_q"));
        assert!(template.contains("filterMediaOptions(true)"));
        assert!(template.contains("allowed.indexOf(current) === -1"));
    }

    #[test]
    fn exact_track_and_error_controls_mirror_independently() {
        let template = include_str!("../../templates/discs.html");

        for prefix in ["tracks", "errors"] {
            assert!(template.contains(&format!("id=\"{prefix}-min\"")));
            assert!(template.contains(&format!("id=\"{prefix}-max\"")));
            assert!(template.contains(&format!("id=\"{prefix}-max-mirror\"")));
            assert!(template.contains(&format!("id=\"{prefix}-exact\"")));
            assert!(template.contains(&format!("initExactRange('{prefix}')")));
        }
        assert_eq!(template.matches("<span>Exact</span>").count(), 7);
        assert!(template.contains("{% if tracks_exact %}checked{% endif %}"));
        assert!(template.contains("{% if errors_exact %}checked{% endif %}"));
        assert!(template.contains("max.value = min.value"));
        assert!(template.contains("maxMirror.value = min.value"));
        assert!(template.contains("max.disabled = true"));
        assert!(template.contains("maxMirror.disabled = false"));
    }

    #[test]
    fn array_text_filters_render_with_dependent_exact_controls() {
        let template = include_str!("../../templates/discs.html");

        for (field, id) in [
            ("title", "title"),
            ("title_foreign", "title-foreign"),
            ("serial", "serial"),
            ("edition", "edition"),
            ("barcode", "barcode"),
        ] {
            assert!(template.contains(&format!("id=\"{id}-filter\" name=\"{field}\"")));
            assert!(template.contains(&format!(
                "id=\"{id}-exact\" name=\"{field}_exact\" value=\"1\" data-requires-nonblank=\"{field}\""
            )));
            assert!(template.contains(&format!("{{% if {field}_exact %}}checked{{% endif %}}")));
        }
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
    fn advanced_sort_controls_cover_all_fields_and_independent_directions() {
        let template = include_str!("../../templates/discs.html");

        for sort in [
            "title", "region", "system", "version", "edition", "language", "serial", "status",
            "added", "modified",
        ] {
            assert!(
                template.contains(&format!("<option value=\"{sort}\"")),
                "missing advanced sort option {sort}"
            );
        }

        assert!(!template.contains("title|asc"));
        assert!(!template.contains("added|desc"));
        assert!(template.contains("name=\"sort\""));
        assert!(template.contains("aria-label=\"Sort direction\""));
        assert!(template.contains("aria-label=\"Ascending\""));
        assert!(template.contains("aria-label=\"Descending\""));
        assert!(template.contains("this.form.order.value='asc'"));
        assert!(template.contains("this.form.order.value='desc'"));
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
                status: "Verified",
                letter: "#",
                dumper: "A/B",
                title: "Game Title!",
                title_exact: true,
                title_foreign: "Foreign Title",
                title_foreign_exact: true,
                serial: "SLUS 12345",
                serial_exact: true,
                edition: "Limited Edition",
                edition_exact: true,
                barcode: "0 12345 67890",
                barcode_exact: true,
                tracks_min: "2",
                tracks_max: "8",
                errors_min: "3",
                errors_max: "12",
                edc: "no",
                protection: "SecuROM 7+",
                comments: "Disc & manual",
                contents: "Game data & extras",
                ringcode: "MASTER  L0",
                offset: "123",
                q: "Game Name",
                sort: "status",
                order: "desc",
                page: 2,
                advanced: true,
            }),
            "/discs?system=PS2&region=us&language=en&media=dvd9&category=Bonus%20Discs&status=Verified&letter=%23&dumper=A%2FB&title=Game%20Title%21&title_exact=1&title_foreign=Foreign%20Title&title_foreign_exact=1&serial=SLUS%2012345&serial_exact=1&edition=Limited%20Edition&edition_exact=1&barcode=0%2012345%2067890&barcode_exact=1&tracks_min=2&tracks_max=8&errors_min=3&errors_max=12&edc=no&protection=SecuROM%207%2B&comments=Disc%20%26%20manual&contents=Game%20data%20%26%20extras&ringcode=MASTER%20%20L0&offset=123&q=Game%20Name&sort=status&order=desc&page=2&advanced=1"
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
                title_exact: true,
                title_foreign_exact: true,
                serial_exact: true,
                edition_exact: true,
                barcode_exact: true,
                sort: "title",
                order: "asc",
                page: 1,
                ..Default::default()
            }),
            "/discs"
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

        assert!(clause.contains("LOWER(d.title) LIKE"));
        assert!(clause.contains("LOWER(d.title_foreign) LIKE"));
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
    fn exact_flags_require_active_text_and_literal_one() {
        let active = Some("Value".to_string());

        assert!(exact_filter_enabled(active.as_ref(), Some("1")));
        assert!(!exact_filter_enabled(active.as_ref(), Some("true")));
        assert!(!exact_filter_enabled(active.as_ref(), Some("0")));
        assert!(!exact_filter_enabled(active.as_ref(), None));
        assert!(!exact_filter_enabled(None, Some("1")));
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
                    "(s.{capability} AND compact_disc_array_search(d.{column}) LIKE '%' || ${bind_idx} || '%' AND EXISTS (SELECT 1 FROM unnest(d.{column}) AS filter_value(value) WHERE LOWER(REGEXP_REPLACE(filter_value.value, '[[:space:]]+', '', 'g')) LIKE '%' || ${bind_idx} || '%'))"
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
    fn title_text_clauses_use_base_columns_and_gate_foreign_titles() {
        assert_eq!(
            title_text_search_clause(TitleTextField::Title, 5, false),
            "LOWER(d.title) LIKE '%' || LOWER($5) || '%'"
        );
        assert_eq!(
            title_text_search_clause(TitleTextField::Title, 5, true),
            "d.title = $5"
        );
        assert_eq!(
            title_text_search_clause(TitleTextField::ForeignTitle, 6, false),
            "(s.has_title_foreign AND LOWER(d.title_foreign) LIKE '%' || LOWER($6) || '%')"
        );
        assert_eq!(
            title_text_search_clause(TitleTextField::ForeignTitle, 6, true),
            "(s.has_title_foreign AND d.title_foreign = $6)"
        );
    }

    #[test]
    fn title_filters_precede_array_text_filters_in_binding_order() {
        let mut clauses = Vec::new();
        let mut bind_idx = 4;

        add_title_text_clauses(
            &mut clauses,
            &mut bind_idx,
            [
                (TitleTextField::Title, true, false),
                (TitleTextField::ForeignTitle, true, true),
            ],
        );
        add_array_text_clauses(
            &mut clauses,
            &mut bind_idx,
            [
                (ArrayTextField::Serial, true, false),
                (ArrayTextField::Edition, false, false),
                (ArrayTextField::Barcode, false, false),
            ],
        );

        assert_eq!(bind_idx, 7);
        assert_eq!(clauses[0], "LOWER(d.title) LIKE '%' || LOWER($5) || '%'");
        assert_eq!(clauses[1], "(s.has_title_foreign AND d.title_foreign = $6)");
        assert!(clauses[2].contains("unnest(d.serial)"));
        assert!(clauses[2].contains("$7"));
    }

    #[test]
    fn array_text_clauses_keep_serial_edition_barcode_binding_order() {
        let mut clauses = Vec::new();
        let mut bind_idx = 4;

        add_array_text_clauses(
            &mut clauses,
            &mut bind_idx,
            [
                (ArrayTextField::Serial, true, false),
                (ArrayTextField::Edition, true, true),
                (ArrayTextField::Barcode, true, false),
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
            "LOWER(d.comments) LIKE '%' || LOWER($7) || '%'"
        );
        assert_eq!(
            contents_clause,
            "LOWER(d.contents) LIKE '%' || LOWER($8) || '%'"
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
    fn ringcode_clause_searches_both_mastering_fields_on_any_layer() {
        let clause = ringcode_search_clause(8);

        assert!(clause.contains("FROM disc_ring_code_entries ring_entry"));
        assert!(clause.contains("JOIN disc_ring_code_layers ring_layer"));
        assert!(clause.contains("ring_layer.entry_id = ring_entry.id"));
        assert!(clause.contains("ring_entry.disc_id = d.id"));
        assert!(clause.contains("ring_layer.mastering_code"));
        assert!(clause.contains("ring_layer.mastering_sid"));
        assert_eq!(clause.matches("LOWER($8)").count(), 3);
        assert!(clause.contains("ringcode_layer_search_text"));
        assert_eq!(clause.matches("'[[:blank:]]{2,}'").count(), 2);
        assert_eq!(clause.matches("COALESCE(").count(), 2);
    }

    #[test]
    fn offset_clause_matches_primary_or_applicable_extra_offset_exactly() {
        assert_eq!(
            offset_search_clause(9),
            "EXISTS (SELECT 1 FROM disc_ring_code_entries offset_entry WHERE offset_entry.disc_id = d.id AND (offset_entry.offset_value = $9 OR (s.has_offset_extra AND offset_entry.offset_extra_value = $9)))"
        );
    }

    #[test]
    fn contents_ringcode_and_offset_follow_comments_in_binding_order() {
        let mut clauses = vec![comments_search_clause(7), contents_search_clause(8)];
        let mut bind_idx = 8;

        add_ring_filter_clauses(&mut clauses, &mut bind_idx, true, true);

        assert_eq!(bind_idx, 10);
        assert_eq!(clauses.len(), 4);
        assert!(clauses[1].contains("LOWER($8)"));
        assert!(clauses[2].contains("LOWER($9)"));
        assert!(clauses[3].contains("offset_entry.offset_value = $10"));
    }

    #[test]
    fn protection_search_respects_system_and_guest_visibility() {
        assert_eq!(
            protection_search_clause(6, true),
            "(s.has_protection AND LOWER(d.protection) LIKE '%' || LOWER($6) || '%')"
        );
        assert_eq!(
            protection_search_clause(6, false),
            "(s.has_protection AND s.code NOT IN ('BD-VIDEO', 'HDDVD-VIDEO') AND LOWER(d.protection) LIKE '%' || LOWER($6) || '%')"
        );
    }

    #[test]
    fn disc_query_uses_unsuffixed_advanced_text_parameters_only() {
        let uri = "/discs?title=Game&title_exact=1&title_foreign=Foreign&title_foreign_exact=1&serial=SLUS-12345&serial_exact=1&edition=Limited&edition_exact=1&barcode=012345&barcode_exact=1&protection=SecuROM&comments=Note&contents=Bonus%20videos&ringcode=MASTER-L0&offset=%2B123&edition_q=Old&comments_q=Old"
                .parse()
                .unwrap();
        let Query(query) = Query::<DiscsQuery>::try_from_uri(&uri).unwrap();

        assert_eq!(query.title.as_deref(), Some("Game"));
        assert_eq!(query.title_exact.as_deref(), Some("1"));
        assert_eq!(query.title_foreign.as_deref(), Some("Foreign"));
        assert_eq!(query.title_foreign_exact.as_deref(), Some("1"));
        assert_eq!(query.serial.as_deref(), Some("SLUS-12345"));
        assert_eq!(query.serial_exact.as_deref(), Some("1"));
        assert_eq!(query.edition.as_deref(), Some("Limited"));
        assert_eq!(query.edition_exact.as_deref(), Some("1"));
        assert_eq!(query.barcode.as_deref(), Some("012345"));
        assert_eq!(query.barcode_exact.as_deref(), Some("1"));
        assert_eq!(query.protection.as_deref(), Some("SecuROM"));
        assert_eq!(query.comments.as_deref(), Some("Note"));
        assert_eq!(query.contents.as_deref(), Some("Bonus videos"));
        assert_eq!(query.ringcode.as_deref(), Some("MASTER-L0"));
        assert_eq!(query.offset.as_deref(), Some("+123"));

        let legacy_uri = "/discs?edition_q=Old&comments_q=Old".parse().unwrap();
        let Query(legacy_query) = Query::<DiscsQuery>::try_from_uri(&legacy_uri).unwrap();
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
        );
        let member = build_discs_count_cache_key(
            DiscsUrlOptions {
                system: "PS2",
                title: "Game",
                ..Default::default()
            },
            true,
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

        let modification_index =
            include_str!("../../migrations/013_index_disc_modification_sort.sql");
        assert!(modification_index.contains("idx_submissions_genuine_change_target_time"));
        assert!(
            modification_index.contains("INCLUDE (reviewed_at, created_at, id, submission_type)")
        );
        assert!(
            modification_index.contains("DROP INDEX idx_submissions_public_target_created_desc")
        );

        let contents_index = include_str!("../../migrations/014_index_disc_contents_search.sql");
        assert!(contents_index.contains("idx_discs_contents_trgm"));
        assert!(contents_index.contains("LOWER(contents) gin_trgm_ops"));
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
            transliteration: Arc::new(
                crate::transliteration::TransliterationRegistry::new().unwrap(),
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
                ringcode: Some("IFPI L557".to_string()),
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
            let response = discs_page(State(state.clone()), CurrentUser(None), Query(query))
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
