use personal_secretary::{
    ContentSegment, ConversationKind, ConversationRef, InboundMessageEnvelope, MediaKind,
    MessageSource, SourceMessageRef, VerifiedActor, VerifiedActorKind,
};
use qqbot::napcat::{
    GroupMessageEvent, MessageSegment as NapCatMessageSegment, NapCatError, PrivateMessageEvent,
};

/// 把 NapCat 协议身份映射到个人秘书的统一消息边界。
pub(crate) struct NapCatInboundMapper {
    self_qq_id: i64,
}

impl NapCatInboundMapper {
    pub(crate) fn new(self_qq_id: i64) -> Self {
        Self { self_qq_id }
    }

    pub(crate) fn map_group(
        &self,
        event: GroupMessageEvent,
    ) -> Result<InboundMessageEnvelope, NapCatError> {
        self.map(
            event.message_id,
            ConversationKind::Group,
            event.group_id,
            event.user_id,
            event.is_self,
            event.time,
            event.normalized_text,
            event.segments,
        )
    }

    pub(crate) fn map_private(
        &self,
        event: PrivateMessageEvent,
    ) -> Result<InboundMessageEnvelope, NapCatError> {
        self.map(
            event.message_id,
            ConversationKind::Private,
            event.peer_id,
            event.user_id,
            event.is_self,
            event.time,
            event.normalized_text,
            event.segments,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn map(
        &self,
        message_id: String,
        conversation_kind: ConversationKind,
        conversation_id: i64,
        protocol_actor_id: i64,
        is_self: bool,
        occurred_at_unix_secs: i64,
        normalized_text: String,
        segments: Vec<NapCatMessageSegment>,
    ) -> Result<InboundMessageEnvelope, NapCatError> {
        let actor_id = if is_self {
            self.self_qq_id
        } else {
            protocol_actor_id
        };
        let actor_kind = if is_self {
            VerifiedActorKind::Owner
        } else {
            VerifiedActorKind::External
        };

        map_core(
            self.self_qq_id,
            message_id,
            conversation_kind,
            conversation_id,
            actor_kind,
            actor_id,
            occurred_at_unix_secs,
            normalized_text,
            segments,
        )
    }
}

/// 实时消息和历史消息共用的身份与消息段映射核心。
///
/// 历史回补必须与实时入库使用同一套身份、角色和消息段规则，避免历史事件与实时事件
/// 产生不一致的 SourceEvent。历史消息的 `connection_epoch_id` 为空（非实时观测）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn map_core(
    self_qq_id: i64,
    message_id: String,
    conversation_kind: ConversationKind,
    conversation_id: i64,
    actor_kind: VerifiedActorKind,
    actor_id: i64,
    occurred_at_unix_secs: i64,
    normalized_text: String,
    segments: Vec<NapCatMessageSegment>,
) -> Result<InboundMessageEnvelope, NapCatError> {
    InboundMessageEnvelope::new(
        SourceMessageRef::new(MessageSource::NapCat, self_qq_id.to_string(), message_id)
            .map_err(map_identity_error)?,
        ConversationRef::new(conversation_kind, conversation_id.to_string())
            .map_err(map_identity_error)?,
        VerifiedActor::new(actor_kind, actor_id.to_string()).map_err(map_identity_error)?,
        occurred_at_unix_secs,
        normalized_text,
        map_segments(segments),
    )
    .map_err(map_identity_error)
}

fn map_segments(segments: Vec<NapCatMessageSegment>) -> Vec<ContentSegment> {
    segments
        .into_iter()
        .map(|segment| match segment {
            NapCatMessageSegment::Text { content } => ContentSegment::Text { content },
            NapCatMessageSegment::Face { id, text } => ContentSegment::Face {
                source_id: id.to_string(),
                display_text: text,
            },
            NapCatMessageSegment::Image { file, url } => ContentSegment::Media {
                kind: MediaKind::Image,
                source_key: file,
                source_url: url,
                display_name: None,
            },
            NapCatMessageSegment::At { qq } if qq.eq_ignore_ascii_case("all") => {
                ContentSegment::MentionAll
            }
            NapCatMessageSegment::At { qq } => ContentSegment::Mention { actor_id: qq },
            NapCatMessageSegment::Reply { id } => ContentSegment::Reply {
                platform_message_id: id,
            },
            NapCatMessageSegment::Record { file } => ContentSegment::Media {
                kind: MediaKind::Audio,
                source_key: file,
                source_url: None,
                display_name: None,
            },
            NapCatMessageSegment::Video { file, url } => ContentSegment::Media {
                kind: MediaKind::Video,
                source_key: file,
                source_url: url,
                display_name: None,
            },
            NapCatMessageSegment::File { file, name, .. } => ContentSegment::Media {
                kind: MediaKind::File,
                source_key: file,
                source_url: None,
                display_name: name,
            },
            // 合并转发引用：只保留协议层 ID，不下载全部内容（B2 约束）。
            NapCatMessageSegment::Forward { id } => ContentSegment::Unknown {
                protocol_value: format!("forward:{id}"),
            },
            // 富消息 envelope：只保存有限描述，不保存完整载荷（B2/B6 约束）。
            NapCatMessageSegment::Rich {
                kind,
                data,
                summary,
                ..
            } => {
                let value = match (data, summary) {
                    (Some(d), Some(s)) => format!("{kind:?}:{d}|{s}"),
                    (Some(d), None) => format!("{kind:?}:{d}"),
                    (None, Some(s)) => format!("{kind:?}:|{s}"),
                    (None, None) => format!("{kind:?}"),
                };
                ContentSegment::Unknown {
                    protocol_value: value,
                }
            }
            NapCatMessageSegment::Unknown { seg_type, raw } => ContentSegment::Unknown {
                protocol_value: raw.map(|r| format!("{seg_type}:{r}")).unwrap_or(seg_type),
            },
        })
        .collect()
}

fn map_identity_error(error: personal_secretary::InboundIdentityError) -> NapCatError {
    NapCatError::Protocol(format!(
        "cannot map NapCat message into personal secretary identity boundary: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use personal_secretary::{MessageRole, MessageSource};
    use qqbot::napcat::{GroupMessageEvent, PrivateMessageEvent};
    use serde_json::Value;

    use super::*;

    fn private_message(is_self: bool) -> PrivateMessageEvent {
        PrivateMessageEvent {
            message_id: "private-1".into(),
            user_id: if is_self { 10001 } else { 20002 },
            peer_id: 20002,
            raw_message: "明天下午开会".into(),
            normalized_text: "明天下午开会".into(),
            segments: Vec::new(),
            time: 100,
            sender: None,
            is_self,
            raw_event: Value::Null,
        }
    }

    #[test]
    fn napcat_owner_message_is_observation_not_instruction() {
        let message = NapCatInboundMapper::new(10001)
            .map_private(private_message(true))
            .unwrap();

        assert_eq!(message.source.channel, MessageSource::NapCat);
        assert_eq!(message.source.account_id, "10001");
        assert_eq!(message.actor.id, "10001");
        assert_eq!(message.role(), MessageRole::OwnerObservation);
        assert!(!message.accepts_instructions());
    }

    #[test]
    fn private_peer_and_external_actor_are_kept_separate() {
        let message = NapCatInboundMapper::new(10001)
            .map_private(private_message(false))
            .unwrap();

        assert_eq!(message.conversation.id, "20002");
        assert_eq!(message.actor.id, "20002");
        assert_eq!(message.role(), MessageRole::ExternalObservation);
    }

    #[test]
    fn group_message_uses_group_as_conversation_identity() {
        let event = GroupMessageEvent {
            message_id: "group-1".into(),
            group_id: 30003,
            user_id: 20002,
            raw_message: "收到".into(),
            normalized_text: "收到".into(),
            segments: vec![
                NapCatMessageSegment::At { qq: "20003".into() },
                NapCatMessageSegment::At { qq: "all".into() },
                NapCatMessageSegment::Reply {
                    id: "group-0".into(),
                },
            ],
            at_bot: false,
            time: 101,
            sender: None,
            is_self: false,
            raw_event: Value::Null,
        };

        let message = NapCatInboundMapper::new(10001).map_group(event).unwrap();
        assert_eq!(message.conversation.id, "30003");
        assert_eq!(message.actor.id, "20002");
        assert_eq!(message.mentioned_actor_ids().collect::<Vec<_>>(), ["20003"]);
        assert!(message.mentions_all());
        assert_eq!(message.reply_to_platform_message_id(), Some("group-0"));
    }
}
