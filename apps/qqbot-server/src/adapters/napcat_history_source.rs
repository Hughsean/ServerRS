//! NapCat 只读历史来源适配器：实现 [`HistoryBackfillSourceT`]。
//!
//! 职责：
//! - 调用 `NapCatHistoryReadT` 的群/私聊历史只读接口；
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
    BackfillAnchor, BackfillContinuation, BackfillCursor, BackfillHistoryItem, BackfillPage,
    BackfillReadDirection, BackfillScope, BackfillSourceError, ConversationKind,
    HistoryBackfillSourceT, SourceAccountRef,
};
use qqbot::napcat::{
    FriendHistoryQuery, GroupHistoryQuery, HistoryMessage, HistoryReadDirection,
    MessageSegment as NapCatMessageSegment, NapCatError, NapCatHistoryReadT,
};

use crate::inbound::map_core;

/// 账号视角的只读历史来源。所有历史读取都经过最小历史读取端口。
pub(crate) struct NapCatHistorySource {
    client: Arc<dyn NapCatHistoryReadT>,
    account: SourceAccountRef,
    self_qq_id: i64,
}

impl NapCatHistorySource {
    pub(crate) fn new(
        client: Arc<dyn NapCatHistoryReadT>,
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
        direction: BackfillReadDirection,
        page_size: u32,
    ) -> Result<BackfillPage, BackfillSourceError> {
        let BackfillReadDirection::NewestToOldest = direction;
        if scope.account != self.account {
            return Err(BackfillSourceError::Protocol(
                "history scope belongs to a different account subject".into(),
            ));
        }
        if self.account.account_id != self.self_qq_id.to_string() {
            return Err(BackfillSourceError::Protocol(
                "history adapter account identity is inconsistent".into(),
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
        if let Some(cursor) = cursor
            && (cursor.anchor.message_id.trim().is_empty()
                || cursor.anchor.message_seq.trim().is_empty())
        {
            return Err(BackfillSourceError::Protocol(
                "history cursor does not contain a stable anchor".into(),
            ));
        }

        let message_seq = cursor.map(|cursor| cursor.anchor.message_seq.as_str());
        let messages = match scope.conversation.kind {
            ConversationKind::Group => {
                let query = GroupHistoryQuery::new(
                    scope.conversation.id.clone(),
                    message_seq.map(str::to_owned),
                    page_size,
                    HistoryReadDirection::TowardOlder,
                )
                .map_err(map_source_error)?;
                self.client.get_group_msg_history(&query).await
            }
            ConversationKind::Private => {
                let query = FriendHistoryQuery::new(
                    scope.conversation.id.clone(),
                    message_seq.map(str::to_owned),
                    page_size,
                    HistoryReadDirection::TowardOlder,
                )
                .map_err(map_source_error)?;
                self.client.get_friend_msg_history(&query).await
            }
            ConversationKind::OwnerControl => {
                // 历史回补不覆盖官方 Bot 控制会话（其事件由开放平台驱动）。
                return Ok(BackfillPage {
                    items: Vec::new(),
                    continuation: BackfillContinuation::UnprovenStop,
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
            let envelope = map_history_message(&message, scope, self.self_qq_id).map_err(|_| {
                BackfillSourceError::Protocol("history response could not be mapped safely".into())
            })?;
            items.push(BackfillHistoryItem {
                envelope,
                anchor: anchor.clone(),
            });
            last_anchor = Some(anchor);
        }

        // NapCat 无法证明空页是历史起点。非空页（包括短页）始终仅从
        // 最后一条真实返回消息构造 Next，不根据页长推断终止。
        let continuation = match last_anchor {
            Some(anchor) => {
                BackfillContinuation::Next(BackfillCursor::new(self.account.clone(), anchor))
            }
            None => BackfillContinuation::UnprovenStop,
        };

        Ok(BackfillPage {
            items,
            continuation,
        })
    }

    fn history_start_evidence_proven(&self) -> bool {
        false
    }

    fn page_order_evidence_proven(&self) -> bool {
        // ENV-004 完成前，请求参数只表达期望方向，不能证明具体 NapCat/PacketBackend
        // 响应确实按新到旧排列。
        false
    }

    fn account_conversation_set_proven(&self) -> bool {
        // 真实 NapCat 无法枚举账号全部会话，因此永远返回 false。账号级 Gap 必须保持
        // uncertain，只有确定性 Fake 来源可以返回 true 以验证 verified_complete。
        false
    }
}

/// 校验 NapCat 响应仍属于请求的账号主体和群/私聊会话。身份字段缺失或
/// 不一致都 fail closed，禁止把跨账号或跨会话响应写入当前 Scope。
fn validate_history_identity(
    message: &HistoryMessage,
    scope: &BackfillScope,
    account: &SourceAccountRef,
) -> Result<(), BackfillSourceError> {
    if message.self_id.trim().is_empty() || message.self_id != account.account_id {
        return Err(BackfillSourceError::Protocol(
            "history response self_id differs from the requested account subject".into(),
        ));
    }
    match scope.conversation.kind {
        ConversationKind::Group => {
            if message.message_type != "group"
                || message.group_id.as_deref() != Some(scope.conversation.id.as_str())
            {
                return Err(BackfillSourceError::Protocol(
                    "history response does not match the requested group conversation".into(),
                ));
            }
        }
        ConversationKind::Private => {
            let peer_matches =
                message.user_id == account.account_id || message.user_id == scope.conversation.id;
            if message.message_type != "private" || !peer_matches {
                return Err(BackfillSourceError::Protocol(
                    "history response does not match the requested private conversation".into(),
                ));
            }
        }
        ConversationKind::OwnerControl => {
            return Err(BackfillSourceError::Protocol(
                "owner-control history is not supported".into(),
            ));
        }
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
    let conversation_id: i64 =
        scope.conversation.id.parse().map_err(|_| {
            NapCatError::Protocol("history conversation id is not an integer".into())
        })?;

    let is_self = message.user_id == self_qq_id.to_string();
    let protocol_actor_id: i64 = message
        .user_id
        .parse()
        .map_err(|_| NapCatError::Protocol("history user_id is not an integer".into()))?;
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
        NapCatError::Connection(_) => {
            BackfillSourceError::Unavailable("NapCat history transport unavailable".into())
        }
        NapCatError::Protocol(_) => {
            BackfillSourceError::Protocol("NapCat history protocol error".into())
        }
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
                let _ = action;
                BackfillSourceError::Protocol(format!("NapCat history API failed: code={code}"))
            }
        }
        NapCatError::Handler(_) => {
            BackfillSourceError::Unavailable("NapCat history handler unavailable".into())
        }
        // Heartbeat 超时只影响实时监听连接，不发生在历史回补的只读 API 调用路径；
        // 若误入此处，视为暂时性不可用以便重试。
        NapCatError::HeartbeatTimeout(_) => {
            BackfillSourceError::Unavailable("NapCat history transport unavailable".into())
        }
    }
}

/// 把历史消息的结构化 `message` 数组解析为协议无关消息段，与实时 CQ 解析保持一致。
pub(crate) fn parse_history_segments(
    message: &serde_json::Value,
    _self_qq_id: i64,
) -> Vec<NapCatMessageSegment> {
    use qqbot::napcat::MessageSegment;
    if let Some(array) = message.as_array() {
        // 与实时 WebSocket 共用同一结构化段解析器，避免历史 Backfill 将 json/xml/card/
        // forward 等已知段降级为 Unknown，并统一段数、单段和总字节预算。
        return qqbot::napcat::segments::parse_structured_segments(array).0;
    }

    // 非数组（如纯字符串 raw_message）回退为单条文本段。
    message
        .as_str()
        .map(|text| {
            vec![MessageSegment::Text {
                content: text.to_string(),
            }]
        })
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

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
            user_id: "30001".into(),
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

    #[derive(Default)]
    struct FakeHistoryClient {
        group_pages: Mutex<VecDeque<Result<Vec<HistoryMessage>, NapCatError>>>,
        private_pages: Mutex<VecDeque<Result<Vec<HistoryMessage>, NapCatError>>>,
        group_queries: Mutex<Vec<(Option<String>, HistoryReadDirection)>>,
        private_queries: Mutex<Vec<(Option<String>, HistoryReadDirection)>>,
    }

    #[async_trait]
    impl NapCatHistoryReadT for FakeHistoryClient {
        async fn get_group_msg_history(
            &self,
            query: &GroupHistoryQuery,
        ) -> Result<Vec<HistoryMessage>, NapCatError> {
            self.group_queries
                .lock()
                .unwrap()
                .push((query.message_seq().map(str::to_owned), query.direction()));
            self.group_pages
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted group page")
        }

        async fn get_friend_msg_history(
            &self,
            query: &FriendHistoryQuery,
        ) -> Result<Vec<HistoryMessage>, NapCatError> {
            self.private_queries
                .lock()
                .unwrap()
                .push((query.message_seq().map(str::to_owned), query.direction()));
            self.private_pages
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted private page")
        }
    }

    fn source(client: Arc<FakeHistoryClient>) -> NapCatHistorySource {
        let port: Arc<dyn NapCatHistoryReadT> = client;
        NapCatHistorySource::new(port, account("10001"), 10001)
    }

    fn private_scope() -> BackfillScope {
        BackfillScope {
            account: account("10001"),
            conversation: ConversationRef::new(ConversationKind::Private, "30001").unwrap(),
            boundary_cursor: None,
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

    #[tokio::test]
    async fn nonempty_short_group_page_returns_real_next_and_empty_page_is_unproven() {
        let client = Arc::new(FakeHistoryClient::default());
        let mut newest = message("10001", "20001");
        newest.message_id = "newer-message".into();
        newest.message_seq = "11".into();
        let mut oldest = message("10001", "20001");
        oldest.message_id = "older-message".into();
        oldest.message_seq = "10".into();
        client
            .group_pages
            .lock()
            .unwrap()
            .push_back(Ok(vec![newest, oldest]));
        client.group_pages.lock().unwrap().push_back(Ok(Vec::new()));
        let source = source(client.clone());
        let scope = scope(account("10001"), "20001");

        let first = source
            .fetch_page(&scope, None, BackfillReadDirection::NewestToOldest, 100)
            .await
            .unwrap();
        let next = match first.continuation {
            BackfillContinuation::Next(cursor) => cursor,
            other => panic!("short nonempty page must continue, got {other:?}"),
        };
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.anchor.message_id.as_str())
                .collect::<Vec<_>>(),
            ["newer-message", "older-message"]
        );
        assert_eq!(next.anchor.message_id, "older-message");
        assert_eq!(next.anchor.message_seq, "10");

        let second = source
            .fetch_page(
                &scope,
                Some(&next),
                BackfillReadDirection::NewestToOldest,
                100,
            )
            .await
            .unwrap();
        assert!(second.items.is_empty());
        assert_eq!(second.continuation, BackfillContinuation::UnprovenStop);
        assert_eq!(
            client.group_queries.lock().unwrap().as_slice(),
            [
                (None, HistoryReadDirection::TowardOlder),
                (Some("10".to_owned()), HistoryReadDirection::TowardOlder),
            ]
        );
        assert!(!source.history_start_evidence_proven());
        assert!(!source.page_order_evidence_proven());
        assert!(!source.account_conversation_set_proven());
    }

    #[tokio::test]
    async fn private_history_preserves_opaque_cursor_and_uses_toward_older_direction() {
        let client = Arc::new(FakeHistoryClient::default());
        let mut private_message = message("10001", "");
        private_message.message_type = "private".into();
        private_message.group_id = None;
        private_message.message_seq = "opaque/private+seq".into();
        client
            .private_pages
            .lock()
            .unwrap()
            .push_back(Ok(vec![private_message]));
        client
            .private_pages
            .lock()
            .unwrap()
            .push_back(Ok(Vec::new()));
        let source = source(client.clone());
        let scope = private_scope();

        let first = source
            .fetch_page(&scope, None, BackfillReadDirection::NewestToOldest, 10)
            .await
            .unwrap();
        let next = match first.continuation {
            BackfillContinuation::Next(cursor) => cursor,
            other => panic!("private nonempty page must continue, got {other:?}"),
        };
        source
            .fetch_page(
                &scope,
                Some(&next),
                BackfillReadDirection::NewestToOldest,
                10,
            )
            .await
            .unwrap();

        assert_eq!(
            client.private_queries.lock().unwrap().as_slice(),
            [
                (None, HistoryReadDirection::TowardOlder),
                (
                    Some("opaque/private+seq".to_owned()),
                    HistoryReadDirection::TowardOlder
                ),
            ]
        );
    }

    #[tokio::test]
    async fn owner_control_never_fabricates_history_start_evidence() {
        let client = Arc::new(FakeHistoryClient::default());
        let source = source(client);
        let owner_scope = BackfillScope {
            account: account("10001"),
            conversation: ConversationRef::new(ConversationKind::OwnerControl, "owner").unwrap(),
            boundary_cursor: None,
        };
        let page = source
            .fetch_page(
                &owner_scope,
                None,
                BackfillReadDirection::NewestToOldest,
                10,
            )
            .await
            .unwrap();
        assert_eq!(page.continuation, BackfillContinuation::UnprovenStop);
    }

    #[test]
    fn identity_and_protocol_errors_are_redacted() {
        let requested_account = account("10001");
        let requested_scope = scope(requested_account.clone(), "20001");
        let identity_error = validate_history_identity(
            &message("sensitive-account-9988", "sensitive-group-7766"),
            &requested_scope,
            &requested_account,
        )
        .unwrap_err()
        .to_string();

        let protocol_error = map_source_error(NapCatError::Api {
            action: "get_group_msg_history".into(),
            code: 500,
            message: "account=9988 qq=8877 group=7766 message_id=mid-secret \
                      message_seq=seq-secret body-secret token-secret \
                      https://secret.invalid response-data-secret"
                .into(),
        })
        .to_string();

        for error in [identity_error, protocol_error] {
            for secret in [
                "9988",
                "8877",
                "7766",
                "mid-secret",
                "seq-secret",
                "body-secret",
                "token-secret",
                "secret.invalid",
                "response-data-secret",
            ] {
                assert!(!error.contains(secret), "error leaked {secret}: {error}");
            }
        }
    }

    // 历史与实时共用解析器；未知段按字符边界有界，不能因多字节内容 panic。
    #[test]
    fn parse_history_segments_unknown_segment_truncates_multibyte_safely() {
        let big_content = "你好".repeat(2000);
        let unknown_seg = serde_json::json!([{"type":"poke","data":{"name": big_content}}]);
        let segments = parse_history_segments(&unknown_seg, 10001);
        assert_eq!(segments.len(), 1);
        if let qqbot::napcat::MessageSegment::Unknown { raw, .. } = &segments[0] {
            let raw = raw.as_ref().expect("raw must be present");
            assert!(raw.chars().count() <= qqbot::napcat::segments::MAX_META_CHARS);
            assert!(std::str::from_utf8(raw.as_bytes()).is_ok());
        } else {
            panic!("expected Unknown segment");
        }
    }

    #[test]
    fn parse_history_segments_preserves_json_card_as_rich_content() {
        let message = serde_json::json!([{
            "type": "json",
            "data": {"data": {"app": "com.tencent.structmsg", "desc": "群分享"}}
        }]);
        let segments = parse_history_segments(&message, 10001);
        assert_eq!(segments.len(), 1);
        let qqbot::napcat::MessageSegment::Rich {
            kind,
            content_sha256,
            ..
        } = &segments[0]
        else {
            panic!("expected rich JSON segment");
        };
        assert_eq!(*kind, qqbot::napcat::RichKind::Json);
        let digest = content_sha256.as_deref().expect("JSON digest must exist");
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
