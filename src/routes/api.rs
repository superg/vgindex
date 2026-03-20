use axum::{extract::State, response::Html, routing::get, Json, Router};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/online", get(online_users))
        .route("/api/online-html", get(online_users_html))
        .route("/api/news", get(news_feed))
        .merge(crate::auth::oidc::routes())
}

async fn online_users(State(state): State<AppState>) -> Json<OnlineInfo> {
    let registered: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM sessions
         WHERE user_id IS NOT NULL AND last_active_at > NOW() - INTERVAL '15 minutes'"
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    let guests: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions
         WHERE user_id IS NULL AND last_active_at > NOW() - INTERVAL '15 minutes'"
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    let usernames: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT u.username FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.last_active_at > NOW() - INTERVAL '15 minutes'
         ORDER BY u.username"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    Json(OnlineInfo {
        registered,
        guests,
        usernames,
    })
}

async fn online_users_html(State(state): State<AppState>) -> Html<String> {
    let registered: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT user_id) FROM sessions
         WHERE user_id IS NOT NULL AND last_active_at > NOW() - INTERVAL '15 minutes'"
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    let guests: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions
         WHERE user_id IS NULL AND last_active_at > NOW() - INTERVAL '15 minutes'"
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    let total = registered + guests;
    let html = if total == 0 {
        "No users online".to_string()
    } else {
        format!("{total} user(s) online ({registered} registered, {guests} guest(s))")
    };

    Html(html)
}

/// News feed from phpBB. Tries to read from the phpBB database directly.
/// Falls back to a static message if phpBB DB is not available.
async fn news_feed(State(state): State<AppState>) -> Html<String> {
    // Try to connect to the phpBB database and read recent topics from a "News" forum.
    // The phpBB database is on the same PostgreSQL server but different database.
    // This requires a separate connection or cross-database query.
    // For simplicity, we'll use a placeholder that can be connected later.
    let phpbb_url = state.config.database_url
        .replace("/vgindex", "/phpbb");

    if let Ok(phpbb_pool) = crate::db::create_pool(&phpbb_url).await {
        // phpBB uses phpbb_ prefix by default with bitnami image
        let topics: Vec<NewsTopicRow> = sqlx::query_as(
            "SELECT t.topic_id, t.topic_title, t.topic_time, t.topic_views
             FROM phpbb_topics t
             JOIN phpbb_forums f ON f.forum_id = t.forum_id
             WHERE f.forum_name = 'News' OR f.forum_id = 1
             ORDER BY t.topic_time DESC
             LIMIT 5"
        )
        .fetch_all(&phpbb_pool)
        .await
        .unwrap_or_default();

        if !topics.is_empty() {
            let mut html = String::from("<ul>");
            for t in &topics {
                let date = chrono::DateTime::from_timestamp(t.topic_time, 0)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                html.push_str(&format!(
                    "<li><small>{date}</small> <a href=\"/forum/viewtopic.php?t={}\">{}</a></li>",
                    t.topic_id,
                    html_escape(&t.topic_title),
                ));
            }
            html.push_str("</ul>");
            return Html(html);
        }
    }

    Html("<p>No news available. Visit the <a href=\"/forum/\">forum</a> for updates.</p>".to_string())
}

#[derive(serde::Serialize)]
struct OnlineInfo {
    registered: i64,
    guests: i64,
    usernames: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct NewsTopicRow {
    topic_id: i64,
    topic_title: String,
    topic_time: i64,
    topic_views: i64,
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
