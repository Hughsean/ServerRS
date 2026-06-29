use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::app::agent::agent_runtime::AgentTool;
use crate::domain::agent::AgentContext;
use crate::shared::config::WeatherPluginConfig;
use crate::shared::error::AppError;

pub struct GetWeatherTool {
    config: WeatherPluginConfig,
    http_client: reqwest::Client,
}

impl GetWeatherTool {
    pub fn new(config: WeatherPluginConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config,
            http_client,
        }
    }
}

#[async_trait]
impl AgentTool for GetWeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn description(&self) -> &str {
        "当用户询问实时天气、气温、是否带伞、穿衣建议且有地点可解析时使用。地点优先来自用户本轮参数，其次来自会话 location，最后来自默认配置。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "可选。查询地点，例如北京、上海、杭州。未提供时优先使用会话 location，其次使用默认地点。"
                },
                "lang": {
                    "type": "string",
                    "description": "可选。语言代码，默认 zh_CN。",
                    "default": "zh_CN"
                }
            }
        })
    }

    async fn execute(&self, context: &AgentContext, args: Value) -> Result<String, AppError> {
        tracing::info!(tool = "get_weather", "executing agent tool");

        // Check API key
        if self.config.api_key.trim().is_empty() {
            return Ok("未配置天气服务 API Key，无法查询。".to_string());
        }

        // Resolve location
        let query_location = resolve_location(&args, context, &self.config);
        let query_location = match query_location {
            Some(loc) => loc,
            None => return Ok("未提供有效的查询地点。".to_string()),
        };

        // Resolve lang
        let raw_lang = args.get("lang").and_then(|v| v.as_str()).unwrap_or("zh_CN");
        let normalized_lang = normalize_lang(raw_lang);

        // Step 1: City lookup
        let location_data = match self.city_lookup(&query_location, &normalized_lang).await {
            Ok(data) => data,
            Err(err_msg) => return Ok(err_msg),
        };

        // Extract location id and name
        let (location_id, location_name) = match extract_location_info(&location_data) {
            Ok(info) => info,
            Err(err_msg) => return Ok(err_msg),
        };

        // Step 2: Weather now
        let weather_data = match self.weather_now(&location_id, &normalized_lang).await {
            Ok(data) => data,
            Err(err_msg) => return Ok(err_msg),
        };

        // Extract now object
        let now = match weather_data.get("now") {
            Some(n) => n,
            None => return Ok("实时天气查询失败，请稍后再试。".to_string()),
        };

        // Build prompt
        let prompt = build_weather_prompt(&location_name, now, &normalized_lang);

        Ok(prompt)
    }
}

// ── Location resolution ───────────────────────────────────────────────────────

