use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{Datelike, Local, NaiveDate};
use serde::Deserialize;
use std::sync::Arc;
use tracing::info;

use crate::{config::Config, dualis::DualisClient, error::AppError};

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

pub async fn timetable_raw(
    State(config): State<Arc<Config>>,
    Query(params): Query<TimetableParams>,
) -> Result<axum::response::Html<String>, AppError> {
    let week = parse_week(params.week.as_deref())?;
    let client = DualisClient::new()?;
    let html = client
        .fetch_timetable_raw(&config.dualis_username, &config.dualis_password, week)
        .await?;
    Ok(axum::response::Html(html))
}

#[derive(Deserialize)]
pub struct TimetableParams {
    /// ISO week string, e.g. "2024-W05".
    /// Defaults to current week if omitted.
    week: Option<String>,
}

pub async fn timetable(
    State(config): State<Arc<Config>>,
    Query(params): Query<TimetableParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    let week = parse_week(params.week.as_deref())?;
    info!(week = %format!("{}-W{:02}", week.year(), week.week()), "Fetching timetable");

    let client = DualisClient::new()?;
    let timetable = client
        .fetch_timetable(&config.dualis_username, &config.dualis_password, week)
        .await?;

    Ok(Json(serde_json::to_value(timetable).unwrap()))
}

fn parse_week(input: Option<&str>) -> Result<chrono::IsoWeek, AppError> {
    match input {
        None => Ok(Local::now().date_naive().iso_week()),
        Some(s) => {
            // Accept "YYYY-Www" (e.g. "2024-W05") or "YYYY-WW" (e.g. "2024-05")
            let s = s.to_uppercase();
            let s = s.trim_start_matches(|c: char| !c.is_ascii_digit());

            // Try parsing as YYYY-Www
            let parts: Vec<&str> = s.splitn(2, 'W').collect();
            if parts.len() == 2 {
                let year: i32 = parts[0]
                    .trim_end_matches('-')
                    .parse()
                    .map_err(|_| AppError::BadRequest(format!("Invalid year in week: {s}")))?;
                let week: u32 = parts[1]
                    .parse()
                    .map_err(|_| AppError::BadRequest(format!("Invalid week number in: {s}")))?;

                NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon)
                    .map(|d| d.iso_week())
                    .ok_or_else(|| AppError::BadRequest(format!("Invalid ISO week: {s}")))
            } else {
                Err(AppError::BadRequest(
                    "week must be in ISO format, e.g. '2024-W05'".into(),
                ))
            }
        }
    }
}
