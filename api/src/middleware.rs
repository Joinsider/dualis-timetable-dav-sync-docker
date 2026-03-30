use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::{AppState, error::AppError};

pub async fn require_api_key(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let key = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match key {
        Some(k) if k == state.config.api_key => Ok(next.run(req).await),
        _ => Err(AppError::Unauthorized),
    }
}
