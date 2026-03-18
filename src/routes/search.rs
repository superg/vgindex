use axum::{extract::{Query, State}, response::Html, routing::get, Router};
use serde::Deserialize;

use crate::services::search_service;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/search", get(quick_search))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

async fn quick_search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Html<String> {
    let results = search_service::quick_search(&state.pool, &query.q, 10).await;

    let mut html = String::from("<ul class=\"search-results\">");
    for r in &results {
        html.push_str(&format!(
            "<li><a href=\"/disc/{}/\">{} <small>({})</small></a></li>",
            r.id, r.title, r.system
        ));
    }
    if results.is_empty() {
        html.push_str("<li>No results found</li>");
    }
    html.push_str("</ul>");
    Html(html)
}
