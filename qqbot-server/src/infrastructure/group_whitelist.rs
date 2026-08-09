//! NapCat 群观察白名单的运行期快照与文件持久化。

use std::collections::HashSet;
#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use thiserror::Error;

use crate::config::WhitelistFile;

#[derive(Debug, Error)]
pub(crate) enum GroupWhitelistError {
    #[error("group whitelist is not configured")]
    NotConfigured,
    #[error("group whitelist contains an invalid group id")]
    InvalidGroup,
    #[error("group whitelist persistence failed")]
    Persistence,
}

pub struct GroupWhitelist {
    path: Option<PathBuf>,
    groups: RwLock<HashSet<i64>>,
}

impl GroupWhitelist {
    pub(crate) fn new(path: Option<PathBuf>, groups: HashSet<i64>) -> Self {
        Self {
            path,
            groups: RwLock::new(groups),
        }
    }

    pub(crate) fn contains(&self, group_id: i64) -> bool {
        group_id > 0
            && self
                .groups
                .read()
                .expect("group whitelist lock poisoned")
                .contains(&group_id)
    }

    pub(crate) fn snapshot(&self) -> HashSet<i64> {
        self.groups
            .read()
            .expect("group whitelist lock poisoned")
            .clone()
    }

    pub(crate) fn set(&self, group_id: i64, enabled: bool) -> Result<(), GroupWhitelistError> {
        if group_id <= 0 {
            return Err(GroupWhitelistError::InvalidGroup);
        }
        let Some(path) = &self.path else {
            return Err(GroupWhitelistError::NotConfigured);
        };
        let mut next = self.snapshot();
        if enabled {
            next.insert(group_id);
        } else {
            next.remove(&group_id);
        }
        persist(path, &next)?;
        *self.groups.write().expect("group whitelist lock poisoned") = next;
        Ok(())
    }
}

fn persist(path: &Path, groups: &HashSet<i64>) -> Result<(), GroupWhitelistError> {
    let parent = path.parent().ok_or(GroupWhitelistError::Persistence)?;
    std::fs::create_dir_all(parent).map_err(|_| GroupWhitelistError::Persistence)?;
    let mut ordered = groups.iter().copied().collect::<Vec<_>>();
    ordered.sort_unstable();
    let bytes = serde_json::to_vec_pretty(&WhitelistFile { groups: ordered })
        .map_err(|_| GroupWhitelistError::Persistence)?;
    let temp_path = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)
        .map_err(|_| GroupWhitelistError::Persistence)?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|_| GroupWhitelistError::Persistence)?;
    drop(file);
    if std::fs::rename(&temp_path, path).is_err() {
        // Windows 不允许 rename 覆盖现有目标。目标短暂缺失时系统仍 fail-closed。
        if path.exists() {
            std::fs::remove_file(path).map_err(|_| GroupWhitelistError::Persistence)?;
        }
        std::fs::rename(&temp_path, path).map_err(|_| GroupWhitelistError::Persistence)?;
    }
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| GroupWhitelistError::Persistence)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_whitelist_denies_groups_and_updates_after_durable_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("groups.json");
        let whitelist = GroupWhitelist::new(Some(path.clone()), HashSet::new());
        assert!(!whitelist.contains(42));
        whitelist.set(42, true).unwrap();
        assert!(whitelist.contains(42));
        whitelist.set(42, false).unwrap();
        assert!(!whitelist.contains(42));
        let saved: WhitelistFile =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(saved.groups.is_empty());
    }
}
