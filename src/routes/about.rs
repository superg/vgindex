use askama::Template;
use axum::{response::Html, routing::get, Router};

use crate::auth::middleware::CurrentUser;
use crate::config::SiteConfig;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/about/", get(about_page))
}

#[derive(Template)]
#[template(path = "about.html")]
struct AboutTemplate {
    current_user: Option<String>,
}
impl SiteConfig for AboutTemplate {}

async fn about_page(user: CurrentUser) -> Html<String> {
    Html(
        AboutTemplate {
            current_user: user.user().map(|u| u.username.clone()),
        }
        .render()
        .unwrap(),
    )
}
