use std::env;
use std::sync::OnceLock;

static SITE_NAME: OnceLock<String> = OnceLock::new();
static WIKI_URL: OnceLock<String> = OnceLock::new();
static FORUM_URL: OnceLock<String> = OnceLock::new();

pub fn init_site_config(config: &Config) {
    SITE_NAME.set(config.domain.clone()).ok();
    WIKI_URL.set(config.wiki_url.clone()).ok();
    FORUM_URL.set(config.forum_url.clone()).ok();
}

pub trait SiteConfig {
    fn site_name(&self) -> &str {
        SITE_NAME.get().map(|s| s.as_str()).unwrap_or("localhost")
    }
    fn wiki_url(&self) -> &str {
        WIKI_URL.get().map(|s| s.as_str()).unwrap_or("#")
    }
    fn forum_url(&self) -> &str {
        FORUM_URL.get().map(|s| s.as_str()).unwrap_or("#")
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub domain: String,
    pub https_port: u16,
    pub database_url: String,
    pub secret_key: String,
    pub base_url: String,
    pub wiki_url: String,
    pub forum_url: String,
    pub port: u16,
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_user: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_from: Option<String>,
    pub data_dir: String,
    /// Used as the OIDC issuer and base for back-channel endpoints (token,
    /// userinfo, jwks). Defaults to `base_url`. In Docker set this to the
    /// internal service URL (e.g. `http://app:3000`) so MediaWiki/phpBB can
    /// reach the token endpoint without TLS/DNS issues.
    pub oidc_issuer_url: String,
}

fn public_url(subdomain: &str, domain: &str, port: u16) -> String {
    if port == 443 {
        format!("https://{subdomain}.{domain}")
    } else {
        format!("https://{subdomain}.{domain}:{port}")
    }
}

impl Config {
    pub fn from_env() -> Self {
        let domain = env::var("DOMAIN").unwrap_or_else(|_| "localhost".into());
        let https_port: u16 = env::var("HTTPS_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8443);

        let base_url = public_url("www", &domain, https_port);
        let wiki_url = public_url("wiki", &domain, https_port);
        let forum_url = public_url("forum", &domain, https_port);

        Self {
            oidc_issuer_url: env::var("OIDC_ISSUER_URL")
                .unwrap_or_else(|_| base_url.clone()),
            domain,
            https_port,
            base_url,
            wiki_url,
            forum_url,
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://vgindex:changeme@localhost:5432/vgindex".into()),
            secret_key: env::var("APP_SECRET_KEY")
                .unwrap_or_else(|_| "devsecretkey0000000000000000000000000000000000000000000000000000".into()),
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