fn resolve_location(
    args: &Value,
    context: &AgentContext,
    config: &WeatherPluginConfig,
) -> Option<String> {
    // Priority 1: args.location
    if let Some(loc) = args.get("location").and_then(|v| v.as_str()) {
        let trimmed = loc.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    // Priority 2: context.location — try multiple field names
    if let Some(ref loc) = context.location {
        let location_str = extract_location_string(loc);
        if !location_str.is_empty() {
            return Some(location_str);
        }
    }

    // Priority 3: config.default_location
    let default = config.default_location.trim();
    if !default.is_empty() {
        return Some(default.to_string());
    }

    None
}

/// Extract a human-readable location string from a location JSON value.
/// Tries common field names in order of preference.
fn extract_location_string(location: &Value) -> String {
    let field_names = [
        "city",
        "district",
        "county",
        "name",
        "address",
        "formatted_address",
        "province",
    ];

    for field in &field_names {
        if let Some(v) = location.get(field).and_then(|v| v.as_str()) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }

    String::new()
}

// ── Lang normalization ────────────────────────────────────────────────────────

/// Normalize language codes to QWeather-compatible format.
///
/// Mapping:
/// - zh_cn / zh-CN  → zh-Hans
/// - zh_hk / zh-HK  → zh-Hant
/// - zh_tw / zh-TW  → zh-Hant
/// - en_us / en-US  → en
/// - en_gb / en-GB  → en
/// - ja_jp / ja-JP  → ja
/// - ko_kr / ko-KR  → ko
///
/// Fallback: replace underscores with hyphens.
pub fn normalize_lang(lang: &str) -> String {
    let lower = lang.trim().to_lowercase();

    match lower.as_str() {
        "zh_cn" | "zh-cn" | "zh-hans" => "zh-Hans".to_string(),
        "zh_hk" | "zh-hk" | "zh_tw" | "zh-tw" | "zh-hant" => "zh-Hant".to_string(),
        "en_us" | "en-us" | "en_gb" | "en-gb" | "en" => "en".to_string(),
        "ja_jp" | "ja-jp" | "ja" => "ja".to_string(),
        "ko_kr" | "ko-kr" => "ko".to_string(),
        other => other.replace('_', "-"),
    }
}

// ── URL sanitization for logging ──────────────────────────────────────────────

/// Strip the `key` query parameter from a URL so it is safe to log.
pub fn sanitize_url_for_log(url: &str) -> String {
    // Replace key=... until the next & or end of string
    let re = regex::Regex::new(r"key=[^&]*").unwrap_or_else(|_| {
        // Fallback: if regex fails, just return a placeholder
        regex::Regex::new(".").unwrap()
    });
    re.replace(url, "key=***").to_string()
}

/// Build a city lookup URL with the given parameters.
fn build_city_lookup_url(
    endpoint: &str,
    api_key: &str,
    location: &str,
    lang: Option<&str>,
) -> String {
    let mut url = format!(
        "{endpoint}?key={}&location={}",
        urlencoding::encode(api_key),
        urlencoding::encode(location),
    );
    if let Some(l) = lang {
        url.push_str(&format!("&lang={}", urlencoding::encode(l)));
    }
    url
}

/// Build a weather now URL with the given parameters.
fn build_weather_now_url(
    endpoint: &str,
    api_key: &str,
    location_id: &str,
    lang: Option<&str>,
) -> String {
    let mut url = format!(
        "{endpoint}?key={}&location={}",
        urlencoding::encode(api_key),
        urlencoding::encode(location_id),
    );
    if let Some(l) = lang {
        url.push_str(&format!("&lang={}", urlencoding::encode(l)));
    }
    url
}

// ── City lookup ───────────────────────────────────────────────────────────────

impl GetWeatherTool {
    async fn city_lookup(&self, location: &str, lang: &str) -> Result<Value, String> {
        let url = build_city_lookup_url(
            &self.config.city_lookup_endpoint,
            &self.config.api_key,
            location,
            if self.config.lang_query_enabled {
                Some(lang)
            } else {
                None
            },
        );

        match self.do_city_lookup_request(&url, location, lang).await {
            Ok(parsed) => return Ok(parsed),
            Err(err) => {
                // Check for lang param rejection → retry without lang
                if self.config.lang_query_enabled && is_lang_param_rejected(&err.body) {
                    tracing::info!(
                        tool = "get_weather",
                        location,
                        "lang param rejected by city lookup, retrying without lang"
                    );

                    let retry_url = build_city_lookup_url(
                        &self.config.city_lookup_endpoint,
                        &self.config.api_key,
                        location,
                        None,
                    );

                    return self
                        .do_city_lookup_request(&retry_url, location, lang)
                        .await
                        .map_err(|e| e.user_message);
                }

                return Err(err.user_message);
            }
        }
    }

    /// Perform a single city lookup HTTP request and parse the response.
    async fn do_city_lookup_request(
        &self,
        url: &str,
        location: &str,
        _lang: &str,
    ) -> Result<Value, CityLookupError> {
        let response = self
            .http_client
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(
                    tool = "get_weather",
                    endpoint = "city_lookup",
                    location,
                    error = %e,
                    url = %sanitize_url_for_log(url),
                    "city lookup request failed"
                );
                CityLookupError {
                    body: String::new(),
                    user_message: format!("地点查询失败，天气服务网络请求异常。"),
                }
            })?;

        let status = response.status();

        let body = response.text().await.map_err(|e| {
            tracing::warn!(
                tool = "get_weather",
                endpoint = "city_lookup",
                location,
                status = status.as_u16(),
                error = %e,
                "city lookup read body failed"
            );
            CityLookupError {
                body: String::new(),
                user_message: format!("地点查询失败，天气服务返回了无法解析的数据。"),
            }
        })?;

        let body_preview = body_preview(&body, 300);

        if !status.is_success() {
            tracing::warn!(
                tool = "get_weather",
                endpoint = "city_lookup",
                location,
                status = status.as_u16(),
                body_preview,
                "city lookup returned non-2xx"
            );
            return Err(CityLookupError {
                body,
                user_message: format!("地点查询失败，天气服务返回 HTTP {}。", status.as_u16()),
            });
        }

        let parsed: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    tool = "get_weather",
                    endpoint = "city_lookup",
                    location,
                    status = status.as_u16(),
                    body_preview,
                    error = %e,
                    "city lookup JSON parse failed"
                );
                return Err(CityLookupError {
                    body,
                    user_message: "地点查询失败，天气服务返回了无法解析的数据。".to_string(),
                });
            }
        };

        // Check QWeather code
        let code = parsed.get("code").and_then(|v| v.as_str()).unwrap_or("");
        if code != "200" {
            tracing::warn!(
                tool = "get_weather",
                endpoint = "city_lookup",
                location,
                status = status.as_u16(),
                qweather_code = code,
                body_preview,
                "city lookup returned non-200 QWeather code"
            );
            return Err(CityLookupError {
                body,
                user_message: format!("地点查询失败，天气服务返回错误码 {code}。"),
            });
        }

        // Check location array
        let locations = parsed.get("location").and_then(|v| v.as_array());
        match locations {
            Some(arr) if !arr.is_empty() => {}
            _ => {
                tracing::info!(
                    tool = "get_weather",
                    endpoint = "city_lookup",
                    location,
                    qweather_code = code,
                    "city lookup returned empty location array"
                );
                return Err(CityLookupError {
                    body,
                    user_message: format!("未找到与「{location}」匹配的地点，请确认名称是否正确。"),
                });
            }
        }

        // Check location[0].id
        let location_id = parsed
            .get("location")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|loc| loc.get("id"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        if location_id.is_none() {
            tracing::warn!(
                tool = "get_weather",
                endpoint = "city_lookup",
                location,
                qweather_code = code,
                "city lookup returned location without id"
            );
            return Err(CityLookupError {
                body,
                user_message: "地点信息不完整，无法查询天气。".to_string(),
            });
        }

        tracing::debug!(
            tool = "get_weather",
            endpoint = "city_lookup",
            location,
            status = status.as_u16(),
            qweather_code = code,
            "city lookup succeeded"
        );

        Ok(parsed)
    }

    async fn weather_now(&self, location_id: &str, lang: &str) -> Result<Value, String> {
        let url = build_weather_now_url(
            &self.config.weather_now_endpoint,
            &self.config.api_key,
            location_id,
            if self.config.lang_query_enabled {
                Some(lang)
            } else {
                None
            },
        );

        match self.do_weather_now_request(&url, location_id).await {
            Ok(parsed) => Ok(parsed),
            Err(err) => {
                // Check for lang param rejection → retry without lang
                if self.config.lang_query_enabled && is_lang_param_rejected(&err.body) {
                    tracing::info!(
                        tool = "get_weather",
                        endpoint = "weather_now",
                        location_id,
                        "lang param rejected by weather now, retrying without lang"
                    );

                    let retry_url = build_weather_now_url(
                        &self.config.weather_now_endpoint,
                        &self.config.api_key,
                        location_id,
                        None,
                    );

                    return self
                        .do_weather_now_request(&retry_url, location_id)
                        .await
                        .map_err(|e| e.user_message);
                }

                Err(err.user_message)
            }
        }
    }

    /// Perform a single weather now HTTP request and parse the response.
    async fn do_weather_now_request(
        &self,
        url: &str,
        location_id: &str,
    ) -> Result<Value, WeatherNowError> {
        let response = self
            .http_client
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(
                    tool = "get_weather",
                    endpoint = "weather_now",
                    location_id,
                    error = %e,
                    url = %sanitize_url_for_log(url),
                    "weather now request failed"
                );
                WeatherNowError {
                    body: String::new(),
                    user_message: "实时天气查询失败，天气服务网络请求异常。".to_string(),
                }
            })?;

        let status = response.status();

        let body = response.text().await.map_err(|e| {
            tracing::warn!(
                tool = "get_weather",
                endpoint = "weather_now",
                location_id,
                status = status.as_u16(),
                error = %e,
                "weather now read body failed"
            );
            WeatherNowError {
                body: String::new(),
                user_message: "实时天气查询失败，天气服务返回了无法解析的数据。".to_string(),
            }
        })?;

        let body_preview = body_preview(&body, 300);

        if !status.is_success() {
            tracing::warn!(
                tool = "get_weather",
                endpoint = "weather_now",
                location_id,
                status = status.as_u16(),
                body_preview,
                "weather now returned non-2xx"
            );
            return Err(WeatherNowError {
                body,
                user_message: format!("实时天气查询失败，天气服务返回 HTTP {}。", status.as_u16()),
            });
        }

        let parsed: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    tool = "get_weather",
                    endpoint = "weather_now",
                    location_id,
                    status = status.as_u16(),
                    body_preview,
                    error = %e,
                    "weather now JSON parse failed"
                );
                return Err(WeatherNowError {
                    body,
                    user_message: "实时天气查询失败，天气服务返回了无法解析的数据。".to_string(),
                });
            }
        };

        // Check QWeather code
        let code = parsed.get("code").and_then(|v| v.as_str()).unwrap_or("");
        if code != "200" {
            tracing::warn!(
                tool = "get_weather",
                endpoint = "weather_now",
                location_id,
                status = status.as_u16(),
                qweather_code = code,
                body_preview,
                "weather now returned non-200 QWeather code"
            );
            return Err(WeatherNowError {
                body,
                user_message: format!("实时天气查询失败，天气服务返回错误码 {code}。"),
            });
        }

        tracing::debug!(
            tool = "get_weather",
            endpoint = "weather_now",
            location_id,
            status = status.as_u16(),
            qweather_code = code,
            "weather now succeeded"
        );

        Ok(parsed)
    }
}

