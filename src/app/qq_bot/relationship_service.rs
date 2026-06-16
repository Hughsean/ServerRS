use std::sync::Arc;

use tracing::info;

use crate::domain::qq_bot::relationship::RelationshipState;
use crate::domain::qq_bot::relationship_repository::RelationshipRepository;
use crate::shared::error::AppError;

/// 关系服务 — 管理猫猫与每个群友的社交关系
///
/// 职责：
/// - 每次收到消息时更新互动计数
/// - 构建关系上下文字符串（供 context_builder 使用）
/// - 从 LLM 回复中解析昵称偏好、兴趣等
pub struct RelationshipService {
    repo: Arc<dyn RelationshipRepository>,
}

impl RelationshipService {
    pub fn new(repo: Arc<dyn RelationshipRepository>) -> Self {
        Self { repo }
    }

    /// 每次收到消息时调用 → 递增互动计数 + 更新熟悉度
    pub async fn update_interaction(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
        _sent_at: Option<i64>,
    ) -> Result<(), AppError> {
        self.repo
            .increment_interaction(qq_group_id, qq_user_id)
            .await
    }

    /// 构建关系上下文字符串（供 context_builder 注入 LLM prompt）
    ///
    /// 只返回与参与者列表中有互动历史的关系信息，避免注入不必要的噪音。
    pub async fn build_relationship_context(
        &self,
        qq_group_id: i64,
        participant_ids: &[i64],
    ) -> String {
        // 获取群所有关系
        let all_rels = match self.repo.find_by_group(qq_group_id).await {
            Ok(rels) => rels,
            Err(e) => {
                tracing::warn!(group_id = qq_group_id, error = %e, "加载关系失败");
                return String::new();
            }
        };

        // 只保留与当前参与者有关且熟悉度 >= 0.3 的关系
        let relevant: Vec<&RelationshipState> = all_rels
            .iter()
            .filter(|r| participant_ids.contains(&r.qq_user_id) && r.familiarity >= 0.3)
            .collect();

        if relevant.is_empty() {
            return String::new();
        }

        let mut parts = Vec::new();
        parts.push("## 群友关系".to_string());
        for rel in &relevant {
            parts.push(rel.describe());
        }

        parts.join("\n")
    }

    /// 获取某个群友的知名关系信息（如果熟悉度足够）
    pub async fn get_relationship(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
    ) -> Result<Option<RelationshipState>, AppError> {
        self.repo.find(qq_group_id, qq_user_id).await
    }

    /// 更新某群友的昵称偏好（从 LLM 回复中解析）
    pub async fn update_nickname_preference(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
        nickname: String,
    ) -> Result<(), AppError> {
        let rel = self.repo.find(qq_group_id, qq_user_id).await?;
        if let Some(mut rel) = rel {
            rel.nickname_preference = Some(nickname);
            self.repo.upsert(&rel).await?;
            info!(
                group_id = qq_group_id,
                user_id = qq_user_id,
                nickname = %rel.nickname_preference.as_deref().unwrap_or(""),
                "updated nickname preference"
            );
        }
        Ok(())
    }

    /// 更新某群友的已知兴趣和应回避话题（从 LLM 回复中解析）
    pub async fn update_known_info(
        &self,
        qq_group_id: i64,
        qq_user_id: i64,
        interests: &[String],
        avoid_topics: &[String],
    ) -> Result<(), AppError> {
        let rel = self.repo.find(qq_group_id, qq_user_id).await?;
        if let Some(mut rel) = rel {
            for interest in interests {
                if !rel.known_interests.contains(interest) {
                    rel.known_interests.push(interest.clone());
                }
            }
            for topic in avoid_topics {
                if !rel.known_avoid_topics.contains(topic) {
                    rel.known_avoid_topics.push(topic.clone());
                }
            }
            self.repo.upsert(&rel).await?;
            info!(
                group_id = qq_group_id,
                user_id = qq_user_id,
                interests_count = rel.known_interests.len(),
                "updated known interests and avoid topics"
            );
        }
        Ok(())
    }
}
