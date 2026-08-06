//! NapCat 事件到个人秘书入站边界的映射与白名单过滤。
//!
//! 统一身份后先幂等落库（由 ingestion Worker 完成），只有新事件才进入后续处理。
//! 群白名单非空时只处理白名单内群消息与群撤回；为空表示不启用白名单（放行所有群）。

use std::sync::Arc;

use personal_secretary::{ConversationKind, ConversationRef};
use qqbot::napcat::{NapCatError, NapCatEvent, NapCatEventHandler};

use super::realtime_spool_runtime::RealtimeSpoolAdmissionQueue;
use crate::inbound::NapCatInboundMapper;
use crate::ingestion_worker::IngestionQueue;
use crate::recall::RecallHandler;

pub enum MessageAdmission {
    Memory(IngestionQueue),
    Durable(RealtimeSpoolAdmissionQueue),
}

/// 无稳定消息 ID 的 OneBot 通知只作为有界历史回补信号。回调不执行 SQL；队列满时
/// 明确失败并由连接循环创建普通连续性 Gap，禁止静默丢弃该信号。
#[derive(Clone)]
pub(crate) struct NonMessageHistorySignalQueue {
    sender: tokio::sync::mpsc::Sender<ConversationRef>,
    fatal_sender: tokio::sync::watch::Sender<bool>,
}

impl NonMessageHistorySignalQueue {
    pub(crate) fn new(
        sender: tokio::sync::mpsc::Sender<ConversationRef>,
        fatal_sender: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        Self {
            sender,
            fatal_sender,
        }
    }

    fn try_schedule(&self, conversation: ConversationRef) -> Result<(), &'static str> {
        match self.sender.try_send(conversation) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.fatal_sender.send_replace(true);
                Err("non-message history signal queue is full")
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.fatal_sender.send_replace(true);
                Err("non-message history signal queue is closed")
            }
        }
    }
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
    /// 文件上传等没有稳定消息 ID 的通知触发有界历史回补；由连接周期 Worker 持久化。
    pub(crate) non_message_history: NonMessageHistorySignalQueue,
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
            NapCatEvent::GroupUpload(event) => {
                if !should_accept_group_message(event.group_id, &self.group_whitelist) {
                    tracing::debug!("群文件通知不在白名单内，跳过历史回补信号");
                    return Ok(());
                }
                let conversation =
                    ConversationRef::new(ConversationKind::Group, event.group_id.to_string())
                        .map_err(|_| {
                            NapCatError::Protocol("invalid group upload conversation".into())
                        })?;
                self.non_message_history
                    .try_schedule(conversation)
                    .map_err(|error| NapCatError::Handler(error.into()))?;
            }
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

    #[tokio::test]
    async fn non_message_history_signal_is_bounded_and_preserves_group_scope() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let (fatal_sender, fatal_receiver) = tokio::sync::watch::channel(false);
        let queue = NonMessageHistorySignalQueue::new(sender, fatal_sender);
        let conversation = ConversationRef::new(ConversationKind::Group, "671260344").unwrap();
        queue.try_schedule(conversation.clone()).unwrap();
        assert_eq!(receiver.recv().await, Some(conversation));
        assert!(!*fatal_receiver.borrow());
    }

    #[test]
    fn non_message_history_signal_queue_rejects_overflow() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let (fatal_sender, fatal_receiver) = tokio::sync::watch::channel(false);
        let queue = NonMessageHistorySignalQueue::new(sender, fatal_sender);
        queue
            .try_schedule(ConversationRef::new(ConversationKind::Group, "1").unwrap())
            .unwrap();
        assert_eq!(
            queue
                .try_schedule(ConversationRef::new(ConversationKind::Group, "2").unwrap())
                .unwrap_err(),
            "non-message history signal queue is full"
        );
        assert!(*fatal_receiver.borrow());
    }

    #[test]
    fn non_message_history_signal_queue_rejects_closed_worker() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let (fatal_sender, fatal_receiver) = tokio::sync::watch::channel(false);
        let queue = NonMessageHistorySignalQueue::new(sender, fatal_sender);
        assert_eq!(
            queue
                .try_schedule(ConversationRef::new(ConversationKind::Group, "1").unwrap())
                .unwrap_err(),
            "non-message history signal queue is closed"
        );
        assert!(*fatal_receiver.borrow());
    }
}
