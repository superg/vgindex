use chrono::{DateTime, Utc};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const NEWS_ITEM_LIMIT: usize = 5;
const FEED_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewsItem {
    pub title: String,
    pub author: String,
    pub url: String,
    pub published_date: String,
    pub published_at: Option<DateTime<Utc>>,
    pub content_html: String,
}

#[derive(Debug, thiserror::Error)]
pub enum NewsError {
    #[error("news feed request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("news feed returned HTTP {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("news feed XML is invalid: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("{0}")]
    Message(String),
}

#[derive(Clone)]
pub struct NewsCache {
    inner: Arc<RwLock<CachedNews>>,
    ttl: Duration,
}

#[derive(Default)]
struct CachedNews {
    loaded_at: Option<Instant>,
    items: Vec<NewsItem>,
}

impl NewsCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(CachedNews::default())),
            ttl,
        }
    }

    pub async fn get(
        &self,
        http: &reqwest::Client,
        feed_url: &str,
    ) -> Result<Vec<NewsItem>, NewsError> {
        self.get_with_loader(|| fetch_news_feed(http, feed_url))
            .await
    }

    async fn get_with_loader<F, Fut>(&self, loader: F) -> Result<Vec<NewsItem>, NewsError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<NewsItem>, NewsError>>,
    {
        let now = Instant::now();
        {
            let cached = self.inner.read().await;
            if cached.is_fresh(now, self.ttl) {
                return Ok(cached.items.clone());
            }
        }

        let stale = {
            let cached = self.inner.read().await;
            cached.loaded_at.map(|_| cached.items.clone())
        };

        match loader().await {
            Ok(items) => {
                let mut cached = self.inner.write().await;
                cached.loaded_at = Some(Instant::now());
                cached.items = items.clone();
                Ok(items)
            }
            Err(err) => {
                if let Some(items) = stale {
                    tracing::warn!("Failed to refresh news feed; using stale cache: {err}");
                    Ok(items)
                } else {
                    Err(err)
                }
            }
        }
    }
}

impl CachedNews {
    fn is_fresh(&self, now: Instant, ttl: Duration) -> bool {
        self.loaded_at
            .map(|loaded_at| now.duration_since(loaded_at) < ttl)
            .unwrap_or(false)
    }
}

async fn fetch_news_feed(
    http: &reqwest::Client,
    feed_url: &str,
) -> Result<Vec<NewsItem>, NewsError> {
    let response = http
        .get(feed_url)
        .timeout(FEED_REQUEST_TIMEOUT)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(NewsError::HttpStatus(status));
    }

    let xml = response.text().await?;
    parse_news_feed(&xml)
}

pub(crate) fn parse_news_feed(xml: &str) -> Result<Vec<NewsItem>, NewsError> {
    let doc = roxmltree::Document::parse(xml)?;
    let mut items = Vec::new();

    for entry in doc
        .descendants()
        .filter(|node| node.is_element() && is_tag(*node, "entry"))
    {
        let title = child_text(entry, "title").unwrap_or_default();
        let url = entry_link(entry).unwrap_or_default();
        if title.is_empty() || url.is_empty() {
            continue;
        }

        let published_at = child_text(entry, "published")
            .or_else(|| child_text(entry, "updated"))
            .as_deref()
            .and_then(parse_atom_date);
        let published_date = published_at
            .map(|date| date.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        let author = entry
            .children()
            .find(|node| node.is_element() && is_tag(*node, "author"))
            .and_then(|author| child_text(author, "name"))
            .unwrap_or_default();
        let content_html = child_text(entry, "content")
            .or_else(|| child_text(entry, "summary"))
            .map(|content| clean_phpbb_feed_content(&content, &url))
            .unwrap_or_default();

        items.push(NewsItem {
            title,
            author,
            url,
            published_date,
            published_at,
            content_html,
        });
    }

    items.sort_by(|a, b| {
        b.published_at
            .cmp(&a.published_at)
            .then_with(|| b.url.cmp(&a.url))
            .then_with(|| a.title.cmp(&b.title))
    });
    items.truncate(NEWS_ITEM_LIMIT);
    Ok(items)
}

fn is_tag(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.tag_name().name() == name
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && is_tag(*child, name))
        .and_then(|child| child.text())
        .map(|text| text.trim().to_string())
}

fn entry_link(entry: roxmltree::Node<'_, '_>) -> Option<String> {
    entry
        .children()
        .filter(|node| node.is_element() && is_tag(*node, "link"))
        .find_map(|link| link.attribute("href").map(str::trim))
        .filter(|href| !href.is_empty())
        .map(str::to_string)
        .or_else(|| child_text(entry, "id"))
}

fn parse_atom_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn clean_phpbb_feed_content(raw: &str, base_url: &str) -> String {
    let without_separator = strip_trailing_hr(raw.trim());
    let without_statistics = strip_trailing_statistics(without_separator).trim();
    absolutize_html_urls(without_statistics, base_url)
}

fn strip_trailing_hr(value: &str) -> &str {
    for suffix in ["<hr />", "<hr/>", "<hr>"] {
        if let Some(stripped) = strip_suffix_ignore_ascii_case(value, suffix) {
            return stripped.trim_end();
        }
    }
    value
}

fn strip_trailing_statistics(value: &str) -> &str {
    let lower = value.to_ascii_lowercase();
    if !lower.ends_with("</p>") {
        return value;
    }

    let Some(start) = lower.rfind("<p>") else {
        return value;
    };
    let paragraph = value[start..].trim_start();
    if paragraph
        .get(3..)
        .is_some_and(|text| text.trim_start().starts_with("Statistics:"))
    {
        value[..start].trim_end()
    } else {
        value
    }
}

