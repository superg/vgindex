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
    fn url_encode(&self, value: &str) -> String {
        urlencoding::encode(value).into_owned()
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
    pub oidc_provider_url: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
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

        let default_base_url = public_url("www", &domain, https_port);
        let base_url = env::var("APP_PUBLIC_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(default_base_url);
        let wiki_url = public_url("wiki", &domain, https_port);
        let forum_url = public_url("forum", &domain, https_port);

        Self {
            domain,
            https_port,
            base_url,
            wiki_url,
            forum_url,
            oidc_provider_url: env::var("APP_OIDC_PROVIDER_URL")
                .unwrap_or_else(|_| "http://phpbb/app.php/oidc".into()),
            oidc_client_id: env::var("APP_OIDC_CLIENT_ID").unwrap_or_else(|_| "vgindex-app".into()),
            oidc_client_secret: env::var("APP_OIDC_CLIENT_SECRET")
                .unwrap_or_else(|_| "changeme-app-oidc".into()),
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
