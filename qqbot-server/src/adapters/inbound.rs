use personal_secretary::{
    ContentSegment, ConversationKind, ConversationRef, InboundMessageEnvelope, MediaKind,
    MessageSource, ObservedSenderProfile, RichContentKind, SourceMessageRef, VerifiedActor,
    VerifiedActorKind,
};
use qqbot::napcat::{
    GroupMessageEvent, MessageSegment as NapCatMessageSegment, NapCatError, PrivateMessageEvent,
    SenderInfo,
};

/// 把 NapCat 协议身份映射到个人秘书的统一消息边界。
pub struct NapCatInboundMapper {
    self_qq_id: i64,
}

impl NapCatInboundMapper {
    pub fn new(self_qq_id: i64) -> Self {
        Self { self_qq_id }
    }

    pub fn map_group(
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
            event.sender,
        )
    }

    pub fn map_private(
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
            event.sender,
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
        sender: Option<SenderInfo>,
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
            sender.map(sender_profile_from_sender_info),
        )
    }
}

/// 协议发送者资料 → 观察档案。昵称缺失时保留空串（仅显示，不授权），
/// 群角色保留协议原值，由领域层 `GroupRole::parse_protocol` 解析。
fn sender_profile_from_sender_info(sender: SenderInfo) -> ObservedSenderProfile {
    ObservedSenderProfile {
        nickname: sender.nickname.chars().take(200).collect(),
        group_card: sender.card.map(|card| card.chars().take(200).collect()),
        group_role: sender.role.map(|role| role.chars().take(16).collect()),
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
    sender_profile: Option<ObservedSenderProfile>,
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
    .map_err(map_identity_error)?
    .with_sender_profile(sender_profile)
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
            NapCatMessageSegment::Forward { id } => ContentSegment::Forward { source_key: id },
            NapCatMessageSegment::Rich {
                kind,
                data: _,
                content_sha256,
                summary,
            } => match kind {
                qqbot::napcat::RichKind::Json => {
                    map_rich_segment(RichContentKind::Json, "rich_json", content_sha256, summary)
                }
                qqbot::napcat::RichKind::Xml => {
                    map_rich_segment(RichContentKind::Xml, "rich_xml", content_sha256, summary)
                }
                qqbot::napcat::RichKind::Card => {
                    map_rich_segment(RichContentKind::Card, "rich_card", content_sha256, summary)
                }
                qqbot::napcat::RichKind::Other => ContentSegment::Unknown {
                    protocol_value: "rich_other".into(),
                },
            },
            NapCatMessageSegment::Unknown { seg_type, raw: _ } => ContentSegment::Unknown {
                protocol_value: format!("unknown:{seg_type}"),
            },
        })
        .collect()
}

fn map_rich_segment(
    kind: RichContentKind,
    fallback_source_key: &str,
    content_sha256: Option<String>,
    summary: Option<String>,
) -> ContentSegment {
    match content_sha256 {
        Some(digest)
            if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            ContentSegment::Rich {
                kind,
                source_key: format!("sha256:{digest}"),
                summary,
            }
        }
        _ => ContentSegment::Rich {
            kind,
            source_key: fallback_source_key.into(),
            summary,
        },
    }
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
    fn unknown_segment_discards_raw_payload() {
        let raw = serde_json::json!({
            "url": "https://example.invalid/file?token=secret-token",
            "token": "secret-token",
            "body": {"private": "message text"}
        });
        let mapped = map_segments(vec![NapCatMessageSegment::Unknown {
            seg_type: "future_card".into(),
            raw: Some(raw.to_string()),
        }]);
        let serialized = serde_json::to_string(&mapped).expect("serialize mapped segments");

        assert_eq!(
            mapped,
            vec![ContentSegment::Unknown {
                protocol_value: "unknown:future_card".into()
            }]
        );
        assert!(!serialized.contains("example.invalid"));
        assert!(!serialized.contains("secret-token"));
        assert!(!serialized.contains("message text"));
    }

    #[test]
    fn rich_segment_uses_complete_payload_digest_as_structured_reference() {
        let digest = "a".repeat(64);
        let mapped = map_segments(vec![NapCatMessageSegment::Rich {
            kind: qqbot::napcat::RichKind::Json,
            data: Some("bounded preview".into()),
            content_sha256: Some(digest.clone()),
            summary: Some("摘要".into()),
        }]);

        assert_eq!(
            mapped,
            vec![ContentSegment::Rich {
                kind: RichContentKind::Json,
                source_key: format!("sha256:{digest}"),
                summary: Some("摘要".into()),
            }]
        );
    }

    #[test]
    fn rich_segment_without_complete_digest_keeps_artifact_but_not_strong_reference() {
        let mapped = map_segments(vec![NapCatMessageSegment::Rich {
            kind: qqbot::napcat::RichKind::Json,
            data: Some("preview only".into()),
            content_sha256: None,
            summary: Some("相同摘要".into()),
        }]);

        assert_eq!(
            mapped,
            vec![ContentSegment::Rich {
                kind: RichContentKind::Json,
                source_key: "rich_json".into(),
                summary: Some("相同摘要".into()),
            }]
        );
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
