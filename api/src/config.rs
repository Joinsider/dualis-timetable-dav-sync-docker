use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub api_key: String,
    pub dualis_username: String,
    pub dualis_password: String,
    pub port: u16,
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
        })
    }
}

fn required(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("Missing required env var: {key}"))
}
