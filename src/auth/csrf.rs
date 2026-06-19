use axum::http::HeaderMap;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::auth::middleware::AuthenticatedUser;
use crate::error::{AppError, AppResult};

pub const CSRF_FIELD_NAME: &str = "_csrf";
pub const CSRF_HEADER_NAME: &str = "x-csrf-token";

#[derive(Debug, Deserialize)]
pub struct CsrfForm {
    #[serde(default, rename = "_csrf")]
    pub csrf_token: String,
}

pub fn verify_form(user: &AuthenticatedUser, form: &CsrfForm) -> AppResult<()> {
    verify_token(user, &form.csrf_token)
}

pub fn verify_headers(user: &AuthenticatedUser, headers: &HeaderMap) -> AppResult<()> {
    let token = headers
        .get(CSRF_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    verify_token(user, token)
}

pub fn verify_token(user: &AuthenticatedUser, token: &str) -> AppResult<()> {
    if tokens_match(&user.csrf_token, token) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn tokens_match(expected: &str, submitted: &str) -> bool {
    let expected = expected.trim();
    let submitted = submitted.trim();
    if expected.is_empty() || submitted.is_empty() {
        return false;
    }

    let expected_digest = Sha256::digest(expected.as_bytes());
    let submitted_digest = Sha256::digest(submitted.as_bytes());
    expected_digest
        .iter()
        .zip(submitted_digest.iter())
        .fold(0u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::UserRole;

    fn user() -> AuthenticatedUser {
        AuthenticatedUser {
            id: 1,
            username: "tester".to_string(),
            role: UserRole::User,
            csrf_token: "known-csrf-token".to_string(),
            avatar_url: None,
        }
    }

    #[test]
    fn verify_token_accepts_matching_token() {
        assert!(verify_token(&user(), "known-csrf-token").is_ok());
    }

    #[test]
    fn verify_token_rejects_missing_empty_and_wrong_tokens() {
        let user = user();

        assert!(matches!(verify_token(&user, ""), Err(AppError::Forbidden)));
        assert!(matches!(
            verify_token(&user, "different-token"),
            Err(AppError::Forbidden)
        ));

        let mut no_token_user = user.clone();
        no_token_user.csrf_token.clear();
        assert!(matches!(
            verify_token(&no_token_user, "known-csrf-token"),
            Err(AppError::Forbidden)
        ));
    }

    #[test]
    fn verify_headers_accepts_csrf_header() {
        let mut headers = HeaderMap::new();
        headers.insert(CSRF_HEADER_NAME, "known-csrf-token".parse().unwrap());

        assert!(verify_headers(&user(), &headers).is_ok());
    }
}
