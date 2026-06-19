use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use axum_extra::extract::Form;

use crate::auth::{
    csrf::{self, CsrfForm},
    middleware::RequireAuth,
    session,
};
use crate::error::AppResult;
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/logout", post(logout))
}

async fn logout(
    State(state): State<AppState>,
    RequireAuth(user): RequireAuth,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    csrf::verify_form(&user, &form)?;

    if let Some(sid) = session::extract_session_cookie(&headers) {
        session::delete_session(&state.pool, &sid).await.ok();
    }

    let cookie = session::expired_session_cookie(&state.config);
    let mut response = Redirect::to("/").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie.parse().unwrap());
    Ok(response)
}
