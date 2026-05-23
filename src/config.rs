use std::env;
use std::sync::OnceLock;

pub const DATA_DIR: &str = "./data";

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
    pub base_url: String,
    pub wiki_url: String,
    pub forum_url: String,
    pub port: u16,
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
        let domain = env::var("DOMAIN").expect("DOMAIN must be set");
        let https_port: u16 = env::var("HTTPS_PORT")
            .expect("HTTPS_PORT must be set")
            .parse()
            .expect("HTTPS_PORT must be a valid port number");

        let base_url = public_url("www", &domain, https_port);
        let wiki_url = public_url("wiki", &domain, https_port);
        let forum_url = public_url("forum", &domain, https_port);

        Self {
            oidc_issuer_url: env::var("OIDC_ISSUER_URL").unwrap_or_else(|_| base_url.clone()),
            domain,
            https_port,
            base_url,
            wiki_url,
            forum_url,
            database_url: format!(
                "postgres://{}:{}@{}:{}/{}",
                env::var("POSTGRES_USER").expect("POSTGRES_USER must be set"),
                env::var("POSTGRES_PASSWORD").expect("POSTGRES_PASSWORD must be set"),
                env::var("POSTGRES_HOST").unwrap_or_else(|_| "localhost".into()),
                env::var("POSTGRES_PORT").unwrap_or_else(|_| "5432".into()),
                env::var("POSTGRES_DB").expect("POSTGRES_DB must be set"),
            ),
            port: env::var("APP_PORT")
                .expect("APP_PORT must be set")
                .parse()
                .expect("APP_PORT must be a valid port number"),
        }
    }
}
