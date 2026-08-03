use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ConnectionEpochId;

/// 一个消息实际由哪个接入通道观测到。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    NapCat,
    QqOpenPlatform,
}

impl MessageSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NapCat => "napcat",
            Self::QqOpenPlatform => "qq_open_platform",
        }
    }
}

/// 接入通道对消息的原始定位信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMessageRef {
    pub channel: MessageSource,
    /// 接收该消息的账号主体。不同个人 QQ 或不同开放平台 Bot 必须使用不同值。
    pub account_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceAccountRef {
    pub channel: MessageSource,
    pub account_id: String,
}

impl SourceAccountRef {
    pub fn new(
        channel: MessageSource,
        account_id: impl Into<String>,
    ) -> Result<Self, InboundIdentityError> {
        let value = Self {
            channel,
            account_id: account_id.into(),
        };
        require_non_empty("source.account_id", &value.account_id)?;
        Ok(value)
    }
}

impl SourceMessageRef {
    pub fn new(
        channel: MessageSource,
        account_id: impl Into<String>,
        message_id: impl Into<String>,
    ) -> Result<Self, InboundIdentityError> {
        let value = Self {
            channel,
            account_id: account_id.into(),
            message_id: message_id.into(),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), InboundIdentityError> {
        require_non_empty("source.account_id", &self.account_id)?;
        require_non_empty("source.message_id", &self.message_id)
    }

    /// 精确投递幂等键。通道和账号主体都参与计算，避免两个账号或两个协议误碰撞。
    pub fn idempotency_key(&self) -> IdempotencyKey {
        IdempotencyKey(format!(
            "{}:{}:{}:{}:{}",
            self.channel.as_str(),
            self.account_id.len(),
            self.account_id,
            self.message_id.len(),
            self.message_id
        ))
    }

    pub fn account_ref(&self) -> SourceAccountRef {
        SourceAccountRef {
            channel: self.channel,
            account_id: self.account_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Private,
    Group,
    /// 用户与官方 Bot 之间经过配置绑定的控制会话。
    OwnerControl,
}

impl ConversationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Group => "group",
            Self::OwnerControl => "owner_control",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationRef {
    pub kind: ConversationKind,
    pub id: String,
}

impl ConversationRef {
    pub fn new(
        kind: ConversationKind,
        id: impl Into<String>,
    ) -> Result<Self, InboundIdentityError> {
        let value = Self {
            kind,
            id: id.into(),
        };
        require_non_empty("conversation.id", &value.id)?;
        Ok(value)
    }
}

/// 由配置绑定或平台签名验证后的发送者身份，而不是从消息文本猜测的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifiedActorKind {
    Owner,
    OfficialBot,
    External,
}

impl VerifiedActorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::OfficialBot => "official_bot",
            Self::External => "external",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedActor {
    pub kind: VerifiedActorKind,
    pub id: String,
}

impl VerifiedActor {
    pub fn new(
        kind: VerifiedActorKind,
        id: impl Into<String>,
    ) -> Result<Self, InboundIdentityError> {
        let value = Self {
            kind,
            id: id.into(),
        };
        require_non_empty("actor.id", &value.id)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// 唯一允许驱动秘书执行动作的入站角色。
    OwnerCommand,
    /// 主人在个人 QQ 中发送的内容，只作为事实背景和对话轨迹。
    OwnerObservation,
    ExternalObservation,
    AssistantOutput,
}

impl MessageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnerCommand => "owner_command",
            Self::OwnerObservation => "owner_observation",
            Self::ExternalObservation => "external_observation",
            Self::AssistantOutput => "assistant_output",
        }
    }
}

/// 协议无关的结构化消息段。业务层依赖这些类型，而不是 CQ/OneBot 载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentSegment {
    Text {
        content: String,
    },
    Mention {
        actor_id: String,
    },
    MentionAll,
    Reply {
        platform_message_id: String,
    },
    Face {
        source_id: String,
        display_text: Option<String>,
    },
    Media {
        kind: MediaKind,
        source_key: String,
        source_url: Option<String>,
        display_name: Option<String>,
    },
    Forward {
        source_key: String,
    },
    Rich {
        kind: RichContentKind,
        source_key: String,
        summary: Option<String>,
    },
    Unknown {
        protocol_value: String,
    },
}

