use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/feeds/recent/rss", get(recent_rss))
}

async fn recent_rss(State(state): State<AppState>) -> Response {
    let rows: Vec<RssRow> = sqlx::query_as(
        "SELECT d.id, d.title, s.code AS system, sub.created_at
         FROM (
             SELECT target_disc_id, MAX(created_at) AS created_at
             FROM disc_submissions
             WHERE target_disc_id IS NOT NULL
             GROUP BY target_disc_id
             ORDER BY created_at DESC
             LIMIT 50
         ) sub
         JOIN discs d ON d.id = sub.target_disc_id AND d.enabled
         JOIN systems s ON s.code = d.system_code
         ORDER BY sub.created_at DESC"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let base = &state.config.base_url;
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
<title>vgindex.org - Recent Dumps</title>
<link>"#,
    );
    xml.push_str(base);
    xml.push_str("</link>\n<description>Recent disc dumps on vgindex.org</description>\n");

    for row in &rows {
        xml.push_str("<item>\n<title>");
        xml.push_str(&html_escape(&format!("[{}] {}", row.system, row.title)));
        xml.push_str("</title>\n<link>");
        xml.push_str(&format!("{base}/disc/{}/", row.id));
        xml.push_str("</link>\n<pubDate>");
        if let Some(dt) = row.created_at {
            xml.push_str(&dt.to_rfc2822());
        }
        xml.push_str("</pubDate>\n</item>\n");
    }

    xml.push_str("</channel>\n</rss>");

    ([(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")], xml).into_response()
}

#[derive(sqlx::FromRow)]
struct RssRow {
    id: i32,
    title: String,
    system: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
