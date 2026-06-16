use async_trait::async_trait;

use crate::domain::qq_bot::relationship::RelationshipState;
use crate::shared::error::AppError;

/// 关系仓库接口
///
/// 管理猫猫与每个群友的关系状态，持久化到数据库。
#[async_trait]
pub trait RelationshipRepository: Send + Sync {
    /// 查找某群中某用户的关系
    async fn find(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
    ) -> Result<Option<RelationshipState>, AppError>;

    /// 创建或更新关系
    async fn upsert(&self, rel: &RelationshipState) -> Result<RelationshipState, AppError>;

    /// 获取某群所有用户的关系（用于 context building）
    async fn find_by_group(&self, qq_group_id: i64) -> Result<Vec<RelationshipState>, AppError>;

    /// 递增互动次数（原子操作，不返回完整对象）
    async fn increment_interaction(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
    ) -> Result<(), AppError>;
}
