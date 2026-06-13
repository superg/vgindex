pub mod about;
pub mod api;
pub mod auth_routes;
pub mod disc_edit;
pub mod disc_view;
pub mod discs;
pub mod downloads;
pub mod main_page;
pub mod queue;

use crate::error::{AppError, AppResult};
use crate::AppState;
use axum::{
    extract::{rejection::PathRejection, Path},
    Router,
};

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

pub(crate) fn path_i32(path: Result<Path<i32>, PathRejection>) -> AppResult<i32> {
    path.map(|Path(id)| id).map_err(|_| AppError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_i32_returns_value_for_valid_extractor() {
        assert_eq!(path_i32(Ok(Path(123))).unwrap(), 123);
    }
}
