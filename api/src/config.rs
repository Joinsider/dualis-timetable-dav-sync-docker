use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub api_key: String,
    pub dualis_username: String,
    pub dualis_password: String,
    pub port: u16,
    pub weeks_ahead: u32,
    pub calendar_name: String,
    pub uid_domain: String,
    pub timezone: String,
    pub cache_ttl_seconds: u64,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Ok(Self {
            api_key: required("API_KEY")?,
            dualis_username: required("DUALIS_USERNAME")?,
            dualis_password: required("DUALIS_PASSWORD")?,
            port: env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()
                .map_err(|_| "PORT must be a number".to_string())?,
            weeks_ahead: env::var("WEEKS_AHEAD")
                .unwrap_or_else(|_| "2".into())
                .parse()
                .map_err(|_| "WEEKS_AHEAD must be a number".to_string())?,
            calendar_name: env::var("CALENDAR_NAME")
                .unwrap_or_else(|_| "Dualis Timetable".into()),
            uid_domain: env::var("UID_DOMAIN")
                .unwrap_or_else(|_| "schedule-sync.local".into()),
            timezone: env::var("TIMEZONE")
                .unwrap_or_else(|_| "Europe/Berlin".into()),
            cache_ttl_seconds: env::var("CACHE_TTL_SECONDS")
                .unwrap_or_else(|_| "3600".into())
                .parse()
                .map_err(|_| "CACHE_TTL_SECONDS must be a number".to_string())?,
        })
    }
}

fn required(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("Missing required env var: {key}"))
}
