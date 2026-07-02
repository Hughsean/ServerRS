use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct HttpGetRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout_secs: Option<u64>,
}

impl HttpGetRequest {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            timeout_secs: None,
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = Some(timeout_secs);
        self
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_redirection(&self) -> bool {
        (300..400).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Debug, Clone)]
pub struct HttpClientError {
    message: String,
}

impl HttpClientError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HttpClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HttpClientError {}

#[async_trait]
pub trait HttpClientT: Send + Sync {
    async fn get(&self, request: HttpGetRequest) -> Result<HttpResponse, HttpClientError>;
}
