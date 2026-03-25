pub mod api;
pub mod auth_routes;
pub mod disc_edit;
pub mod disc_submit;
pub mod disc_view;
pub mod discs;
pub mod downloads;
pub mod feeds;
pub mod main_page;
pub mod submissions;

use axum::Router;
use crate::AppState;

pub fn build_router() -> Router<AppState> {
    Router::new()
        .merge(main_page::routes())
        .merge(auth_routes::routes())
        .merge(crate::auth::oidc::routes())
        .merge(discs::routes())
        .merge(disc_view::routes())
        .merge(disc_edit::routes())
        .merge(disc_submit::routes())
        .merge(downloads::routes())
        .merge(submissions::routes())
        .merge(feeds::routes())
        .merge(api::routes())
}
