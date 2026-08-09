//! 群白名单配置。只有白名单内的群消息才会被处理和持久化。
//!
//! 白名单文件是 JSON 格式：`{"groups": [671260344, ...]}`。
//! `whitelist_file` 为相对路径时以配置文件目录为基准。未配置、文件缺失或列表为空
//! 都表示不观察任何群；私聊始终由独立入站路径接收。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::ConfigError;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhitelistConfig {
    /// 白名单 JSON 文件路径。不配则不启用白名单过滤。
    pub whitelist_file: Option<PathBuf>,
}

impl WhitelistConfig {
    /// 解析白名单文件路径：相对路径以 `config_dir` 为基准，绝对路径直接使用。
    pub(crate) fn resolve_path(&self, config_dir: &Path) -> Option<PathBuf> {
        self.whitelist_file.as_ref().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                config_dir.join(path)
            }
        })
    }

    pub(super) fn validate(&self, config_dir: &Path) -> Result<(), ConfigError> {
        if let Some(path) = self.resolve_path(config_dir)
            && path.exists()
        {
            // 尝试解析 JSON，确认格式正确。
            let content = std::fs::read_to_string(&path).map_err(|error| {
                ConfigError::Invalid(format!("读取白名单文件失败 {}: {error}", path.display()))
            })?;
            let parsed: WhitelistFile = serde_json::from_str(&content).map_err(|error| {
                ConfigError::Invalid(format!(
                    "白名单文件 JSON 格式错误 {}: {error}",
                    path.display()
                ))
            })?;
            validate_groups(&parsed.groups)?;
        }
        Ok(())
    }

    /// 加载白名单群号集合。未配置或文件尚不存在时返回空集合（拒绝全部群）。
    /// 相对路径以 `config_dir` 为基准。
    ///
    /// 拒绝空数组（防止文件被改为空时 fail-open）和非正群号。
    pub fn load_groups(
        &self,
        config_dir: &Path,
    ) -> Result<std::collections::HashSet<i64>, ConfigError> {
        let Some(path) = self.resolve_path(config_dir) else {
            return Ok(std::collections::HashSet::new());
        };
        if !path.exists() {
            return Ok(std::collections::HashSet::new());
        }
        let content = std::fs::read_to_string(&path).map_err(|error| {
            ConfigError::Invalid(format!("读取白名单文件失败 {}: {error}", path.display()))
        })?;
        let parsed: WhitelistFile = serde_json::from_str(&content).map_err(|error| {
            ConfigError::Invalid(format!(
                "白名单文件 JSON 格式错误 {}: {error}",
                path.display()
            ))
        })?;
        validate_groups(&parsed.groups)?;
        Ok(parsed.groups.into_iter().collect())
    }
}

fn validate_groups(groups: &[i64]) -> Result<(), ConfigError> {
    if groups.iter().any(|group_id| *group_id <= 0) {
        return Err(ConfigError::Invalid(
            "白名单文件包含非法群号；群号必须为正整数".into(),
        ));
    }
    Ok(())
}

/// 白名单 JSON 文件结构。
#[derive(Debug, Deserialize, serde::Serialize)]
pub(crate) struct WhitelistFile {
    pub(crate) groups: Vec<i64>,
}