// ── Error types for internal request handling ─────────────────────────────────

struct CityLookupError {
    /// Raw response body (for lang-rejection inspection).
    body: String,
    /// User-facing error message.
    user_message: String,
}

struct WeatherNowError {
    /// Raw response body (for lang-rejection inspection).
    body: String,
    /// User-facing error message.
    user_message: String,
}

// ── Body preview helper ───────────────────────────────────────────────────────

/// Return the first `max_chars` characters of the body for safe logging.
fn body_preview(body: &str, max_chars: usize) -> String {
    if body.len() <= max_chars {
        body.to_string()
    } else {
        format!("{}…[truncated]", &body[..max_chars])
    }
}

// ── Location info extraction ──────────────────────────────────────────────────

fn extract_location_info(data: &Value) -> Result<(String, String), String> {
    let locations = data
        .get("location")
        .and_then(|v| v.as_array())
        .filter(|arr| !arr.is_empty());

    let locations = match locations {
        Some(arr) => arr,
        None => {
            return Err("未找到与查询匹配的地点，请确认名称是否正确。".to_string());
        }
    };

    let first = &locations[0];
    let location_id = first
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let location_id = match location_id {
        Some(id) => id.to_string(),
        None => return Err("地点信息不完整，无法查询天气。".to_string()),
    };

    let location_name = combine_location_name(first);

    Ok((location_id, location_name))
}

