use async_trait::async_trait;

use super::user_profile::UserProfile;

/// QQ 用户画像仓库 - 操作 qq_user_profiles 表
#[async_trait]
pub trait QqUserProfileRepoT: Send + Sync {
    /// 查找指定 QQ 用户的画像
    async fn find_by_qq_user_id(
        &self,
        qq_user_id: i64,
    ) -> Result<Option<UserProfile>, crate::shared::error::AppError>;
    /// 插入或更新画像
    async fn upsert(
        &self,
        profile: &UserProfile,
    ) -> Result<UserProfile, crate::shared::error::AppError>;
    /// 更新画像计数器（轻量，只涉及 total_messages / avg_message_length / emoji_usage_rate）
    async fn update_stats(
        &self,
        qq_user_id: i64,
        total_messages: u32,
        avg_message_length: f64,
        emoji_usage_rate: f64,
    ) -> Result<(), crate::shared::error::AppError>;
    /// 更新画像摘要时间
    async fn update_summary_at(
        &self,
        qq_user_id: i64,
        last_summary_at: i64,
    ) -> Result<(), crate::shared::error::AppError>;
}
