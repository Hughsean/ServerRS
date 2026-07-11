use thiserror::Error;

/// CLI 统一错误类型。
///
/// REPL 内的错误会被捕获并打印,不会导致进程退出;
/// 仅启动阶段(配置加载、首次登录)的致命错误才向上传播退出进程。
#[derive(Error, Debug)]
pub enum CliError {
    /// 认证失败:登录被拒、token 失效且无法刷新。
    #[error("认证失败: {0}")]
    Auth(String),

    /// 后端返回非 2xx(401 已在 client 层透明处理,不会走到这里)。
    #[error("请求失败 (状态码 {status}): {msg}")]
    Api { status: u16, msg: String },

    /// 网络错误:连接失败、超时。
    #[error("网络错误: {0}")]
    Network(#[from] reqwest::Error),

    /// IO 错误:token 缓存文件、终端 IO。
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 命令参数解析失败。
    #[error("参数解析失败: {0}")]
    Parse(String),

    /// JSON 序列化/反序列化失败。
    #[error("JSON 错误: {0}")]
    Serde(#[from] serde_json::Error),
}

/// 需要重新登录的信号由 `CliError::Auth("...需要重新登录...")` 表达,
/// 用 `is_relogin_required()` 判定,REPL 捕获后触发交互式登录,不退出进程。

impl CliError {
    /// 判断是否为"需要重新登录"的认证错误。
    pub fn is_relogin_required(&self) -> bool {
        matches!(self, CliError::Auth(msg) if msg.contains("需要重新登录"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_with_relogin_marker_is_detected() {
        let err = CliError::Auth("登录已过期,需要重新登录".into());
        assert!(err.is_relogin_required());
    }

    #[test]
    fn auth_error_without_marker_not_detected() {
        let err = CliError::Auth("用户名或密码错误".into());
        assert!(!err.is_relogin_required());
    }

    #[test]
    fn non_auth_error_not_detected() {
        let err = CliError::Parse("缺参数".into());
        assert!(!err.is_relogin_required());
    }
}
