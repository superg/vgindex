use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};

use crate::auth::session;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/logout", post(logout))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(sid) = session::extract_session_cookie(&headers) {
        session::delete_session(&state.pool, &sid).await.ok();
    }

    let cookie = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        session::SESSION_COOKIE_NAME
    );
    let mut response = Redirect::to("/").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie.parse().unwrap());
    response
}
