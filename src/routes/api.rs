use axum::{extract::State, response::Html, routing::get, Json, Router};

use crate::auth::middleware::CurrentUser;
use crate::db::models::html_escape;
use crate::AppState;

const ACTIVE_WINDOW_MINUTES: i32 = 2;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/online", get(online_users))
        .route("/api/online-html", get(online_users_html))
        .route("/api/news", get(news_feed))
}

async fn online_users(State(state): State<AppState>) -> Json<OnlineInfo> {
    let (registered, guests) = fetch_online_counts(&state).await;

    let usernames: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT u.username FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.user_id IS NOT NULL
           AND s.expires_at > NOW()
           AND s.last_active_at > NOW() - ($1 * INTERVAL '1 minute')
         ORDER BY u.username"
    )
    .bind(ACTIVE_WINDOW_MINUTES)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    Json(OnlineInfo {
        registered,
        guests,
        usernames,
    })
}

async fn online_users_html(State(state): State<AppState>, _user: CurrentUser) -> Html<String> {
    let (registered, guests) = fetch_online_counts(&state).await;

    let html = format!("Users: {registered}, Guests: {guests}");

    Html(html)
}

async fn fetch_online_counts(state: &AppState) -> (i64, i64) {
    sqlx::query_as::<_, (i64, i64)>(
        "SELECT
             COUNT(DISTINCT user_id) FILTER (WHERE user_id IS NOT NULL) AS registered,
             COUNT(*) FILTER (WHERE user_id IS NULL) AS guests
         FROM sessions
         WHERE expires_at > NOW()
           AND last_active_at > NOW() - ($1 * INTERVAL '1 minute')",
    )
    .bind(ACTIVE_WINDOW_MINUTES)
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0, 0))
}

/// News feed from phpBB. Tries to read from the phpBB database directly.
/// Falls back to a static message if phpBB DB is not available.
async fn news_feed(State(state): State<AppState>) -> Html<String> {
    let phpbb_url = state.config.database_url
        .rsplitn(2, '/')
        .last()
        .unwrap_or(&state.config.database_url)
        .to_owned()
        + "/phpbb";
    let forum_base = state.config.forum_url.trim_end_matches('/');

    if let Ok(phpbb_pool) = crate::db::create_pool(&phpbb_url).await {
        // phpBB uses phpbb_ prefix by default
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
                    "<li><small>{date}</small> <a href=\"{}/viewtopic.php?t={}\">{}</a></li>",
                    forum_base,
                    t.topic_id,
                    html_escape(&t.topic_title),
                ));
            }
            html.push_str("</ul>");
            return Html(html);
        }
    }

    Html(format!(
        "<p>No news available. Visit the <a href=\"{}/\">forum</a> for updates.</p>",
        forum_base
    ))
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

