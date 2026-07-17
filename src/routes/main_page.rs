use askama::Template;
use axum::{extract::State, response::Html, routing::get, Router};
use sqlx::PgPool;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

use crate::auth::middleware::{AuthenticatedUser, CurrentUser};
use crate::config::SiteConfig;
use crate::db::models::format_display_title;
use crate::services::disc_service::{disc_date_sort_sql, disc_order_by_sql};
use crate::services::news_service::NewsItem;
use crate::AppState;

const HOME_RECENT_LIMIT: i64 = 30;
const HOME_NEWS_LIMIT: usize = 3;
pub const HOMEPAGE_CACHE_TTL_SECONDS: u64 = 60;

fn home_recent_discs_sql() -> String {
    home_recent_date_sql("added", "created_at")
}

fn home_recent_changes_sql() -> String {
    home_recent_date_sql("modified", "modified_at")
}

fn home_recent_date_sql(sort_column: &str, date_alias: &str) -> String {
    let date_sort = disc_date_sort_sql(sort_column).unwrap();
    let order_by = disc_order_by_sql(sort_column, date_sort.expression, "DESC");

    format!(
        "{}
         SELECT d.id, d.title, d.disc_number, d.disc_title, d.filename_suffix,
                s.has_disc_number, s.has_disc_title,
                s.code AS system_code, s.short_name AS system_short_name,
                {} AS {date_alias}
         FROM discs d
         JOIN systems s ON s.code = d.system_code{}
         WHERE d.status != 'Disabled'
         ORDER BY {order_by}
         LIMIT $1",
        date_sort.cte, date_sort.expression, date_sort.join
    )
}

fn home_display_title(
    title: &str,
    disc_number: Option<&str>,
    disc_title: Option<&str>,
    filename_suffix: Option<&str>,
    has_disc_number: bool,
    has_disc_title: bool,
) -> String {
    format_display_title(
        title,
        if has_disc_number { disc_number } else { None },
        if has_disc_title { disc_title } else { None },
        filename_suffix,
    )
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/", get(homepage))
}

#[derive(Template)]
#[template(path = "main.html")]
struct MainTemplate {
    current_user: Option<AuthenticatedUser>,
    news_items: Vec<NewsItem>,
    recent_discs: Vec<RecentDisc>,
    recent_changes: Vec<RecentChange>,
}
impl SiteConfig for MainTemplate {}

#[derive(Clone)]
pub struct HomepageCache {
    inner: Arc<RwLock<CachedHomepage>>,
    refresh: Arc<Mutex<()>>,
    ttl: Duration,
}

#[derive(Default)]
struct CachedHomepage {
    loaded_at: Option<Instant>,
    data: HomepageData,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HomepageData {
    recent_discs: Vec<RecentDisc>,
    recent_changes: Vec<RecentChange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecentDisc {
    id: i32,
    title: String,
    system: String,
    region_flags: Vec<HomeRegionFlag>,
    created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecentChange {
    id: i32,
    title: String,
    system: String,
    region_flags: Vec<HomeRegionFlag>,
    modified_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HomeRegionFlag {
    code: String,
    name: String,
}

impl HomepageCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(CachedHomepage::default())),
            refresh: Arc::new(Mutex::new(())),
            ttl,
        }
    }

    async fn get(&self, pool: &PgPool) -> Result<HomepageData, sqlx::Error> {
        self.get_with_loader(|| load_homepage_data(pool)).await
    }

    async fn get_with_loader<F, Fut, E>(&self, loader: F) -> Result<HomepageData, E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<HomepageData, E>>,
        E: std::fmt::Display,
    {
        let now = Instant::now();
        {
            let cached = self.inner.read().await;
            if cached.is_fresh(now, self.ttl) {
                return Ok(cached.data.clone());
            }
        }

        let stale = {
            let cached = self.inner.read().await;
            cached.loaded_at.map(|_| cached.data.clone())
        };

        match self.refresh.try_lock() {
            Ok(guard) => self.refresh_with_guard(guard, stale, loader).await,
            Err(_) => {
                if let Some(stale) = stale {
                    return Ok(stale);
                }

                let guard = self.refresh.lock().await;
                let cached = self.inner.read().await;
                if cached.loaded_at.is_some() {
                    return Ok(cached.data.clone());
                }
                drop(cached);

                self.refresh_with_guard(guard, None, loader).await
            }
        }
    }

