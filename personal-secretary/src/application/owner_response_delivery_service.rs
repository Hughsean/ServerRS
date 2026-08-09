use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    InboundEventStoreError, MessageSource, NotificationFailureKind, OwnerResponseDraft,
    SourceAccountRef,
};

const MAX_PLATFORM_MESSAGE_ID_BYTES: usize = 512;
const MAX_OWNER_ACTOR_ID_BYTES: usize = 191;

macro_rules! bounded_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, InboundEventStoreError> {
                let value = value.into();
                if value.trim().is_empty() || value.len() > 36 || !value.is_ascii() {
                    return Err(InboundEventStoreError::InvalidData(format!(
                        "{} must contain 1..=36 bytes",
                        $field
                    )));
                }
                Ok(Self(value))
            }

            pub fn generate() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

bounded_id!(OwnerResponseId, "owner response id");
bounded_id!(OwnerResponseLeaseToken, "owner response lease token");

/// Owner 被动回复的完整授权范围。托管账号、开放平台 Bot 和 Owner 身份缺一不可。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerResponseDeliveryScope {
    pub managed_account: SourceAccountRef,
    pub command_account: SourceAccountRef,
    pub owner_actor_id: String,
}

impl OwnerResponseDeliveryScope {
    pub fn new(
        managed_account: SourceAccountRef,
        command_account: SourceAccountRef,
        owner_actor_id: impl Into<String>,
    ) -> Result<Self, InboundEventStoreError> {
        let owner_actor_id = owner_actor_id.into();
        if command_account.channel != MessageSource::QqOpenPlatform {
            return Err(InboundEventStoreError::InvalidData(
                "owner responses require a QQ Open Platform command account".into(),
            ));
        }
        if owner_actor_id.trim().is_empty() || owner_actor_id.len() > MAX_OWNER_ACTOR_ID_BYTES {
            return Err(InboundEventStoreError::InvalidData(
                "owner response actor id must contain 1..=191 bytes".into(),
            ));
        }
        Ok(Self {
            managed_account,
            command_account,
            owner_actor_id,
        })
    }
}

/// 已通过数据库最终授权复验并取得 fencing 租约的 Owner 回复。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedOwnerResponse {
    pub response_id: OwnerResponseId,
    pub lease_token: OwnerResponseLeaseToken,
    pub draft: OwnerResponseDraft,
    /// 只能来自本回复对应的权威 QQ Gateway 事件。
    pub reply_to_platform_message_id: String,
    pub target: OwnerResponseTarget,
}

/// 回复目标由数据库中的权威 Gateway 原始事件恢复，调用方不能自行替换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerResponseTarget {
    C2c,
    Group { group_openid: String },
}

impl OwnerResponseTarget {
    pub fn group(group_openid: impl Into<String>) -> Result<Self, InboundEventStoreError> {
        let group_openid = group_openid.into();
        if group_openid.trim().is_empty() || group_openid.len() > 191 {
            return Err(InboundEventStoreError::InvalidData(
                "owner response group target must contain 1..=191 bytes".into(),
            ));
        }
        Ok(Self::Group { group_openid })
    }
}

#[async_trait]
pub trait OwnerResponseDeliveryStoreT: Send + Sync {
    async fn claim_pending_response(
        &self,
        scope: &OwnerResponseDeliveryScope,
        now_unix_secs: i64,
        lease_secs: u64,
        max_reply_age_secs: u64,
    ) -> Result<Option<ClaimedOwnerResponse>, InboundEventStoreError>;

    async fn mark_response_delivered(
        &self,
        response_id: &OwnerResponseId,
        lease_token: &OwnerResponseLeaseToken,
        platform_message_id: &str,
    ) -> Result<(), InboundEventStoreError>;

    async fn mark_response_failed(
        &self,
        response_id: &OwnerResponseId,
        lease_token: &OwnerResponseLeaseToken,
        error_code: &str,
        kind: NotificationFailureKind,
    ) -> Result<(), InboundEventStoreError>;
}

pub struct OwnerResponseDeliveryUseCase {
    store: Arc<dyn OwnerResponseDeliveryStoreT>,
    scope: OwnerResponseDeliveryScope,
}

impl OwnerResponseDeliveryUseCase {
    pub fn new(
        store: Arc<dyn OwnerResponseDeliveryStoreT>,
        scope: OwnerResponseDeliveryScope,
    ) -> Self {
        Self { store, scope }
    }

    pub async fn claim_pending_response(
        &self,
        now_unix_secs: i64,
        lease_secs: u64,
        max_reply_age_secs: u64,
    ) -> Result<Option<ClaimedOwnerResponse>, InboundEventStoreError> {
        if now_unix_secs < 0
            || !(1..=3600).contains(&lease_secs)
            || !(30..=300).contains(&max_reply_age_secs)
        {
            return Err(InboundEventStoreError::InvalidData(
                "owner response delivery bounds are invalid".into(),
            ));
        }
        self.store
            .claim_pending_response(&self.scope, now_unix_secs, lease_secs, max_reply_age_secs)
            .await
    }

    pub async fn mark_response_delivered(
        &self,
        response_id: &OwnerResponseId,
        lease_token: &OwnerResponseLeaseToken,
        platform_message_id: &str,
    ) -> Result<(), InboundEventStoreError> {
        if platform_message_id.trim().is_empty()
            || platform_message_id.len() > MAX_PLATFORM_MESSAGE_ID_BYTES
        {
            return Err(InboundEventStoreError::InvalidData(
                "owner response platform message id must contain 1..=512 bytes".into(),
            ));
        }
        self.store
            .mark_response_delivered(response_id, lease_token, platform_message_id)
            .await
    }

    pub async fn mark_response_failed(
        &self,
        response_id: &OwnerResponseId,
        lease_token: &OwnerResponseLeaseToken,
        error_code: &str,
        kind: NotificationFailureKind,
    ) -> Result<(), InboundEventStoreError> {
        if error_code.trim().is_empty() || error_code.len() > 64 {
            return Err(InboundEventStoreError::InvalidData(
                "owner response error code must contain 1..=64 bytes".into(),
            ));
        }
        self.store
            .mark_response_failed(response_id, lease_token, error_code, kind)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageSource;

    fn account(source: MessageSource, id: &str) -> SourceAccountRef {
        SourceAccountRef::new(source, id).unwrap()
    }

    #[test]
    fn scope_requires_a_bounded_owner_identity() {
        assert!(
            OwnerResponseDeliveryScope::new(
                account(MessageSource::NapCat, "managed"),
                account(MessageSource::QqOpenPlatform, "bot"),
                "owner",
            )
            .is_ok()
        );
        assert!(
            OwnerResponseDeliveryScope::new(
                account(MessageSource::NapCat, "managed"),
                account(MessageSource::QqOpenPlatform, "bot"),
                " ",
            )
            .is_err()
        );
        assert!(
            OwnerResponseDeliveryScope::new(
                account(MessageSource::NapCat, "managed"),
                account(MessageSource::NapCat, "not-an-official-bot"),
                "owner",
            )
            .is_err()
        );
    }

    #[test]
    fn generated_delivery_ids_are_bounded() {
        assert_eq!(OwnerResponseId::generate().as_str().len(), 36);
        assert_eq!(OwnerResponseLeaseToken::generate().as_str().len(), 36);
    }
}
