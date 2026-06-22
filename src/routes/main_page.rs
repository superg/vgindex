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
use crate::services::disc_service::RECENT_CHANGE_PREDICATE;
use crate::services::news_service::NewsItem;
use crate::AppState;

const HOME_RECENT_LIMIT: i64 = 30;
const HOME_NEWS_LIMIT: usize = 3;
pub const HOMEPAGE_CACHE_TTL_SECONDS: u64 = 60;
// Shares `RECENT_CHANGE_PREDICATE` with the disc list "Modification date" sort so
// both agree on what counts as a genuine change.
fn home_recent_changes_sql() -> String {
    format!(
        "SELECT d.id, d.title, s.code AS system_code, s.short_name AS system_short_name,
                MAX(ds.created_at) AS modified_at
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         JOIN disc_submissions ds ON ds.target_disc_id = d.id
         WHERE d.status != 'Disabled'
           AND {RECENT_CHANGE_PREDICATE}
         GROUP BY d.id, d.title, s.code, s.short_name
         ORDER BY MAX(ds.created_at) DESC, d.id DESC
         LIMIT $1"
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
    let rows: Vec<RecentDiscRow> = sqlx::query_as(
        "SELECT d.id, d.title, s.code AS system_code, s.short_name AS system_short_name,
                (SELECT MIN(ds.created_at)
                 FROM disc_submissions ds
                 WHERE ds.target_disc_id = d.id) AS created_at
         FROM discs d
         JOIN systems s ON s.code = d.system_code
         WHERE d.status != 'Disabled'
         ORDER BY d.id DESC
         LIMIT $1",
    )
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
        recent_discs.push(RecentDisc {
            id: r.id,
            title: r.title,
            system: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            region_flags: regions_by_disc.get(&r.id).cloned().unwrap_or_default(),
            created_at: r
                .created_at
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
        });
    }

    let mut recent_changes = Vec::with_capacity(change_rows.len());
    for r in change_rows {
        recent_changes.push(RecentChange {
            id: r.id,
            title: r.title,
            system: crate::db::models::short_system_display(&r.system_short_name, &r.system_code),
            region_flags: regions_by_disc.get(&r.id).cloned().unwrap_or_default(),
            modified_at: r
                .modified_at
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
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
    system_code: String,
    system_short_name: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct RecentChangeRow {
    id: i32,
    title: String,
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
    fn recent_changes_query_only_uses_public_history_rows() {
        assert!(home_recent_changes_sql().contains("ds.status IN ('Approved', 'Legacy')"));
    }

    #[test]
    fn recent_changes_query_excludes_new_disc_creation_rows() {
        let sql = home_recent_changes_sql();
        assert!(sql.contains("ds.changes = '{}'::jsonb"));
        assert!(sql.contains(
            "COALESCE(ds.review_comment, '') IN ('added-backfill', 'no-added-sentinel')"
        ));
        assert!(sql.contains("ds.submission_type = 'Disc'"));
        assert!(sql.contains("ds.id <> ("));
        assert!(sql.contains("SELECT MIN(ds_first.id)"));
    }

    #[test]
    fn recent_changes_query_keeps_edit_rows() {
        assert!(home_recent_changes_sql().contains("ds.submission_type = 'Edit'"));
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
                published_at: None,
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
    }
}
