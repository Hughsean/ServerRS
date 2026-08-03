//! NapCat 只读历史来源适配器：实现 [`HistoryBackfillSourceT`]。
//!
//! 职责：
//! - 调用 `NapCatApiClient` 的群/私聊历史只读接口；
//! - 把 `HistoryMessage` 转换成协议无关回补 DTO（[`BackfillHistoryItem`]）；
//! - 复用 [`crate::inbound::map_core`]，使历史消息与实时消息使用同一套身份、角色和
//!   消息段规则；
//! - 所有 Cursor 和锚点绑定当前账号视角，不跨账号解释平台消息 ID；
//! - 分页推进只基于接口实际返回的真实锚点，禁止数值加减。
//!
//! 不实现回补用例本身、不写 SQL、不发送消息。

use std::sync::Arc;

use async_trait::async_trait;
use personal_secretary::{
    BackfillAnchor, BackfillCursor, BackfillHistoryItem, BackfillPage, BackfillScope,
    BackfillSourceError, ConversationKind, HistoryBackfillSourceT, SourceAccountRef,
};
use qqbot::napcat::{
    HistoryMessage, MessageSegment as NapCatMessageSegment, NapCatApiClient, NapCatError,
};

use crate::inbound::map_core;

/// 账号视角的只读历史来源。所有历史读取都经过 `NapCatApiClient` 的只读方法。
pub(crate) struct NapCatHistorySource {
    client: Arc<NapCatApiClient>,
    account: SourceAccountRef,
    self_qq_id: i64,
}

impl NapCatHistorySource {
    pub(crate) fn new(
        client: Arc<NapCatApiClient>,
        account: SourceAccountRef,
        self_qq_id: i64,
    ) -> Self {
        Self {
            client,
            account,
            self_qq_id,
        }
    }
}

#[async_trait]
impl HistoryBackfillSourceT for NapCatHistorySource {
    async fn fetch_page(
        &self,
        scope: &BackfillScope,
        cursor: Option<&BackfillCursor>,
        page_size: u32,
    ) -> Result<BackfillPage, BackfillSourceError> {
        if scope.account != self.account {
            return Err(BackfillSourceError::Protocol(
                "history scope belongs to a different account subject".into(),
            ));
        }
        // 锚点必须绑定当前账号视角；外部传入的游标若属于其它账号则视为协议错误。
        if let Some(cursor) = cursor
            && cursor.account != self.account
        {
            return Err(BackfillSourceError::Protocol(
                "history cursor belongs to a different account subject".into(),
            ));
        }

        let message_seq = cursor.map(|cursor| cursor.anchor.message_seq.as_str());
        let reverse_order = false;
        let messages = match scope.conversation.kind {
            ConversationKind::Group => {
                self.client
                    .get_group_msg_history(
                        &scope.conversation.id,
                        message_seq,
                        page_size,
                        reverse_order,
                    )
                    .await
            }
            ConversationKind::Private => {
                self.client
                    .get_friend_msg_history(
                        &scope.conversation.id,
                        message_seq,
                        page_size,
                        reverse_order,
                    )
                    .await
            }
            ConversationKind::OwnerControl => {
                // 历史回补不覆盖官方 Bot 控制会话（其事件由开放平台驱动）。
                return Ok(BackfillPage {
                    items: Vec::new(),
                    next_cursor: None,
                });
            }
        }
        .map_err(map_source_error)?;

        let mut items = Vec::with_capacity(messages.len());
        let mut last_anchor: Option<BackfillAnchor> = None;
        for message in messages {
            validate_history_identity(&message, scope, &self.account)?;
            // 缺少任一真实锚点字段时不能安全分页。静默跳过会在其它消息命中边界时错误宣称
            // Scope 完整，因此必须把整页降级为协议异常并保持 Gap uncertain。
            if message.message_id.trim().is_empty() || message.message_seq.trim().is_empty() {
                return Err(BackfillSourceError::Protocol(
                    "history response contains a message without a stable id/sequence anchor"
                        .into(),
                ));
            }
            let anchor =
                BackfillAnchor::new(message.message_id.clone(), message.message_seq.clone());
            let envelope = map_history_message(&message, scope, self.self_qq_id)
                .map_err(|error| BackfillSourceError::Protocol(error.to_string()))?;
            items.push(BackfillHistoryItem {
                envelope,
                anchor: anchor.clone(),
            });
            last_anchor = Some(anchor);
        }

        // 下一页游标：基于本页最后一条消息的真实锚点。若本页为空或无锚点，
        // 则无下一页（由用例判定为空页歧义或到达起点）。
        let next_cursor =
            last_anchor.map(|anchor| BackfillCursor::new(self.account.clone(), anchor));

        Ok(BackfillPage { items, next_cursor })
    }

