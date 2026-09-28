mod config;
mod dualis;
mod error;
mod ical;
mod login_guard;
mod middleware;
mod routes;

use axum::{middleware as axum_middleware, routing::get, Router};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower_http::trace::TraceLayer;
use tracing::{error, info};

pub struct AppState {
    pub config: config::Config,
    pub cache: RwLock<Option<CachedCalendar>>,
    pub login_guard: Arc<login_guard::LoginGuard>,
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
        error!("Configuration error: {e}");
        std::process::exit(1);
    });

    info!(
        port = config.port,
        username = %config.dualis_username,
        weeks_ahead = config.weeks_ahead,
        cache_ttl_seconds = config.cache_ttl_seconds,
        calendar_name = %config.calendar_name,
        login_max_retries = config.login_max_retries,
        login_state_file = %config.login_state_file.display(),
        "Config loaded"
    );

    let login_guard = login_guard::LoginGuard::load(
        config.login_state_file.clone(),
        config.login_max_retries,
        &config.dualis_username,
        &config.dualis_password,
    )
    .unwrap_or_else(|e| {
        error!("Login guard error: {e}");
        std::process::exit(1);
    });
    login_guard.log_status().await;

    let port = config.port;
    let state = Arc::new(AppState {
        config,
        cache: RwLock::new(None),
        login_guard: Arc::new(login_guard),
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
