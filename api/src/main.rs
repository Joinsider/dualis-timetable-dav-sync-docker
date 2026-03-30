mod config;
mod dualis;
mod error;
mod ical;
mod middleware;
mod routes;

use axum::{middleware as axum_middleware, routing::get, Router};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower_http::trace::TraceLayer;
use tracing::info;

pub struct AppState {
    pub config: config::Config,
    pub cache: RwLock<Option<CachedCalendar>>,
}

pub struct CachedCalendar {
    pub ics: String,
    pub generated_at: Instant,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dualis_scraper=info,tower_http=info".into()),
        )
        .init();

    let config = config::Config::from_env().unwrap_or_else(|e| {
        eprintln!("Configuration error: {e}");
        std::process::exit(1);
    });

    let port = config.port;
    let state = Arc::new(AppState {
        config,
        cache: RwLock::new(None),
    });

    let protected = Router::new()
        .route("/timetable", get(routes::timetable))
        .route("/debug/timetable", get(routes::timetable_raw))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::require_api_key,
        ));

    let app = Router::new()
        .route("/health", get(routes::health))
        .route("/calendar.ics", get(routes::calendar_ics))
        .merge(protected)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    info!("Listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}
