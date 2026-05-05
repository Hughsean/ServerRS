use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

use crate::domain::llm::tools::{LlmTool, ToolExecutionContext, ToolOutcome};

/// Weather tool powered by the free [Open-Meteo](https://open-meteo.com/) API.
///
/// Two-step lookup:
/// 1. Geocode city name → latitude / longitude
/// 2. Fetch current weather from the forecast endpoint
///
/// No API key required.
pub struct GetWeatherTool {
    client: reqwest::Client,
}

impl GetWeatherTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("weather reqwest client"),
        }
    }
}

#[async_trait]
impl LlmTool for GetWeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn tool_definition(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Query current weather conditions for a city. Use when the user asks about weather, temperature, or climate in a location.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "The city name, e.g. 'Beijing', 'London', 'New York'"
                        }
                    },
                    "required": ["city"]
                }
            }
        })
    }

    async fn invoke(&self, _context: &mut ToolExecutionContext, arguments: &Value) -> ToolOutcome {
        let city = arguments
            .get("city")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match fetch_weather(&self.client, city).await {
            Ok(summary) => ToolOutcome::continue_(summary),
            Err(err) => {
                warn!(city = %city, error = %err, "weather lookup failed");
                ToolOutcome::continue_(format!(
                    "Sorry, unable to fetch weather for {city} right now ({err})."
                ))
            }
        }
    }
}

// ── Open-Meteo API types ──

#[derive(Debug, Deserialize)]
struct GeocodingResult {
    results: Option<Vec<GeocodingLocation>>,
}

#[derive(Debug, Deserialize)]
struct GeocodingLocation {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeatherResponse {
    current_weather: Option<CurrentWeather>,
}

#[derive(Debug, Deserialize)]
struct CurrentWeather {
    temperature: f64,
    windspeed: f64,
    #[serde(rename = "weathercode")]
    weather_code: u8,
    humidity: Option<f64>,
}

// ── Fetch helpers ──

async fn fetch_weather(client: &reqwest::Client, city: &str) -> Result<String, String> {
    // 1. Geocode
    let geo_url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en",
        urlencoding(city)
    );
    let geo: GeocodingResult = client
        .get(&geo_url)
        .send()
        .await
        .map_err(|e| format!("geocoding request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("geocoding parse: {e}"))?;

    let loc = geo
        .results
        .and_then(|mut v| {
            if v.is_empty() {
                None
            } else {
                Some(v.remove(0))
            }
        })
        .ok_or_else(|| format!("city '{city}' not found"))?;

    // 2. Weather
    let weather_url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}&current_weather=true&timezone=auto",
        loc.latitude, loc.longitude
    );
    let weather: WeatherResponse = client
        .get(&weather_url)
        .send()
        .await
        .map_err(|e| format!("weather request failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("weather parse: {e}"))?;

    let w = weather
        .current_weather
        .ok_or_else(|| "no current weather data".to_string())?;

    let condition = weather_code_description(w.weather_code);
    let country = loc.country.as_deref().unwrap_or("");

    let mut summary = format!(
        "Weather in {city}{country_info}: {condition}, {temp:.0}°C, wind {wind:.1} km/h",
        city = loc.name,
        country_info = if country.is_empty() {
            String::new()
        } else {
            format!(", {country}")
        },
        condition = condition,
        temp = w.temperature,
        wind = w.windspeed,
    );

    if let Some(h) = w.humidity {
        summary.push_str(&format!(", humidity {:.0}%", h));
    }

    Ok(summary)
}

/// Manual url-encode so we don't need a whole crate for one function.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// Map WMO weather codes to human-readable descriptions.
fn weather_code_description(code: u8) -> &'static str {
    match code {
        0 => "Clear",
        1 | 2 => "Partly Cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing Drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing Rain",
        71 | 73 | 75 => "Snow",
        77 => "Snow Grains",
        80 | 81 | 82 => "Rain Showers",
        85 | 86 => "Snow Showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with Hail",
        _ => "Unknown",
    }
}