/// Build human-readable location name from city lookup response fields.
pub fn combine_location_name(location: &Value) -> String {
    let country = location
        .get("country")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let adm1 = location
        .get("adm1")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let name = location
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let mut parts: Vec<&str> = Vec::new();
    if let Some(c) = country {
        parts.push(c);
    }
    if let Some(a) = adm1 {
        parts.push(a);
    }
    if let Some(n) = name {
        parts.push(n);
    }

    if parts.is_empty() {
        "Unknown".to_string()
    } else {
        parts.join(" / ")
    }
}

// ── Lang param rejection detection ────────────────────────────────────────────

/// Check whether the API response indicates the `lang` parameter was rejected.
pub fn is_lang_param_rejected(body: &str) -> bool {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Check error.invalidParams contains "lang"
    if let Some(error) = parsed.get("error") {
        if let Some(invalid_params) = error.get("invalidParams") {
            if let Some(arr) = invalid_params.as_array() {
                return arr
                    .iter()
                    .any(|v| v.as_str().map(|s| s == "lang").unwrap_or(false));
            }
        }
    }

    // Check top-level invalidParams
    if let Some(invalid_params) = parsed.get("invalidParams") {
        if let Some(arr) = invalid_params.as_array() {
            return arr
                .iter()
                .any(|v| v.as_str().map(|s| s == "lang").unwrap_or(false));
        }
    }

    false
}