    async fn refresh_with_guard<F, Fut, E>(
        &self,
        _guard: tokio::sync::MutexGuard<'_, ()>,
        stale: Option<HomepageData>,
        loader: F,
    ) -> Result<HomepageData, E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<HomepageData, E>>,
        E: std::fmt::Display,
    {
        let now = Instant::now();
        {
            let cached = self.inner.read().await;
            if cached.is_fresh(now, self.ttl) {
                return Ok(cached.data.clone());
            }
        }

        match loader().await {
            Ok(data) => {
                let mut cached = self.inner.write().await;
                cached.loaded_at = Some(Instant::now());
                cached.data = data.clone();
                Ok(data)
            }
            Err(err) => {
                if let Some(stale) = stale {
                    tracing::warn!("Failed to refresh homepage data; using stale cache: {err}");
                    Ok(stale)
                } else {
                    Err(err)
                }
            }
        }
    }
}

impl CachedHomepage {
    fn is_fresh(&self, now: Instant, ttl: Duration) -> bool {
        self.loaded_at
            .map(|loaded_at| now.duration_since(loaded_at) < ttl)
            .unwrap_or(false)
    }
}

async fn homepage(State(state): State<AppState>, user: CurrentUser) -> Html<String> {
    let mut news_items = state
        .news_cache
        .get(&state.http, &state.config.news_feed_url)
        .await
        .unwrap_or_else(|err| {
            tracing::warn!("Failed to load homepage news: {err}");
            Vec::new()
        });
    news_items.truncate(HOME_NEWS_LIMIT);

    let homepage_data = state
        .homepage_cache
        .get(&state.pool)
        .await
        .unwrap_or_else(|err| {
            tracing::warn!("Failed to load homepage data: {err}");
            HomepageData::default()
        });

    Html(
        MainTemplate {
            current_user: user.user().cloned(),
            news_items,
            recent_discs: homepage_data.recent_discs,
            recent_changes: homepage_data.recent_changes,
        }
        .render()
        .unwrap(),
    )
}

async fn load_homepage_data(pool: &PgPool) -> Result<HomepageData, sqlx::Error> {
    let total_started = Instant::now();

    let recent_discs_started = Instant::now();
    let rows: Vec<RecentDiscRow> = sqlx::query_as(&home_recent_discs_sql())
        .bind(HOME_RECENT_LIMIT)
        .fetch_all(pool)
        .await?;
    let recent_discs_elapsed = recent_discs_started.elapsed();

    let recent_changes_started = Instant::now();
    let change_rows: Vec<RecentChangeRow> = sqlx::query_as(&home_recent_changes_sql())
        .bind(HOME_RECENT_LIMIT)
        .fetch_all(pool)
        .await?;
    let recent_changes_elapsed = recent_changes_started.elapsed();

    let disc_ids = collect_home_disc_ids(&rows, &change_rows);

    let regions_started = Instant::now();
    let regions_by_disc = load_home_region_flags(pool, &disc_ids).await?;
    let regions_elapsed = regions_started.elapsed();

    let mut recent_discs = Vec::with_capacity(rows.len());
    for r in rows {
        let created_at = r
            .created_at
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        recent_discs.push(RecentDisc {
            id: r.id,
            title: home_display_title(
                &r.title,
                r.disc_number.as_deref(),
                r.disc_title.as_deref(),
                r.filename_suffix.as_deref(),
                r.has_disc_number,
                r.has_disc_title,
            ),
            system: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            region_flags: regions_by_disc.get(&r.id).cloned().unwrap_or_default(),
            created_at,
        });
    }

    let mut recent_changes = Vec::with_capacity(change_rows.len());
    for r in change_rows {
        let modified_at = r
            .modified_at
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        recent_changes.push(RecentChange {
            id: r.id,
            title: home_display_title(
                &r.title,
                r.disc_number.as_deref(),
                r.disc_title.as_deref(),
                r.filename_suffix.as_deref(),
                r.has_disc_number,
                r.has_disc_title,
            ),
            system: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            region_flags: regions_by_disc.get(&r.id).cloned().unwrap_or_default(),
            modified_at,
        });
    }

    tracing::debug!(
        total_ms = total_started.elapsed().as_millis() as u64,
        recent_dumps_ms = recent_discs_elapsed.as_millis() as u64,
        recent_changes_ms = recent_changes_elapsed.as_millis() as u64,
        regions_ms = regions_elapsed.as_millis() as u64,
        recent_dumps_count = recent_discs.len(),
        recent_changes_count = recent_changes.len(),
        "Refreshed homepage recent lists"
    );

    Ok(HomepageData {
        recent_discs,
        recent_changes,
    })
}

