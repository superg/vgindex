use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub secret_key: String,
    pub base_url: String,
    pub port: u16,
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_user: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from: Option<String>,
    pub data_dir: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://vgindex:changeme@localhost:5432/vgindex".into()),
            secret_key: env::var("APP_SECRET_KEY")
                .unwrap_or_else(|_| "devsecretkey0000000000000000000000000000000000000000000000000000".into()),
            base_url: env::var("APP_BASE_URL").unwrap_or_else(|_| "http://localhost:3000".into()),
            port: env::var("APP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            smtp_host: env::var("SMTP_HOST").ok().filter(|s| !s.is_empty()),
            smtp_port: env::var("SMTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(587),
            smtp_user: env::var("SMTP_USER").ok().filter(|s| !s.is_empty()),
            smtp_password: env::var("SMTP_PASSWORD").ok().filter(|s| !s.is_empty()),
            smtp_from: env::var("SMTP_FROM").ok().filter(|s| !s.is_empty()),
            data_dir: env::var("DATA_DIR").unwrap_or_else(|_| "./data".into()),
        }
    }
}