// ── Weather prompt builder ────────────────────────────────────────────────────

/// Build the intermediate prompt given to the LLM with weather data.
pub fn build_weather_prompt(location_name: &str, now: &Value, lang: &str) -> String {
    let obs_time = now.get("obsTime").and_then(|v| v.as_str()).unwrap_or("");
    let text = now.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let temp = now.get("temp").and_then(|v| v.as_str()).unwrap_or("");
    let feels_like = now.get("feelsLike").and_then(|v| v.as_str()).unwrap_or("");
    let humidity = now.get("humidity").and_then(|v| v.as_str()).unwrap_or("");
    let wind_dir = now.get("windDir").and_then(|v| v.as_str()).unwrap_or("");
    let wind_scale = now.get("windScale").and_then(|v| v.as_str()).unwrap_or("");
    let precip = now.get("precip").and_then(|v| v.as_str()).unwrap_or("");
    let pressure = now.get("pressure").and_then(|v| v.as_str()).unwrap_or("");
    let vis = now.get("vis").and_then(|v| v.as_str()).unwrap_or("");

    format!(
        "请参考以下实时天气数据，用{lang}向用户说明当前天气状况，并给出生活建议：\n\
         地点: {location_name}\n\
         观测时间: {obs_time}\n\
         天气概况: {text}\n\
         气温: {temp}°C，体感 {feels_like}°C\n\
         相对湿度: {humidity}%\n\
         风向风力: {wind_dir}，风力等级 {wind_scale}级\n\
         小时降水量: {precip} mm\n\
         气压: {pressure} hPa\n\
         能见度: {vis} km\n\
         请同时简要建议是否需要携带雨具或增减衣物，并保持语气自然亲切。"
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── API key / location tests ───────────────────────────────────────

    #[test]
    fn missing_api_key_returns_message() {
        let config = WeatherPluginConfig {
            api_key: String::new(),
            default_location: String::new(),
            ..WeatherPluginConfig::default()
        };
        let tool = GetWeatherTool::new(config);
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(&ctx, json!({"lang": "zh_CN"})))
            .unwrap();
        assert!(result.contains("未配置天气服务 API Key"));
    }

    #[test]
    fn missing_location_returns_message() {
        let config = WeatherPluginConfig {
            api_key: "test-key".to_string(),
            default_location: String::new(),
            ..WeatherPluginConfig::default()
        };
        let tool = GetWeatherTool::new(config);
        let ctx = test_context();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(tool.execute(&ctx, json!({"lang": "zh_CN"})))
            .unwrap();
        assert!(result.contains("未提供有效的查询地点"));
    }

    #[test]
    fn uses_default_location_when_no_args_location() {
        let config = WeatherPluginConfig {
            default_location: "合肥".to_string(),
            ..WeatherPluginConfig::default()
        };
        let args = json!({"lang": "zh_CN"});
        let ctx = test_context();
        let loc = resolve_location(&args, &ctx, &config);
        assert_eq!(loc, Some("合肥".to_string()));
    }

    #[test]
    fn uses_args_location_over_default() {
        let config = WeatherPluginConfig {
            default_location: "北京".to_string(),
            ..WeatherPluginConfig::default()
        };
        let args = json!({"location": "上海", "lang": "zh_CN"});
        let ctx = test_context();
        let loc = resolve_location(&args, &ctx, &config);
        assert_eq!(loc, Some("上海".to_string()));
    }

    #[test]
    fn uses_context_location_city_field() {
        let config = WeatherPluginConfig {
            default_location: "北京".to_string(),
            ..WeatherPluginConfig::default()
        };
        let args = json!({"lang": "zh_CN"});
        let mut ctx = test_context();
        ctx.location = Some(json!({"city": "杭州", "province": "浙江"}));
        let loc = resolve_location(&args, &ctx, &config);
        assert_eq!(loc, Some("杭州".to_string()));
    }

    #[test]
    fn falls_back_to_default_when_context_location_has_no_valid_field() {
        let config = WeatherPluginConfig {
            default_location: "合肥".to_string(),
            ..WeatherPluginConfig::default()
        };
        let args = json!({"lang": "zh_CN"});
        let mut ctx = test_context();
        ctx.location = Some(json!({"foo": "bar"}));
        let loc = resolve_location(&args, &ctx, &config);
        assert_eq!(loc, Some("合肥".to_string()));
    }

    // ── Lang normalization ─────────────────────────────────────────────

    #[test]
    fn normalizes_lang_codes() {
        assert_eq!(normalize_lang("zh_CN"), "zh-Hans");
        assert_eq!(normalize_lang("zh-CN"), "zh-Hans");
        assert_eq!(normalize_lang("zh_HK"), "zh-Hant");
        assert_eq!(normalize_lang("zh-hk"), "zh-Hant");
        assert_eq!(normalize_lang("zh_TW"), "zh-Hant");
        assert_eq!(normalize_lang("zh-tw"), "zh-Hant");
        assert_eq!(normalize_lang("en_US"), "en");
        assert_eq!(normalize_lang("en-us"), "en");
        assert_eq!(normalize_lang("en_GB"), "en");
        assert_eq!(normalize_lang("ja_JP"), "ja");
        assert_eq!(normalize_lang("ja-jp"), "ja");
        assert_eq!(normalize_lang("ko_kr"), "ko");
        assert_eq!(normalize_lang("ko-KR"), "ko");
        // Fallback: underscore → hyphen
        assert_eq!(normalize_lang("fr_FR"), "fr-fr");
        assert_eq!(normalize_lang("de_DE"), "de-de");
    }

    // ── Location name combination ──────────────────────────────────────

    #[test]
    fn combines_location_name() {
        let data = json!({
            "country": "中国",
            "adm1": "安徽省",
            "name": "合肥"
        });
        assert_eq!(combine_location_name(&data), "中国 / 安徽省 / 合肥");
    }

    #[test]
    fn combines_location_name_with_missing_fields() {
        let data = json!({
            "adm1": "安徽省",
            "name": "合肥"
        });
        assert_eq!(combine_location_name(&data), "安徽省 / 合肥");
    }

    #[test]
    fn combines_location_name_name_only() {
        let data = json!({
            "name": "合肥"
        });
        assert_eq!(combine_location_name(&data), "合肥");
    }

    // ── City lookup response parsing ───────────────────────────────────

    #[test]
    fn parses_location_response_success() {
        let data = json!({
            "code": "200",
            "location": [
                {
                    "id": "101220101",
                    "name": "合肥",
                    "adm1": "安徽省",
                    "country": "中国"
                }
            ]
        });
        let (id, name) = extract_location_info(&data).unwrap();
        assert_eq!(id, "101220101");
        assert_eq!(name, "中国 / 安徽省 / 合肥");
    }

    #[test]
    fn city_lookup_empty_location_returns_not_found() {
        let data = json!({
            "code": "200",
            "location": []
        });
        let result = extract_location_info(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("未找到与查询匹配的地点"));
    }

    #[test]
    fn city_lookup_missing_location_id_returns_incomplete() {
        let data = json!({
            "code": "200",
            "location": [
                {
                    "name": "Unknown"
                }
            ]
        });
        let result = extract_location_info(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("地点信息不完整"));
    }

    // ── Weather now response parsing ───────────────────────────────────

    #[test]
    fn parses_weather_response_success() {
        let now = json!({
            "obsTime": "2024-01-15T10:00+08:00",
            "text": "晴",
            "temp": "15",
            "feelsLike": "12",
            "humidity": "45",
            "windDir": "北风",
            "windScale": "3",
            "precip": "0.0",
            "pressure": "1013",
            "vis": "10"
        });
        let prompt = build_weather_prompt("合肥", &now, "zh-Hans");
        assert!(prompt.contains("地点: 合肥"));
        assert!(prompt.contains("天气概况: 晴"));
        assert!(prompt.contains("气温: 15°C"));
        assert!(prompt.contains("体感 12°C"));
        assert!(prompt.contains("相对湿度: 45%"));
        assert!(prompt.contains("北风"));
        assert!(prompt.contains("zh-Hans"));
    }

    // ── Lang param rejection detection ─────────────────────────────────

    #[test]
    fn detects_lang_param_rejected() {
        let body = r#"{"error":{"invalidParams":["lang"]}}"#;
        assert!(is_lang_param_rejected(body));
    }

    #[test]
    fn detects_lang_param_rejected_top_level() {
        let body = r#"{"invalidParams":["lang"]}"#;
        assert!(is_lang_param_rejected(body));
    }

    #[test]
    fn lang_param_not_rejected_when_absent() {
        let body = r#"{"code":"200","location":[{"id":"101"}]}"#;
        assert!(!is_lang_param_rejected(body));
    }

    #[test]
    fn lang_param_not_rejected_on_invalid_json() {
        assert!(!is_lang_param_rejected("not json"));
    }

    // ── Weather prompt ─────────────────────────────────────────────────

    #[test]
    fn builds_weather_prompt() {
        let now = json!({
            "obsTime": "2024-06-12T14:00+08:00",
            "text": "多云",
            "temp": "25",
            "feelsLike": "24",
            "humidity": "60",
            "windDir": "东南风",
            "windScale": "2",
            "precip": "0.0",
            "pressure": "1012",
            "vis": "15"
        });
        let prompt = build_weather_prompt("中国 / 安徽省 / 合肥", &now, "zh-Hans");
        assert!(prompt.contains("中国 / 安徽省 / 合肥"));
        assert!(prompt.contains("多云"));
        assert!(prompt.contains("25°C"));
        assert!(prompt.contains("携带雨具或增减衣物"));
        assert!(prompt.contains("zh-Hans"));
    }

    #[test]
    fn builds_weather_prompt_with_empty_fields() {
        let now = json!({});
        let prompt = build_weather_prompt("Test", &now, "en");
        // Should not panic with empty fields
        assert!(prompt.contains("地点: Test"));
    }

    // ── URL sanitization ───────────────────────────────────────────────

    #[test]
    fn sanitize_url_hides_api_key() {
        let url = "https://api.example.com/geo/v2/city/lookup?key=abc123secret&location=Hefei";
        let sanitized = sanitize_url_for_log(url);
        assert!(
            !sanitized.contains("abc123secret"),
            "should hide key value, got: {sanitized}"
        );
        assert!(
            sanitized.contains("key=***"),
            "should replace key with ***, got: {sanitized}"
        );
        assert!(
            sanitized.contains("location=Hefei"),
            "should keep location param, got: {sanitized}"
        );
    }

    #[test]
    fn sanitize_url_key_only_param() {
        let url = "https://api.example.com/weather/now?key=secret123";
        let sanitized = sanitize_url_for_log(url);
        assert!(!sanitized.contains("secret123"));
        assert!(sanitized.contains("key=***"));
    }

    #[test]
    fn sanitize_url_no_key_param() {
        let url = "https://api.example.com/health";
        let sanitized = sanitize_url_for_log(url);
        assert_eq!(sanitized, url);
    }

    // ── Body preview ───────────────────────────────────────────────────

    #[test]
    fn body_preview_truncates_long_body() {
        let body = "A".repeat(500);
        let preview = body_preview(&body, 300);
        assert_eq!(preview.len(), 300 + "…[truncated]".len());
        assert!(preview.ends_with("…[truncated]"));
    }

    #[test]
    fn body_preview_returns_short_body_as_is() {
        let body = "short body";
        let preview = body_preview(body, 300);
        assert_eq!(preview, body);
    }

    // ── Helpers ────────────────────────────────────────────────────────

    fn test_context() -> AgentContext {
        use crate::domain::agent::ToolDefinition;

        AgentContext {
            user_id: 1,
            conversation_id: None,
            recent_messages: vec![],
            summary: None,
            memories: vec![],
            rag_chunks: vec![],
            fresh_chunks: vec![],
            user_profile: None,
            location: None,
            tools: vec![ToolDefinition {
                name: "get_weather".into(),
                description: "get weather".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "location": {"type": "string"},
                        "lang": {"type": "string", "default": "zh_CN"}
                    }
                }),
            }],
        }
    }
}
