use std::env;
use std::sync::OnceLock;

pub const DATA_DIR: &str = "./data";

static SITE_NAME: OnceLock<String> = OnceLock::new();
static WIKI_URL: OnceLock<String> = OnceLock::new();
static FORUM_URL: OnceLock<String> = OnceLock::new();

pub fn init_site_config(config: &Config) {
    SITE_NAME.set(config.site_name.clone()).ok();
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
    pub site_name: String,
    pub database_url: String,
    pub site_url: String,
    pub base_url: String,
    pub wiki_url: String,
    pub forum_url: String,
    pub news_feed_url: String,
    pub port: u16,
    pub oidc_provider_url: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn trim_url(value: String) -> String {
    value.trim().trim_end_matches('/').to_string()
}

pub fn host_from_url(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or(authority)
        .trim_start_matches("www.")
        .to_string()
}

impl Config {
    pub fn from_env() -> Self {
        let base_url = trim_url(
            env_nonempty("APP_PUBLIC_URL").unwrap_or_else(|| "http://redump.test:18000".into()),
        );
        let site_url = trim_url(env_nonempty("SITE_APEX_URL").unwrap_or_else(|| base_url.clone()));
        let forum_url = trim_url(
            env_nonempty("PHPBB_PUBLIC_URL")
                .unwrap_or_else(|| "http://forum.redump.test:18000".into()),
        );
        let wiki_url = trim_url(
            env_nonempty("MEDIAWIKI_PUBLIC_URL")
                .unwrap_or_else(|| "http://wiki.redump.test:18000".into()),
        );
        let oidc_provider_url = trim_url(
            env_nonempty("OIDC_PROVIDER_URL")
                .unwrap_or_else(|| format!("{}/app.php/oidc", forum_url)),
        );
        let news_feed_url = trim_url(
            env_nonempty("NEWS_FEED_URL")
                .unwrap_or_else(|| format!("{}/feed.php?mode=news", forum_url)),
        );
        let site_name = env_nonempty("SITE_NAME").unwrap_or_else(|| host_from_url(&base_url));

        Self {
            site_name,
            site_url,
            base_url,
            wiki_url,
            forum_url,
            news_feed_url,
            oidc_provider_url,
            oidc_client_id: env::var("APP_OIDC_CLIENT_ID").unwrap_or_else(|_| "vgindex-app".into()),
            oidc_client_secret: env::var("APP_OIDC_CLIENT_SECRET")
                .unwrap_or_else(|_| "changeme-app-oidc".into()),
            database_url: format!(
                "postgres://{}:{}@{}:{}/{}",
                env::var("POSTGRES_USER").expect("POSTGRES_USER must be set"),
                env::var("POSTGRES_PASSWORD").expect("POSTGRES_PASSWORD must be set"),
                env::var("POSTGRES_HOST").unwrap_or_else(|_| "localhost".into()),
                env_nonempty("POSTGRES_PORT")
                    .or_else(|| env_nonempty("POSTGRES_DIRECT_PORT"))
                    .unwrap_or_else(|| "5432".into()),
                env::var("POSTGRES_DB").expect("POSTGRES_DB must be set"),
            ),
            port: env::var("APP_PORT")
                .expect("APP_PORT must be set")
                .parse()
                .expect("APP_PORT must be a valid port number"),
        }
    }
}
