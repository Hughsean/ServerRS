use async_trait::async_trait;
use serde_json::Value;

use super::attention::AttentionState;
use super::error::QqBotError;
use super::message::NormalizedMessage;

/// 注意力状态端口。
///
/// 应用层只关心注意力语义；具体存储方式由基础设施层决定。
#[async_trait]
pub trait AttentionStore: Send + Sync {
    async fn try_engage(&self, group_id: i64) -> bool;
    async fn confirm_engagement(&self, group_id: i64);
    async fn start_cooldown(&self);
    async fn get_state(&self) -> AttentionState;
    async fn can_process(&self, group_id: i64) -> bool;
    async fn tick_idle(&self);
}

/// 群消息发送端口。
///
/// OneBot/NapCat 的 HTTP 细节留在基础设施层，应用层只依赖发送能力。
#[async_trait]
pub trait GroupMessageGateway: Send + Sync {
    async fn send_group_msg(
        &self,
        group_id: i64,
        message: &str,
    ) -> Result<Option<String>, QqBotError>;
    async fn group_poke(&self, group_id: i64, user_id: i64) -> Result<(), QqBotError>;
}

/// 标准化群消息处理端口。
///
/// WebSocket listener 属于基础设施层，业务处理器属于应用层；两者通过该端口连接。
#[async_trait]
pub trait GroupMessageHandler: Send + Sync {
    async fn handle_group_message(&self, msg: NormalizedMessage, raw_json: Value);
}
