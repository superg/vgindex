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
        "SELECT d.id, d.title, s.short_code AS system, d.created_at
         FROM discs d JOIN systems s ON s.id = d.system_id
         WHERE d.status != 'Bad'
         ORDER BY d.created_at DESC LIMIT 50"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let base = &state.config.base_url;
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
<title>Redump.org - Recent Dumps</title>
<link>"#,
    );
    xml.push_str(base);
    xml.push_str("</link>\n<description>Recent disc dumps on redump.org</description>\n");

    for row in &rows {
        xml.push_str("<item>\n<title>");
        xml.push_str(&html_escape(&format!("[{}] {}", row.system, row.title)));
        xml.push_str("</title>\n<link>");
        xml.push_str(&format!("{base}/disc/{}/", row.id));
        xml.push_str("</link>\n<pubDate>");
        xml.push_str(&row.created_at.to_rfc2822());
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
    created_at: chrono::DateTime<chrono::Utc>,
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
