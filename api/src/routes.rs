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
    /// Start date (inclusive) in YYYY-MM-DD format.
    /// If only `from` is set, fetches `weeks_ahead` weeks forward starting at this date's week.
    from: Option<String>,
    /// End date (inclusive) in YYYY-MM-DD format.
    /// If only `to` is set, fetches `weeks_ahead` weeks backward ending at this date's week.
    to: Option<String>,
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

    let from_date = params
        .from
        .as_deref()
        .map(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|_| AppError::BadRequest(format!("Invalid 'from' date: {s}. Expected YYYY-MM-DD")))
        })
        .transpose()?;

    let to_date = params
        .to
        .as_deref()
        .map(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|_| AppError::BadRequest(format!("Invalid 'to' date: {s}. Expected YYYY-MM-DD")))
        })
        .transpose()?;

    let has_custom_range = from_date.is_some() || to_date.is_some();

    // Compute weeks to fetch based on the query parameters
    let weeks = match (from_date, to_date) {
        // Both set: all weeks spanning from..=to
        (Some(from), Some(to)) => {
            if from > to {
                return Err(AppError::BadRequest(
                    "'from' must be before or equal to 'to'".into(),
                ));
            }
            weeks_between(from, to)
        }
        // Only from: weeks_ahead weeks forward starting at from's week
        (Some(from), None) => {
            let start_week = from.iso_week();
            let mut weeks = vec![start_week];
            let mut monday = NaiveDate::from_isoywd_opt(
                start_week.year(),
                start_week.week(),
                chrono::Weekday::Mon,
            )
            .unwrap();
            for _ in 0..state.config.weeks_ahead {
                monday = monday.checked_add_days(Days::new(7)).unwrap();
                weeks.push(monday.iso_week());
            }
            weeks
        }
        // Only to: weeks_ahead weeks backward ending at to's week
        (None, Some(to)) => {
            let end_week = to.iso_week();
            let end_monday = NaiveDate::from_isoywd_opt(
                end_week.year(),
                end_week.week(),
                chrono::Weekday::Mon,
            )
            .unwrap();
            let start_monday = end_monday
                .checked_sub_days(Days::new(7 * u64::from(state.config.weeks_ahead)))
                .unwrap();
            weeks_between(start_monday, end_monday)
        }
        // Neither: default behaviour (current week + weeks_ahead forward)
        (None, None) => {
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
            weeks
        }
    };

    let ttl = Duration::from_secs(state.config.cache_ttl_seconds);

    // Only use cache for the default (no custom range) requests
    if !has_custom_range {
        // Check cache
        {
            let cache = state.cache.read().await;
            if let Some(ref cached) = *cache {
                let age = cached.generated_at.elapsed();
                if age < ttl {
                    info!(age_secs = age.as_secs(), "Serving calendar from cache");
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

        {
            let cache = state.cache.read().await;
            if let Some(ref cached) = *cache {
                info!(expired_secs_ago = cached.generated_at.elapsed().as_secs().saturating_sub(ttl.as_secs()), "Cache expired, refreshing");
            } else {
                info!("No cached calendar, fetching");
            }
        }
    } else {
        info!(?from_date, ?to_date, weeks_count = weeks.len(), "Custom date range requested, bypassing cache");
    }

    let client = DualisClient::new()?;
    let timetables = client
        .fetch_timetables(
            &state.config.dualis_username,
            &state.config.dualis_password,
            &weeks,
        )
        .await?;

    let total_events: usize = timetables.iter().flat_map(|t| &t.days).map(|d| d.events.len()).sum();
    info!(weeks = timetables.len(), total_events, "Calendar generated");

    let ics = ical::build_calendar(&timetables, &state.config);

    // Only update cache for default requests
    if !has_custom_range {
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

/// Compute all distinct ISO weeks between two dates (inclusive of both dates' weeks).
fn weeks_between(from: NaiveDate, to: NaiveDate) -> Vec<chrono::IsoWeek> {
    let from_week = from.iso_week();
    let to_week = to.iso_week();
    let mut weeks = vec![from_week];
    let mut monday = NaiveDate::from_isoywd_opt(
        from_week.year(),
        from_week.week(),
        chrono::Weekday::Mon,
    )
    .unwrap();
    loop {
        monday = monday.checked_add_days(Days::new(7)).unwrap();
        let w = monday.iso_week();
        if w.year() > to_week.year()
            || (w.year() == to_week.year() && w.week() > to_week.week())
        {
            break;
        }
        weeks.push(w);
    }
    weeks
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