    fn account_conversation_set_proven(&self) -> bool {
        // 真实 NapCat 无法枚举账号全部会话，因此永远返回 false。账号级 Gap 必须保持
        // uncertain，只有确定性 Fake 来源可以返回 true 以验证 verified_complete。
        false
    }
}

/// 校验 NapCat 响应仍属于请求的账号主体和会话。空 `self_id`/`group_id` 兼容部分旧响应，
/// 但一旦接口提供了身份字段就必须与请求一致，禁止把跨账号或跨群响应写入当前 Scope。
fn validate_history_identity(
    message: &HistoryMessage,
    scope: &BackfillScope,
    account: &SourceAccountRef,
) -> Result<(), BackfillSourceError> {
    if !message.self_id.trim().is_empty() && message.self_id != account.account_id {
        return Err(BackfillSourceError::Protocol(
            "history response self_id differs from the requested account subject".into(),
        ));
    }
    if scope.conversation.kind == ConversationKind::Group
        && let Some(group_id) = message.group_id.as_deref()
        && !group_id.trim().is_empty()
        && group_id != scope.conversation.id
    {
        return Err(BackfillSourceError::Protocol(
            "history response group_id differs from the requested conversation".into(),
        ));
    }
    Ok(())
}

/// 把单条 NapCat 历史消息映射为协议无关信封，复用与实时消息相同的映射核心。
fn map_history_message(
    message: &HistoryMessage,
    scope: &BackfillScope,
    self_qq_id: i64,
) -> Result<personal_secretary::InboundMessageEnvelope, NapCatError> {
    let conversation_kind = scope.conversation.kind;
    let conversation_id: i64 = scope.conversation.id.parse().map_err(|_| {
        NapCatError::Protocol(format!(
            "history conversation id is not an integer: {}",
            scope.conversation.id
        ))
    })?;

    let is_self = message.user_id == self_qq_id.to_string();
    let protocol_actor_id: i64 = message.user_id.parse().map_err(|_| {
        NapCatError::Protocol(format!(
            "history user_id is not an integer: {}",
            message.user_id
        ))
    })?;
    let actor_id = if is_self {
        self_qq_id
    } else {
        protocol_actor_id
    };
    let actor_kind = if is_self {
        personal_secretary::VerifiedActorKind::Owner
    } else {
        personal_secretary::VerifiedActorKind::External
    };

    let segments = parse_history_segments(&message.message, self_qq_id);
    let normalized_text = normalize_history_text(&message.message, self_qq_id);

    map_core(
        self_qq_id,
        message.message_id.clone(),
        conversation_kind,
        conversation_id,
        actor_kind,
        actor_id,
        message.time,
        normalized_text,
        segments,
        message
            .sender
            .as_ref()
            .map(|sender| personal_secretary::ObservedSenderProfile {
                nickname: sender.nickname.chars().take(200).collect(),
                group_card: sender
                    .card
                    .as_ref()
                    .map(|card| card.chars().take(200).collect()),
                group_role: sender
                    .role
                    .as_ref()
                    .map(|role| role.chars().take(16).collect()),
            }),
    )
}