#[derive(sqlx::FromRow)]
struct RecentDiscRow {
    id: i32,
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    has_disc_number: bool,
    has_disc_title: bool,
    system_code: String,
    system_short_name: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct RecentChangeRow {
    id: i32,
    title: String,
    disc_number: Option<String>,
    disc_title: Option<String>,
    filename_suffix: Option<String>,
    has_disc_number: bool,
    has_disc_title: bool,
    system_code: String,
    system_short_name: String,
    modified_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct HomeRegionRow {
    disc_id: i32,
    code: String,
    name: String,
}

fn collect_home_disc_ids(rows: &[RecentDiscRow], change_rows: &[RecentChangeRow]) -> Vec<i32> {
    let mut ids = BTreeSet::new();
    ids.extend(rows.iter().map(|row| row.id));
    ids.extend(change_rows.iter().map(|row| row.id));
    ids.into_iter().collect()
}

async fn load_home_region_flags(
    pool: &PgPool,
    disc_ids: &[i32],
) -> Result<BTreeMap<i32, Vec<HomeRegionFlag>>, sqlx::Error> {
    if disc_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    let region_rows: Vec<HomeRegionRow> = sqlx::query_as(
        "SELECT dr.disc_id, r.flag_code AS code, r.name FROM disc_regions dr
         JOIN regions r ON r.code = dr.region_code
         WHERE dr.disc_id = ANY($1) ORDER BY dr.disc_id, r.sort_order",
    )
    .bind(disc_ids)
    .fetch_all(pool)
    .await?;

    Ok(group_home_region_flags(region_rows))
}

fn group_home_region_flags(rows: Vec<HomeRegionRow>) -> BTreeMap<i32, Vec<HomeRegionFlag>> {
    let mut grouped = BTreeMap::new();
    for rr in rows {
        grouped
            .entry(rr.disc_id)
            .or_insert_with(Vec::new)
            .push(HomeRegionFlag {
                code: rr.code.to_lowercase(),
                name: rr.name,
            });
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn homepage_date_queries_use_disc_list_sort_fragments() {
        for (sort_column, date_alias, sql) in [
            ("added", "created_at", home_recent_discs_sql()),
            ("modified", "modified_at", home_recent_changes_sql()),
        ] {
            let date_sort = disc_date_sort_sql(sort_column).unwrap();
            let order_by = disc_order_by_sql(sort_column, date_sort.expression, "DESC");

            assert!(sql.contains(&date_sort.cte));
            assert!(sql.contains(date_sort.join.trim()));
            assert!(sql.contains(&format!("{} AS {date_alias}", date_sort.expression)));
            assert!(sql.contains(&format!("ORDER BY {order_by}")));
            assert!(sql.contains("LIMIT $1"));
        }
    }

    #[test]
    fn recent_changes_query_only_uses_public_history_rows() {
        assert!(home_recent_changes_sql().contains("ds.status IN ('Approved', 'Legacy')"));
    }

    #[test]
    fn recent_changes_query_uses_review_time_with_created_fallback() {
        let sql = home_recent_changes_sql();
        assert!(sql.contains("MAX(COALESCE(ds.reviewed_at, ds.created_at)) AS sort_value"));
        assert!(sql.contains("modified_sort.sort_value AS modified_at"));
    }

    #[test]
    fn recent_changes_query_includes_every_public_history_row() {
        let sql = home_recent_changes_sql();
        assert!(!sql.contains("submission_type"));
        assert!(!sql.contains("changes"));
        assert!(!sql.contains("review_comment"));
        assert!(!sql.contains("MIN(ds_first.id)"));
    }

    #[test]
    fn homepage_queries_select_disc_list_title_components() {
        let recent_discs_sql = home_recent_discs_sql();
        let recent_changes_sql = home_recent_changes_sql();
        for sql in [recent_discs_sql.as_str(), recent_changes_sql.as_str()] {
            for field in [
                "d.disc_number",
                "d.disc_title",
                "d.filename_suffix",
                "s.has_disc_number",
                "s.has_disc_title",
            ] {
                assert!(sql.contains(field), "missing {field}");
            }
        }
    }

    #[test]
    fn homepage_titles_match_disc_list_display_rules() {
        assert_eq!(
            home_display_title(
                "Example",
                Some("2"),
                Some("Bonus Disc"),
                Some("Rev 1"),
                true,
                true,
            ),
            "Example (Disc 2) (Bonus Disc) (Rev 1)"
        );
        assert_eq!(
            home_display_title(
                "Example",
                Some("2"),
                Some("Bonus Disc"),
                Some("Rev 1"),
                false,
                false,
            ),
            "Example (Rev 1)"
        );
    }

    fn homepage_data(title: &str) -> HomepageData {
        HomepageData {
            recent_discs: vec![RecentDisc {
                id: 1,
                title: title.to_string(),
                system: "PSX".to_string(),
                region_flags: vec![HomeRegionFlag {
                    code: "us".to_string(),
                    name: "USA".to_string(),
                }],
                created_at: "2026-01-01".to_string(),
            }],
            recent_changes: vec![RecentChange {
                id: 2,
                title: format!("{title} changed"),
                system: "PS2".to_string(),
                region_flags: vec![],
                modified_at: "2026-01-02".to_string(),
            }],
        }
    }

    #[tokio::test]
    async fn homepage_cache_returns_fresh_data_without_refreshing() {
        let cache = HomepageCache::new(Duration::from_secs(60));
        let calls = Arc::new(AtomicUsize::new(0));

        let first_calls = calls.clone();
        let first = cache
            .get_with_loader(move || {
                let calls = first_calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(homepage_data("fresh"))
                }
            })
            .await
            .unwrap();

        let second_calls = calls.clone();
        let second = cache
            .get_with_loader(move || {
                let calls = second_calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(homepage_data("unexpected"))
                }
            })
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn homepage_cache_serves_stale_data_when_refresh_fails() {
        let cache = HomepageCache::new(Duration::ZERO);

        cache
            .get_with_loader(|| async { Ok::<_, String>(homepage_data("stale")) })
            .await
            .unwrap();

        let data = cache
            .get_with_loader(|| async {
                Err::<HomepageData, _>("database unavailable".to_string())
            })
            .await
            .unwrap();

        assert_eq!(data, homepage_data("stale"));
    }

    #[tokio::test]
    async fn homepage_cache_serves_stale_data_during_one_refresh() {
        let cache = HomepageCache::new(Duration::ZERO);
        cache
            .get_with_loader(|| async { Ok::<_, String>(homepage_data("stale")) })
            .await
            .unwrap();

        let refresh_calls = Arc::new(AtomicUsize::new(0));
        let refresh_started = Arc::new(tokio::sync::Notify::new());
        let allow_refresh = Arc::new(tokio::sync::Notify::new());

        let refresh_cache = cache.clone();
        let first_calls = refresh_calls.clone();
        let first_started = refresh_started.clone();
        let first_allow = allow_refresh.clone();
        let refresh = tokio::spawn(async move {
            refresh_cache
                .get_with_loader(move || {
                    let calls = first_calls.clone();
                    let started = first_started.clone();
                    let allow = first_allow.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        started.notify_one();
                        allow.notified().await;
                        Ok::<_, String>(homepage_data("fresh"))
                    }
                })
                .await
        });

        refresh_started.notified().await;

        let second_calls = refresh_calls.clone();
        let stale = cache
            .get_with_loader(move || {
                let calls = second_calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, String>(homepage_data("unexpected"))
                }
            })
            .await
            .unwrap();

        assert_eq!(stale, homepage_data("stale"));
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);

