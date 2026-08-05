//! NapCat 事件到个人秘书入站边界的映射与白名单过滤。
//!
//! 统一身份后先幂等落库（由 ingestion Worker 完成），只有新事件才进入后续处理。
//! 群白名单非空时只处理白名单内群消息与群撤回；为空表示不启用白名单（放行所有群）。

use std::sync::Arc;

use qqbot::napcat::{NapCatError, NapCatEvent, NapCatEventHandler};

use super::realtime_spool_runtime::RealtimeSpoolAdmissionQueue;
use crate::inbound::NapCatInboundMapper;
use crate::ingestion_worker::IngestionQueue;
use crate::recall::RecallHandler;

pub enum MessageAdmission {
    Memory(IngestionQueue),
    Durable(RealtimeSpoolAdmissionQueue),
}

impl MessageAdmission {
    fn try_admit(&self, message: personal_secretary::InboundMessageEnvelope) -> Result<(), String> {
        match self {
            Self::Memory(queue) => queue
                .try_enqueue(message)
                .map_err(|error| error.to_string()),
            Self::Durable(queue) => queue.try_admit(message).map_err(|error| error.to_string()),
        }
    }
}

/// 个人秘书入站边界：统一身份后先幂等落库，只有新事件才允许进入后续处理。
pub struct PersonalSecretaryInboundHandler {
    pub mapper: NapCatInboundMapper,
    pub admission: MessageAdmission,
    /// 群白名单。非空时只处理白名单内群的消息；为空表示不启用白名单（放行所有群）。
    pub group_whitelist: Arc<std::collections::HashSet<i64>>,
    /// 撤回处理器。可选：未装配时撤回通知只记录日志。
    pub recall_handler: Option<Arc<RecallHandler>>,
}

/// 判断群消息/群撤回是否应被处理。白名单为空时放行所有群（不启用过滤）。
/// 这是一个纯函数，便于单元测试。
fn should_accept_group_message(group_id: i64, whitelist: &std::collections::HashSet<i64>) -> bool {
    whitelist.is_empty() || whitelist.contains(&group_id)
}

#[async_trait::async_trait]
impl NapCatEventHandler for PersonalSecretaryInboundHandler {
    async fn handle(&self, event: NapCatEvent) -> Result<(), NapCatError> {
        match event {
            NapCatEvent::GroupMessage(event) => {
                if !should_accept_group_message(event.group_id, &self.group_whitelist) {
                    tracing::debug!(group_id = event.group_id, "群消息不在白名单内，跳过");
                    return Ok(());
                }
                self.admission
                    .try_admit(self.mapper.map_group(event)?)
                    .map_err(|error| NapCatError::Handler(error.to_string()))?
            }
            NapCatEvent::PrivateMessage(event) => self
                .admission
                .try_admit(self.mapper.map_private(event)?)
                .map_err(|error| NapCatError::Handler(error.to_string()))?,
            NapCatEvent::GroupMemberIncrease(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                "NapCat 入群通知已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::GroupMemberDecrease(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                sub_type = %event.sub_type,
                "NapCat 退群通知已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::Poke(event) => tracing::info!(
                group_id = event.group_id,
                user_id = event.user_id,
                target_id = ?event.target_id,
                "NapCat 戳一戳通知已接收；QQBot 业务尚未接入"
            ),
            NapCatEvent::GroupRecall(event) => {
                if !should_accept_group_message(event.group_id, &self.group_whitelist) {
                    tracing::debug!(group_id = event.group_id, "群撤回不在白名单内，跳过");
                    return Ok(());
                }
                tracing::info!(
                    group_id = event.group_id,
                    user_id = event.user_id,
                    operator_id = ?event.operator_id,
                    "NapCat 群消息撤回通知已接收"
                );
                if let Some(handler) = &self.recall_handler {
                    // 只做非阻塞入队；失败向上返回，禁止吞掉。
                    handler.handle_group_recall(event).await?;
                }
            }
            NapCatEvent::FriendRecall(event) => {
                tracing::info!(user_id = event.user_id, "NapCat 好友消息撤回通知已接收");
                if let Some(handler) = &self.recall_handler {
                    handler.handle_friend_recall(event).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_allows_listed_group() {
        let mut whitelist = std::collections::HashSet::new();
        whitelist.insert(671260344);
        assert!(should_accept_group_message(671260344, &whitelist));
    }

    #[test]
    fn whitelist_rejects_non_listed_group() {
        let mut whitelist = std::collections::HashSet::new();
        whitelist.insert(671260344);
        assert!(!should_accept_group_message(999999999, &whitelist));
    }
}
