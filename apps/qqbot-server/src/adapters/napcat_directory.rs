//! NapCat 只读目录 API 到 `DirectorySourceT` 的适配器。

use std::sync::Arc;

use async_trait::async_trait;
use personal_secretary::{
    DirectoryListEntry, DirectorySourceError, DirectorySourceT, ScopeBoundary, ScopeKind,
    SourceAccountRef,
};
use qqbot::napcat::{NapCatDirectoryReadT, NapCatError};

/// 把 NapCat 只读列表结果转换为协议无关目录条目。
pub struct NapCatDirectorySource {
    client: Arc<dyn NapCatDirectoryReadT>,
}

impl NapCatDirectorySource {
    pub fn new(client: Arc<dyn NapCatDirectoryReadT>) -> Self {
        Self { client }
    }
}

fn map_napcat_error(error: NapCatError) -> DirectorySourceError {
    let message = error.to_string();
    match &error {
        NapCatError::Protocol(detail)
            if detail.contains("oversized") || detail.contains("exceeds") =>
        {
            DirectorySourceError::Oversized(message)
        }
        NapCatError::Connection(detail)
            if detail.contains("timeout") || detail.contains("Timeout") =>
        {
            DirectorySourceError::Timeout(message)
        }
        NapCatError::Connection(_) => DirectorySourceError::Transient(message),
        NapCatError::Protocol(detail)
            if detail.contains("retcode") || detail.contains("unavailable") =>
        {
            DirectorySourceError::Unavailable(message)
        }
        NapCatError::Protocol(_) => DirectorySourceError::Malformed(message),
        _ => DirectorySourceError::Transient(message),
    }
}

#[async_trait]
impl DirectorySourceT for NapCatDirectorySource {
    async fn list_friends(
        &self,
        _account: &SourceAccountRef,
    ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError> {
        let friends = self
            .client
            .get_friend_list()
            .await
            .map_err(map_napcat_error)?;
        Ok(friends
            .into_iter()
            .map(|friend| DirectoryListEntry {
                platform_id: friend.user_id.to_string(),
                display_name: if friend.remark.is_empty() {
                    (!friend.nickname.is_empty()).then_some(friend.nickname)
                } else {
                    Some(friend.remark)
                },
                boundary: None,
                kind_hint: ScopeKind::Friend,
            })
            .collect())
    }

    async fn list_groups(
        &self,
        _account: &SourceAccountRef,
    ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError> {
        let groups = self
            .client
            .get_group_list()
            .await
            .map_err(map_napcat_error)?;
        Ok(groups
            .into_iter()
            .map(|group| DirectoryListEntry {
                platform_id: group.group_id.to_string(),
                display_name: (!group.group_name.is_empty()).then_some(group.group_name),
                boundary: None,
                kind_hint: ScopeKind::Group,
            })
            .collect())
    }

    async fn list_recent_contacts(
        &self,
        _account: &SourceAccountRef,
    ) -> Result<Vec<DirectoryListEntry>, DirectorySourceError> {
        let contacts = self
            .client
            .get_recent_contact()
            .await
            .map_err(map_napcat_error)?;
        Ok(contacts
            .into_iter()
            .map(|contact| DirectoryListEntry {
                platform_id: contact.peer_uin,
                display_name: (!contact.peer_name.is_empty()).then_some(contact.peer_name),
                boundary: (!contact.msg_time.is_empty())
                    .then(|| ScopeBoundary::new(String::new(), contact.msg_time)),
                kind_hint: if contact.chat_type == 2 {
                    ScopeKind::Group
                } else {
                    ScopeKind::Friend
                },
            })
            .collect())
    }
}