        allow_refresh.notify_one();
        let refreshed = refresh.await.unwrap().unwrap();

        assert_eq!(refreshed, homepage_data("fresh"));
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn home_region_flags_are_grouped_by_disc() {
        let grouped = group_home_region_flags(vec![
            HomeRegionRow {
                disc_id: 2,
                code: "US".to_string(),
                name: "USA".to_string(),
            },
            HomeRegionRow {
                disc_id: 2,
                code: "JP".to_string(),
                name: "Japan".to_string(),
            },
            HomeRegionRow {
                disc_id: 4,
                code: "EU".to_string(),
                name: "Europe".to_string(),
            },
        ]);

        assert_eq!(
            grouped.get(&2).cloned().unwrap_or_default(),
            vec![
                HomeRegionFlag {
                    code: "us".to_string(),
                    name: "USA".to_string(),
                },
                HomeRegionFlag {
                    code: "jp".to_string(),
                    name: "Japan".to_string(),
                },
            ]
        );
        assert_eq!(
            grouped.get(&4).cloned().unwrap_or_default(),
            vec![HomeRegionFlag {
                code: "eu".to_string(),
                name: "Europe".to_string(),
            }]
        );
        assert!(grouped.get(&3).cloned().unwrap_or_default().is_empty());
    }

    #[test]
    fn homepage_template_renders_recent_lists() {
        crate::config::init_site_config(&crate::config::Config {
            site_name: "localhost".to_string(),
            database_url: "postgres://localhost/test".to_string(),
            site_url: "http://localhost".to_string(),
            base_url: "http://localhost".to_string(),
            wiki_url: "#".to_string(),
            forum_url: "#".to_string(),
            news_feed_url: "#".to_string(),
            port: 0,
            oidc_provider_url: "#".to_string(),
            oidc_client_id: "test".to_string(),
            oidc_client_secret: "test".to_string(),
        });

        let html = MainTemplate {
            current_user: None,
            news_items: vec![NewsItem {
                title: "Site news".to_string(),
                author: "admin".to_string(),
                url: "#".to_string(),
                published_date: "2026-01-03".to_string(),
                published_at: Some(
                    chrono::DateTime::parse_from_rfc3339("2026-01-03T07:08:09Z")
                        .unwrap()
                        .with_timezone(&chrono::Utc),
                ),
                content_html: "hello".to_string(),
            }],
            recent_discs: homepage_data("recent dump").recent_discs,
            recent_changes: homepage_data("recent change").recent_changes,
        }
        .render()
        .unwrap();

        assert!(html.contains("Recent Dumps"));
        assert!(html.contains("recent dump"));
        assert!(html.contains("Recent Changes"));
        assert!(html.contains("recent change changed"));
        assert!(html.contains("News"));
        assert!(html.contains("Site news"));
        assert!(html.contains("2026-01-01"));
        assert!(html.contains("2026-01-02"));
        assert!(html.contains("<time>2026-01-03</time>"));
    }
}