fn strip_suffix_ignore_ascii_case<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    if value.len() < suffix.len() {
        return None;
    }
    let start = value.len() - suffix.len();
    value[start..]
        .eq_ignore_ascii_case(suffix)
        .then_some(&value[..start])
}

fn absolutize_html_urls(html: &str, base_url: &str) -> String {
    let Ok(base) = reqwest::Url::parse(base_url) else {
        return html.to_string();
    };

    let mut output = String::with_capacity(html.len());
    let mut remaining = html;
    while let Some((attr_start, attr)) = next_url_attr(remaining) {
        let value_start = attr_start + attr.len();
        let Some(value_len) = remaining[value_start..].find('"') else {
            break;
        };
        let value = &remaining[value_start..value_start + value_len];
        output.push_str(&remaining[..value_start]);
        output.push_str(&resolve_html_url(&base, value));
        output.push('"');
        remaining = &remaining[value_start + value_len + 1..];
    }
    output.push_str(remaining);
    output
}

fn next_url_attr(html: &str) -> Option<(usize, &'static str)> {
    [
        ("href=\"", html.find("href=\"")),
        ("src=\"", html.find("src=\"")),
    ]
    .into_iter()
    .filter_map(|(attr, pos)| pos.map(|pos| (pos, attr)))
    .min_by_key(|(pos, _)| *pos)
}

fn resolve_html_url(base: &reqwest::Url, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return value.to_string();
    }

    base.join(trimmed)
        .map(|url| url.to_string())
        .unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sample_item(title: &str) -> NewsItem {
        NewsItem {
            title: title.to_string(),
            author: "Alice".to_string(),
            url: format!("https://forum.example/viewtopic.php?t={title}"),
            published_date: "2026-01-01".to_string(),
            published_at: Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()),
            content_html: "<p>Hello</p>".to_string(),
        }
    }

    #[test]
    fn parses_atom_news_entries_and_limits_to_five() {
        let mut entries = String::new();
        for day in 1..=6 {
            entries.push_str(&format!(
                r#"
                <entry>
                    <author><name><![CDATA[Author {day}]]></name></author>
                    <published>2026-05-{day:02}T12:00:00+00:00</published>
                    <updated>2026-05-{day:02}T12:30:00+00:00</updated>
                    <id>https://forum.example/viewtopic.php?p={day}#p{day}</id>
                    <link href="https://forum.example/viewtopic.php?p={day}#p{day}" />
                    <title type="html"><![CDATA[News {day}]]></title>
                    <content type="html"><![CDATA[
                        <p>Body {day}</p><a href="/viewtopic.php?t={day}">Thread</a><p>Statistics: Posted by Author</p><hr />
                    ]]></content>
                </entry>
                "#
            ));
        }
        let xml = format!(r#"<feed xmlns="http://www.w3.org/2005/Atom">{entries}</feed>"#);

        let items = parse_news_feed(&xml).unwrap();

        assert_eq!(items.len(), 5);
        assert_eq!(items[0].title, "News 6");
        assert_eq!(items[0].author, "Author 6");
        assert_eq!(items[0].published_date, "2026-05-06");
        assert_eq!(items[0].url, "https://forum.example/viewtopic.php?p=6#p6");
        assert_eq!(
            items[0].content_html,
            r#"<p>Body 6</p><a href="https://forum.example/viewtopic.php?t=6">Thread</a>"#
        );
        assert_eq!(items[4].title, "News 2");
    }

    #[test]
    fn falls_back_to_updated_when_published_is_missing() {
        let xml = r#"
            <feed xmlns="http://www.w3.org/2005/Atom">
                <entry>
                    <author><name>Alice</name></author>
                    <updated>2026-03-04T05:06:07+00:00</updated>
                    <link href="https://forum.example/viewtopic.php?p=1#p1" />
                    <title>Updated only</title>
                    <content><![CDATA[<p>Body</p>]]></content>
                </entry>
            </feed>
        "#;

        let items = parse_news_feed(xml).unwrap();

        assert_eq!(items[0].published_date, "2026-03-04");
    }

    #[tokio::test]
    async fn cache_returns_fresh_items_without_refreshing() {
        let cache = NewsCache::new(Duration::from_secs(60));
        let calls = AtomicUsize::new(0);

        let first = cache
            .get_with_loader(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![sample_item("first")])
            })
            .await
            .unwrap();
        let second = cache
            .get_with_loader(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![sample_item("second")])
            })
            .await
            .unwrap();

        assert_eq!(first[0].title, "first");
        assert_eq!(second[0].title, "first");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cache_serves_stale_items_when_refresh_fails() {
        let cache = NewsCache::new(Duration::ZERO);
        cache
            .get_with_loader(|| async { Ok(vec![sample_item("stale")]) })
            .await
            .unwrap();

        let items = cache
            .get_with_loader(|| async { Err(NewsError::Message("refresh failed".into())) })
            .await
            .unwrap();

        assert_eq!(items[0].title, "stale");
    }

    #[tokio::test]
    async fn cache_returns_error_without_stale_items() {
        let cache = NewsCache::new(Duration::ZERO);

        let err = cache
            .get_with_loader(|| async { Err(NewsError::Message("no feed".into())) })
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "no feed");
    }
}
