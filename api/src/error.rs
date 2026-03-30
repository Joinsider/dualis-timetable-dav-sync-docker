use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Login failed: {0}")]
    LoginFailed(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Bad request: {0}")]
    BadRequest(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::LoginFailed(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            AppError::Parse(msg) => (StatusCode::BAD_GATEWAY, msg.clone()),
            AppError::Http(e) => (StatusCode::BAD_GATEWAY, e.to_string()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
