pub mod about;
pub mod api;
pub mod auth_routes;
pub mod disc_edit;
pub mod disc_view;
pub mod discs;
pub mod downloads;
pub mod main_page;
pub mod queue;

use crate::AppState;
use axum::Router;

pub fn build_router() -> Router<AppState> {
    Router::new()
        .merge(main_page::routes())
        .merge(auth_routes::routes())
        .merge(crate::auth::oidc::routes())
        .merge(discs::routes())
        .merge(disc_view::routes())
        .merge(disc_edit::routes())
        .merge(downloads::routes())
        .merge(queue::routes())
        .merge(api::routes())
        .merge(about::routes())
}