fn map_source_error(error: NapCatError) -> BackfillSourceError {
    match error {
        NapCatError::Connection(detail) => BackfillSourceError::Unavailable(detail),
        NapCatError::Protocol(detail) => BackfillSourceError::Protocol(detail),
        NapCatError::Api {
            action,
            code,
            message,
        } => {
            // NapCat/OneBot 的 retcode=200 既可能是权限不足，也可能是“无效相邻 message_seq
            // 锚点”（见 napcat-history-contract.md）。无法可靠区分时一律视为暂时性协议错误
            //（Gap 回到 uncertain 可重试），避免把可恢复的锚点错误误判为永久 PermissionDenied
            // 而标记 unrecoverable。PermissionDenied 仅在消息明确包含权限语义时才采用。
            if message.contains("权限") || message.to_ascii_lowercase().contains("permission") {
                BackfillSourceError::PermissionDenied
            } else {
                BackfillSourceError::Protocol(format!(
                    "NapCat API {action} failed: code={code}, {message}"
                ))
            }
        }
        NapCatError::Handler(detail) => BackfillSourceError::Unavailable(detail),
        // Heartbeat 超时只影响实时监听连接，不发生在历史回补的只读 API 调用路径；
        // 若误入此处，视为暂时性不可用以便重试。
        NapCatError::HeartbeatTimeout(detail) => BackfillSourceError::Unavailable(detail),
    }
}

