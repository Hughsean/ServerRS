//! URL 与主机校验助手。
//!
//! NapCat 关闭鉴权后只允许本机回环地址，且 URL 不得携带凭据、查询或片段，
//! 避免无 Token 模式下通过查询串注入凭据。

use super::ConfigError;

/// 判断主机是否为本机回环地址。NapCat 无 Token 模式只允许回环，杜绝远程暴露。
pub(super) fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

pub(super) fn validate_url(value: &str, schemes: &[&str], field: &str) -> Result<(), ConfigError> {
    let url = url::Url::parse(value).map_err(|error| {
        ConfigError::Invalid(format!("{field} must be an absolute URL: {error}"))
    })?;
    if !schemes.contains(&url.scheme()) {
        return Err(ConfigError::Invalid(format!(
            "{field} must use one of these schemes: {}",
            schemes.join(", ")
        )));
    }
    Ok(())
}

pub(super) fn validate_loopback_url(
    value: &str,
    schemes: &[&str],
    field: &str,
) -> Result<(), ConfigError> {
    validate_url(value, schemes, field)?;
    let url = url::Url::parse(value).map_err(|error| {
        ConfigError::Invalid(format!("{field} must be an absolute URL: {error}"))
    })?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::Invalid(format!(
            "{field} must not contain credentials, query, or fragment"
        )));
    }
    if !url.host_str().is_some_and(is_loopback_host) {
        return Err(ConfigError::Invalid(format!(
            "{field} must use a loopback host because NapCat authentication is disabled"
        )));
    }
    Ok(())
}
