use serde::Deserialize;

use super::default_true;

// ── PluginsConfig ──

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PluginsConfig {
    #[serde(default)]
    pub weather: WeatherPluginConfig,
    #[serde(default)]
    pub news: NewsPluginConfig,
    #[serde(default)]
    pub web_search: WebSearchPluginConfig,
    #[serde(default)]
    pub fetch_web_content: FetchWebContentPluginConfig,
    #[serde(default)]
    pub baidu_baike: BaiduBaikePluginConfig,
}

// ── WeatherPluginConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct WeatherPluginConfig {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub default_location: String,
    #[serde(default = "default_weather_city_lookup_endpoint")]
    pub city_lookup_endpoint: String,
    #[serde(default = "default_weather_now_endpoint")]
    pub weather_now_endpoint: String,
    #[serde(default)]
    pub lang_query_enabled: bool,
}

impl Default for WeatherPluginConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            default_location: String::new(),
            city_lookup_endpoint: default_weather_city_lookup_endpoint(),
            weather_now_endpoint: default_weather_now_endpoint(),
            lang_query_enabled: false,
        }
    }
}

fn default_weather_city_lookup_endpoint() -> String {
    "https://mk4ky3n4am.re.qweatherapi.com/geo/v2/city/lookup".into()
}
fn default_weather_now_endpoint() -> String {
    "https://mk4ky3n4am.re.qweatherapi.com/v7/weather/now".into()
}

// ── NewsPluginConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct NewsPluginConfig {
    #[serde(default = "default_news_default_rss_url")]
    pub default_rss_url: String,
    #[serde(default = "default_news_society_url")]
    pub society_url: String,
    #[serde(default = "default_news_world_url")]
    pub world_url: String,
    #[serde(default = "default_news_finance_url")]
    pub finance_url: String,
}

impl Default for NewsPluginConfig {
    fn default() -> Self {
        Self {
            default_rss_url: default_news_default_rss_url(),
            society_url: default_news_society_url(),
            world_url: default_news_world_url(),
            finance_url: default_news_finance_url(),
        }
    }
}

fn default_news_default_rss_url() -> String {
    "https://www.chinanews.com.cn/rss/society.xml".into()
}
fn default_news_society_url() -> String {
    "https://www.chinanews.com.cn/rss/society.xml".into()
}
fn default_news_world_url() -> String {
    "https://www.chinanews.com.cn/rss/world.xml".into()
}
fn default_news_finance_url() -> String {
    "https://www.chinanews.com.cn/rss/finance.xml".into()
}

// ── WebSearchPluginConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct WebSearchPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_search_timeout")]
    pub timeout_secs: u64,
}

impl Default for WebSearchPluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_secs: default_web_search_timeout(),
        }
    }
}

fn default_web_search_timeout() -> u64 {
    10
}

// ── FetchWebContentPluginConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct FetchWebContentPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_url: String,
}

impl Default for FetchWebContentPluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            proxy_url: String::new(),
        }
    }
}

// ── BaiduBaikePluginConfig ──

#[derive(Debug, Clone, Deserialize)]
pub struct BaiduBaikePluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_url: String,
}

impl Default for BaiduBaikePluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            proxy_url: String::new(),
        }
    }
}
