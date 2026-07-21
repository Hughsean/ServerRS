use std::time::Duration;

use async_trait::async_trait;

use crate::domain::http::{HttpClientError, HttpClientT, HttpGetRequest, HttpResponse};

#[derive(Debug, Clone, Copy)]
pub enum RedirectPolicy {
    None,
    Limited(usize),
}

#[derive(Debug, Clone)]
pub struct ReqwestHttpClientConfig {
    pub connect_timeout_secs: u64,
    pub timeout_secs: u64,
    pub redirect_policy: RedirectPolicy,
    pub proxy_url: Option<String>,
    pub no_proxy: bool,
}

pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new(config: ReqwestHttpClientConfig) -> Result<Self, HttpClientError> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
            .timeout(Duration::from_secs(config.timeout_secs));

        builder = match config.redirect_policy {
            RedirectPolicy::None => builder.redirect(reqwest::redirect::Policy::none()),
            RedirectPolicy::Limited(limit) => {
                builder.redirect(reqwest::redirect::Policy::limited(limit))
            }
        };

        if config.no_proxy {
            builder = builder.no_proxy();
        }

        if let Some(proxy_url) = config
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|proxy_url| !proxy_url.is_empty())
        {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|error| HttpClientError::new(error.to_string()))?;
            builder = builder.proxy(proxy);
        }

        let client = builder
            .build()
            .map_err(|error| HttpClientError::new(error.to_string()))?;

        Ok(Self { client })
    }
}

#[async_trait]
impl HttpClientT for ReqwestHttpClient {
    async fn get(&self, request: HttpGetRequest) -> Result<HttpResponse, HttpClientError> {
        let mut builder = self.client.get(&request.url);

        if let Some(timeout_secs) = request.timeout_secs {
            builder = builder.timeout(Duration::from_secs(timeout_secs));
        }

        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let response = builder
            .send()
            .await
            .map_err(|error| HttpClientError::new(error.to_string()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_string(), value.to_string()))
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|error| HttpClientError::new(error.to_string()))?
            .to_vec();

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}
