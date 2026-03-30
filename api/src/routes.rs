use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{Datelike, Days, Local, NaiveDate};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

use crate::{
    AppState, CachedCalendar,
    dualis::DualisClient,
    error::AppError,
    ical,
};

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

pub async fn timetable_raw(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TimetableParams>,
) -> Result<axum::response::Html<String>, AppError> {
    let week = parse_week(params.week.as_deref())?;
    let client = DualisClient::new()?;
    let html = client
        .fetch_timetable_raw(
            &state.config.dualis_username,
            &state.config.dualis_password,
            week,
        )
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
    State(state): State<Arc<AppState>>,
    Query(params): Query<TimetableParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    let week = parse_week(params.week.as_deref())?;
    info!(week = %format!("{}-W{:02}", week.year(), week.week()), "Fetching timetable");

    let client = DualisClient::new()?;
    let timetable = client
        .fetch_timetable(
            &state.config.dualis_username,
            &state.config.dualis_password,
            week,
        )
        .await?;

    Ok(Json(serde_json::to_value(timetable).unwrap()))
}

#[derive(Deserialize)]
pub struct CalendarParams {
    token: Option<String>,
}

pub async fn calendar_ics(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CalendarParams>,
) -> Result<impl IntoResponse, AppError> {
    // Authenticate via query param token
    match params.token.as_deref() {
        Some(t) if t == state.config.api_key => {}
        _ => return Err(AppError::Unauthorized),
    }

    let ttl = Duration::from_secs(state.config.cache_ttl_seconds);

    // Check cache
    {
        let cache = state.cache.read().await;
        if let Some(ref cached) = *cache {
            if cached.generated_at.elapsed() < ttl {
                info!("Serving calendar from cache");
                return Ok((
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
                        (header::CACHE_CONTROL, "public, max-age=3600"),
                    ],
                    cached.ics.clone(),
                ));
            }
        }
    }

    // Fetch fresh data
    info!("Generating fresh calendar");
    let today = Local::now().date_naive();
    let current_week = today.iso_week();

    let mut weeks = vec![current_week];
    let mut monday = NaiveDate::from_isoywd_opt(
        current_week.year(),
        current_week.week(),
        chrono::Weekday::Mon,
    )
    .unwrap();
    for _ in 0..state.config.weeks_ahead {
        monday = monday.checked_add_days(Days::new(7)).unwrap();
        weeks.push(monday.iso_week());
    }

    let client = DualisClient::new()?;
    let timetables = client
        .fetch_timetables(
            &state.config.dualis_username,
            &state.config.dualis_password,
            &weeks,
        )
        .await?;

    let ics = ical::build_calendar(&timetables, &state.config);

    // Update cache
    {
        let mut cache = state.cache.write().await;
        *cache = Some(CachedCalendar {
            ics: ics.clone(),
            generated_at: Instant::now(),
        });
    }

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        ics,
    ))
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
