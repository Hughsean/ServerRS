use serde::{Deserialize, Serialize};

/// 创建签名的请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureCreateRequest {
    pub app_id: String,
    pub app_key: String,
    pub expires_in: Option<i64>,
}

/// 创建签名的响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureCreateResponse {
    pub token: String,
    pub issued_at: String,
    pub expires_at: String,
}

/// 验证签名的请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureVerifyRequest {
    pub token: String,
    pub app_key: String,
}

/// 验证签名的响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureVerifyResponse {
    pub valid: bool,
    pub app_id: Option<String>,
    pub issued_at: Option<String>,
    pub expires_at: Option<String>,
}