/// 把历史消息的结构化 `message` 数组解析为协议无关消息段，与实时 CQ 解析保持一致。
pub(crate) fn parse_history_segments(
    message: &serde_json::Value,
    self_qq_id: i64,
) -> Vec<NapCatMessageSegment> {
    use qqbot::napcat::MessageSegment;
    let Some(array) = message.as_array() else {
        // 非数组（如纯字符串 raw_message）回退为单条文本段。
        if let Some(text) = message.as_str() {
            return vec![MessageSegment::Text {
                content: text.to_string(),
            }];
        }
        return Vec::new();
    };

    let mut segments = Vec::new();
    for item in array {
        let ty = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let data = item.get("data").cloned().unwrap_or(serde_json::Value::Null);
        match ty {
            "text" => {
                let content = data
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                segments.push(MessageSegment::Text { content });
            }
            "face" => {
                let id = data
                    .get("id")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0) as i32;
                let text = data
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                segments.push(MessageSegment::Face { id, text });
            }
            "image" => {
                let file = data
                    .get("file")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let url = data
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                segments.push(MessageSegment::Image { file, url });
            }
            "at" => {
                let qq = data
                    .get("qq")
                    .map(|value| match value {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                segments.push(MessageSegment::At { qq });
            }
            "reply" => {
                let id = data
                    .get("id")
                    .map(|value| match value {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                segments.push(MessageSegment::Reply { id });
            }
            "record" => {
                let file = data
                    .get("file")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                segments.push(MessageSegment::Record { file });
            }
            "video" => {
                let file = data
                    .get("file")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let url = data
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                segments.push(MessageSegment::Video { file, url });
            }
            "file" => {
                let file = data
                    .get("file")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = data
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                let size = data.get("size").and_then(serde_json::Value::as_u64);
                segments.push(MessageSegment::File { file, name, size });
            }
            _ => {
                // 评审 P1-2：历史 Unknown 段也必须有界，与实时解析语义一致。
                // 必须按 UTF-8 字符边界截断，否则中文字符/emoji 切在中间会 panic。
                let raw = item.to_string();
                let raw = truncate_utf8_safe(&raw, 2000);
                segments.push(MessageSegment::Unknown {
                    seg_type: item
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                        .to_string(),
                    raw: Some(raw),
                });
            }
        }
    }
    // self_qq_id 用于与实时解析保持一致（at_bot 语义），此处仅保留参数以便未来扩展。
    let _ = self_qq_id;
    segments
}

/// 把历史消息结构化数组归一化为纯文本，与实时 `normalize_text` 行为一致。
fn normalize_history_text(message: &serde_json::Value, self_qq_id: i64) -> String {
    let segments = parse_history_segments(message, self_qq_id);
    let mut parts = Vec::new();
    for segment in &segments {
        if let qqbot::napcat::MessageSegment::Text { content } = segment {
            parts.push(content.clone());
        }
    }
    parts.join(" ").trim().to_string()
}

/// 按字节上限截断字符串，保证 UTF-8 字符边界安全（评审 P1-2）。
/// `str::is_char_boundary` 回退到最近的合法字符边界，不会切在多字节字符中间。
fn truncate_utf8_safe(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_secretary::{ConversationRef, MessageSource};

    fn account(id: &str) -> SourceAccountRef {
        SourceAccountRef::new(MessageSource::NapCat, id).unwrap()
    }

    fn scope(account: SourceAccountRef, group_id: &str) -> BackfillScope {
        BackfillScope {
            account,
            conversation: ConversationRef::new(ConversationKind::Group, group_id).unwrap(),
            boundary_cursor: None,
        }
    }

    fn message(self_id: &str, group_id: &str) -> HistoryMessage {
        HistoryMessage {
            self_id: self_id.into(),
            user_id: "sender".into(),
            time: 0,
            message_id: "message".into(),
            message_seq: "sequence".into(),
            message_type: "group".into(),
            group_id: Some(group_id.into()),
            raw_message: String::new(),
            message: serde_json::json!([]),
            sender: None,
        }
    }

    #[test]
    fn history_response_identity_must_match_account_and_group_scope() {
        let expected_account = account("10001");
        let expected_scope = scope(expected_account.clone(), "20001");
        assert!(
            validate_history_identity(
                &message("10001", "20001"),
                &expected_scope,
                &expected_account,
            )
            .is_ok()
        );
        assert!(
            validate_history_identity(
                &message("other-account", "20001"),
                &expected_scope,
                &expected_account,
            )
            .is_err()
        );
        assert!(
            validate_history_identity(
                &message("10001", "other-group"),
                &expected_scope,
                &expected_account,
            )
            .is_err()
        );
    }

    // 评审 P1-2：历史 Unknown 段截断必须按 UTF-8 字符边界，不能 panic。
    // 多字节中文/emoji 切在中间会导致 `byte index is not a char boundary` panic。
    #[test]
    fn truncate_utf8_safe_handles_multibyte_characters_without_panic() {
        // 中文每个字符 3 字节 UTF-8。截断到 4 字节应回退到 3 字节边界（1 个中文字符）。
        let s = "你好世界测试";
        let truncated = truncate_utf8_safe(s, 4);
        assert!(truncated.len() <= 4);
        assert!(s.starts_with(&truncated));
        // 验证不 panic 且结果合法。
        assert_eq!(truncated, "你");

        // emoji 4 字节。截断到 5 字节应回退到 4 字节边界。
        let emoji = "😀😀😀";
        let truncated = truncate_utf8_safe(emoji, 5);
        assert!(truncated.len() <= 5);
        assert_eq!(truncated, "😀");

        // 短字符串不截断。
        assert_eq!(truncate_utf8_safe("短", 100), "短");
    }

    // 评审 P1-2：parse_history_segments 对未知段应用 UTF-8 安全截断。
    #[test]
    fn parse_history_segments_unknown_segment_truncates_multibyte_safely() {
        // 构造一个会超过 2000 字节上限的未知段（中文重复）。
        let big_content = "你好".repeat(2000); // 2000 * 3 = 6000 字节
        let unknown_seg = serde_json::json!([{"type":"poke","data":{"name": big_content}}]);
        let segments = parse_history_segments(&unknown_seg, 10001);
        assert_eq!(segments.len(), 1);
        if let qqbot::napcat::MessageSegment::Unknown { raw, .. } = &segments[0] {
            let raw = raw.as_ref().expect("raw must be present");
            assert!(raw.len() <= 2000);
            // 验证是合法 UTF-8（to_string 成功即合法）。
            assert!(std::str::from_utf8(raw.as_bytes()).is_ok());
        } else {
            panic!("expected Unknown segment");
        }
    }
}