impl ContentSegment {
    fn validate(&self) -> Result<(), InboundIdentityError> {
        match self {
            Self::Mention { actor_id } => require_non_empty("segment.mention.actor_id", actor_id),
            Self::Reply {
                platform_message_id,
            } => require_non_empty("segment.reply.platform_message_id", platform_message_id),
            Self::Media { source_key, .. } => {
                require_non_empty("segment.media.source_key", source_key)
            }
            Self::Forward { source_key } => {
                require_non_empty("segment.forward.source_key", source_key)
            }
            Self::Rich { source_key, .. } => {
                require_non_empty("segment.rich.source_key", source_key)
            }
            Self::Text { .. } | Self::MentionAll | Self::Face { .. } | Self::Unknown { .. } => {
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Audio,
    Video,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RichContentKind {
    Json,
    Xml,
    Card,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMessageEnvelope {
    pub source: SourceMessageRef,
    pub conversation: ConversationRef,
    pub actor: VerifiedActor,
    pub occurred_at_unix_secs: i64,
    /// 可为空，例如只有图片、文件或语音的消息；原始附件以后通过 artifact 引用承载。
    pub normalized_text: String,
    pub segments: Vec<ContentSegment>,
    /// 产生该观察的传输连接周期；历史回补事件允许为空。
    pub connection_epoch_id: Option<ConnectionEpochId>,
    /// 发送者的观察档案（昵称/群名片/群角色）。只用于显示与指代候选，
    /// 绝不构成授权；身份权威仍是 `actor`（SourceAccountRef + 稳定主体 ID）。
    pub sender_profile: Option<ObservedSenderProfile>,
}

/// 发送者观察档案（ID-005）。仅信封级显示信息，不保存正文；
/// `never_long_term` 会话的观察不进入人物长期上下文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedSenderProfile {
    /// 平台昵称（有界）。
    pub nickname: String,
    /// 群名片（群消息可带；私聊为 None）。
    pub group_card: Option<String>,
    /// 群角色协议原值（'owner'/'admin'/'member'/其他）；由领域层
    /// `GroupRole::parse_protocol` 解析，未知值一律视为 Unknown。
    pub group_role: Option<String>,
}

impl ObservedSenderProfile {
    /// 昵称、群名片有界（与领域 `MAX_ATTRIBUTE_VALUE_CHARS` 对齐），群角色原值有界。
    pub fn validate(&self) -> Result<(), InboundIdentityError> {
        if self.nickname.chars().count() > 200 {
            return Err(InboundIdentityError::Invalid(
                "sender_profile.nickname exceeds 200 chars".into(),
            ));
        }
        if self
            .group_card
            .as_ref()
            .is_some_and(|card| card.chars().count() > 200)
        {
            return Err(InboundIdentityError::Invalid(
                "sender_profile.group_card exceeds 200 chars".into(),
            ));
        }
        if self
            .group_role
            .as_ref()
            .is_some_and(|role| role.chars().count() > 16)
        {
            return Err(InboundIdentityError::Invalid(
                "sender_profile.group_role exceeds 16 chars".into(),
            ));
        }
        Ok(())
    }
}

impl InboundMessageEnvelope {
    pub fn new(
        source: SourceMessageRef,
        conversation: ConversationRef,
        actor: VerifiedActor,
        occurred_at_unix_secs: i64,
        normalized_text: impl Into<String>,
        segments: Vec<ContentSegment>,
    ) -> Result<Self, InboundIdentityError> {
        let value = Self {
            source,
            conversation,
            actor,
            occurred_at_unix_secs,
            normalized_text: normalized_text.into(),
            segments,
            connection_epoch_id: None,
            sender_profile: None,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn observed_in(mut self, connection_epoch_id: ConnectionEpochId) -> Self {
        self.connection_epoch_id = Some(connection_epoch_id);
        self
    }

    /// 附加发送者观察档案；档案无效时返回错误（fail-closed，不静默丢弃显示数据）。
    pub fn with_sender_profile(
        mut self,
        profile: Option<ObservedSenderProfile>,
    ) -> Result<Self, InboundIdentityError> {
        if let Some(ref profile) = profile {
            profile.validate()?;
        }
        self.sender_profile = profile;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), InboundIdentityError> {
        self.source.validate()?;
        require_non_empty("conversation.id", &self.conversation.id)?;
        require_non_empty("actor.id", &self.actor.id)?;
        if self.occurred_at_unix_secs < 0 {
            return Err(InboundIdentityError::NegativeTimestamp(
                self.occurred_at_unix_secs,
            ));
        }
        for segment in &self.segments {
            segment.validate()?;
        }
        if let Some(ref profile) = self.sender_profile {
            profile.validate()?;
        }
        Ok(())
    }

    /// 指令权限由可信来源、绑定会话和发送者身份共同决定，绝不从自然语言内容推断。
    pub fn role(&self) -> MessageRole {
        match (self.source.channel, self.conversation.kind, self.actor.kind) {
            (
                MessageSource::QqOpenPlatform,
                ConversationKind::OwnerControl,
                VerifiedActorKind::Owner,
            ) => MessageRole::OwnerCommand,
            (_, _, VerifiedActorKind::Owner) => MessageRole::OwnerObservation,
            (_, _, VerifiedActorKind::OfficialBot) => MessageRole::AssistantOutput,
            (_, _, VerifiedActorKind::External) => MessageRole::ExternalObservation,
        }
    }

    pub fn accepts_instructions(&self) -> bool {
        self.role() == MessageRole::OwnerCommand
    }

    pub fn idempotency_key(&self) -> IdempotencyKey {
        self.source.idempotency_key()
    }

    pub fn mentioned_actor_ids(&self) -> impl Iterator<Item = &str> {
        self.segments.iter().filter_map(|segment| match segment {
            ContentSegment::Mention { actor_id } => Some(actor_id.as_str()),
            _ => None,
        })
    }

    pub fn mentions_all(&self) -> bool {
        self.segments
            .iter()
            .any(|segment| matches!(segment, ContentSegment::MentionAll))
    }

    pub fn reply_to_platform_message_id(&self) -> Option<&str> {
        self.segments.iter().find_map(|segment| match segment {
            ContentSegment::Reply {
                platform_message_id,
            } => Some(platform_message_id.as_str()),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InboundIdentityError {
    #[error("{0} must not be empty")]
    EmptyField(&'static str),
    #[error("occurred_at_unix_secs must not be negative, got {0}")]
    NegativeTimestamp(i64),
    #[error("invalid inbound value: {0}")]
    Invalid(String),
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), InboundIdentityError> {
    if value.trim().is_empty() {
        return Err(InboundIdentityError::EmptyField(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(
        channel: MessageSource,
        conversation_kind: ConversationKind,
        actor_kind: VerifiedActorKind,
    ) -> InboundMessageEnvelope {
        InboundMessageEnvelope::new(
            SourceMessageRef::new(channel, "account-1", "message-1").unwrap(),
            ConversationRef::new(conversation_kind, "conversation-1").unwrap(),
            VerifiedActor::new(actor_kind, "actor-1").unwrap(),
            100,
            "测试消息",
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn only_bound_owner_control_message_is_an_instruction() {
        let command = envelope(
            MessageSource::QqOpenPlatform,
            ConversationKind::OwnerControl,
            VerifiedActorKind::Owner,
        );
        assert_eq!(command.role(), MessageRole::OwnerCommand);
        assert!(command.accepts_instructions());

        let napcat_owner = envelope(
            MessageSource::NapCat,
            ConversationKind::Private,
            VerifiedActorKind::Owner,
        );
        assert_eq!(napcat_owner.role(), MessageRole::OwnerObservation);
        assert!(!napcat_owner.accepts_instructions());
    }

    #[test]
    fn external_sender_cannot_gain_authority_from_control_conversation() {
        let message = envelope(
            MessageSource::QqOpenPlatform,
            ConversationKind::OwnerControl,
            VerifiedActorKind::External,
        );
        assert_eq!(message.role(), MessageRole::ExternalObservation);
        assert!(!message.accepts_instructions());
    }

    #[test]
    fn idempotency_key_is_scoped_by_channel_and_account_subject() {
        let napcat_a = SourceMessageRef::new(MessageSource::NapCat, "account-a", "42").unwrap();
        let napcat_b = SourceMessageRef::new(MessageSource::NapCat, "account-b", "42").unwrap();
        let official =
            SourceMessageRef::new(MessageSource::QqOpenPlatform, "account-a", "42").unwrap();

        assert_ne!(napcat_a.idempotency_key(), napcat_b.idempotency_key());
        assert_ne!(napcat_a.idempotency_key(), official.idempotency_key());
        assert_eq!(napcat_a.idempotency_key(), napcat_a.idempotency_key());
    }

    #[test]
    fn empty_identity_is_rejected_at_the_boundary() {
        let error = SourceMessageRef::new(MessageSource::NapCat, " ", "42").unwrap_err();
        assert_eq!(error, InboundIdentityError::EmptyField("source.account_id"));
    }

    #[test]
    fn mentions_and_reply_remain_structured_after_normalization() {
        let message = InboundMessageEnvelope::new(
            SourceMessageRef::new(MessageSource::NapCat, "account-1", "message-2").unwrap(),
            ConversationRef::new(ConversationKind::Group, "group-1").unwrap(),
            VerifiedActor::new(VerifiedActorKind::External, "sender-1").unwrap(),
            101,
            "@user 请看上面的结论",
            vec![
                ContentSegment::Mention {
                    actor_id: "member-1".into(),
                },
                ContentSegment::MentionAll,
                ContentSegment::Reply {
                    platform_message_id: "message-1".into(),
                },
            ],
        )
        .unwrap();

        assert_eq!(
            message.mentioned_actor_ids().collect::<Vec<_>>(),
            ["member-1"]
        );
        assert!(message.mentions_all());
        assert_eq!(message.reply_to_platform_message_id(), Some("message-1"));
    }
}
