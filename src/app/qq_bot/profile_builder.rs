use std::sync::Arc;

use chrono::Utc;
use tracing::{error, info};

use crate::domain::llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use crate::domain::qq_bot::config::ExternalUser;
use crate::domain::qq_bot::message::{MessageSegment, NormalizedMessage};
use crate::domain::qq_bot::qq_profile_repository::QqUserProfileRepository;
use crate::domain::qq_bot::repository::{
    ExternalUserRepository, GroupMemory, GroupMemoryRepository, GroupMessageRepository,
};
use crate::domain::qq_bot::user_profile::UserProfile;
use crate::domain::qq_bot::QqBotError;
use crate::domain::user::user::NewUser;
use crate::domain::user::user_repository::UserRepository;

/// 画像构建服务配置
#[derive(Debug, Clone)]
pub struct ProfileBuilderConfig {
    /// 触发用户画像 LLM 构建的消息数阈值
    pub user_profile_threshold: u32,
    /// 触发群聊画像 LLM 构建的消息数阈值
    pub group_profile_threshold: u32,
}

impl Default for ProfileBuilderConfig {
    fn default() -> Self {
        Self {
            user_profile_threshold: 50,
            group_profile_threshold: 500,
        }
    }
}

/// 后台画像构建服务
///
/// 职责：
/// 1. 观察消息流，更新用户计数器（total_messages, avg_message_length, emoji 使用率）
/// 2. 首次发现 QQ 用户时自动注册到平台 users 表
/// 3. 达到阈值后调用 LLM 构建用户画像（兴趣标签、说话风格）
/// 4. 达到阈值后调用 LLM 构建群聊画像（群记忆条目）
///
/// 注意：用户级别的 LLM 画像依赖 `recent_by_user` 查询，当前暂用维护一个
/// 简化版本，仅聚焦轻量统计和群聊级画像。
pub struct ProfileBuilder {
    user_repo: Arc<dyn UserRepository>,
    external_user_repo: Arc<dyn ExternalUserRepository>,
    user_profile_repo: Arc<dyn QqUserProfileRepository>,
    group_memory_repo: Arc<dyn GroupMemoryRepository>,
    message_repo: Arc<dyn GroupMessageRepository>,
    llm_provider: Arc<dyn LlmProvider>,
    config: ProfileBuilderConfig,
}

impl ProfileBuilder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        external_user_repo: Arc<dyn ExternalUserRepository>,
        user_profile_repo: Arc<dyn QqUserProfileRepository>,
        group_memory_repo: Arc<dyn GroupMemoryRepository>,
        message_repo: Arc<dyn GroupMessageRepository>,
        llm_provider: Arc<dyn LlmProvider>,
        config: ProfileBuilderConfig,
    ) -> Self {
        Self {
            user_repo,
            external_user_repo,
            user_profile_repo,
            group_memory_repo,
            message_repo,
            llm_provider,
            config,
        }
    }

    // ─── 公共 API ───────────────────────────────────────────────────────

    /// 确保 QQ 用户已注册到平台 users 表（首次见面自动注册）
    ///
    /// 如果 qq_external_users 中不存在该用户，则：
    /// 1. 创建 users 记录（password = null, username = "qq_{qq_user_id}"）
    /// 2. 创建 qq_external_users 记录并关联 internal_user_id
    pub async fn ensure_user_registered(
        &self,
        qq_user_id: i64,
        nickname: Option<&str>,
        now_ms: i64,
    ) -> Result<ExternalUser, QqBotError> {
        // 查找是否已注册
        if let Some(existing) = self.external_user_repo.find_by_qq_user_id(qq_user_id).await
            .map_err(|e| QqBotError::Internal(format!("failed to find external user: {e}")))?
        {
            // 已有记录但没有关联内部用户 → 补创建
            if existing.internal_user_id.is_none() {
                let new_user = self.create_platform_user(qq_user_id, nickname).await?;
                let updated = ExternalUser {
                    internal_user_id: Some(new_user.id),
                    ..existing
                };
                return self.external_user_repo.upsert(&updated).await
                    .map_err(|e| QqBotError::Internal(format!("failed to update external user: {e}")));
            }
            return Ok(existing);
        }

        // 全新用户
        let new_user = self.create_platform_user(qq_user_id, nickname).await?;

        let external = ExternalUser {
            qq_user_id,
            internal_user_id: Some(new_user.id),
            nickname: nickname.map(|s| s.to_string()),
            avatar_url: None,
            last_seen_at: Some(now_ms),
            memory_enabled: false,
            persona_enabled: false,
        };
        let external = self.external_user_repo.upsert(&external).await
            .map_err(|e| QqBotError::Internal(format!("failed to create external user: {e}")))?;

        // 同时创建初始画像记录
        let _ = self.user_profile_repo.upsert(&UserProfile {
            qq_user_id,
            interest_tags: None,
            active_hours: None,
            speaking_style: None,
            topic_frequency: None,
            total_messages: 0,
            avg_message_length: 0.0,
            emoji_usage_rate: 0.0,
            first_seen_at: Some(now_ms),
            last_summary_at: None,
            raw_profile: None,
        }).await;

        info!(
            qq_user_id,
            platform_user_id = new_user.id,
            "QQ user auto-registered"
        );

        Ok(external)
    }

    /// 轻量观察消息 — 每次消息到达时调用，更新计数器
    pub async fn observe_message(&self, msg: &NormalizedMessage) {
        let qq_user_id = match msg.qq_user_id {
            Some(id) => id,
            None => return,
        };

        // 加载或创建画像记录
        let profile = self.user_profile_repo.find_by_qq_user_id(qq_user_id).await
            .ok()
            .flatten()
            .unwrap_or(UserProfile {
                qq_user_id,
                interest_tags: None,
                active_hours: None,
                speaking_style: None,
                topic_frequency: None,
                total_messages: 0,
                avg_message_length: 0.0,
                emoji_usage_rate: 0.0,
                first_seen_at: Some(msg.sent_at),
                last_summary_at: None,
                raw_profile: None,
            });

        let msg_len = msg.normalized_text.len() as f64;
        let has_emoji = msg.segments.iter().any(|s| matches!(s, MessageSegment::Face { .. }))
            || msg.normalized_text.contains(&['😀', '😂', '😊', '🥰', '😍', '🤔', '😭', '😤', '👍', '❤', '🔥', '✨', '🎉', '💀', '🙏'][..]);

        let total = profile.total_messages + 1;
        let old_avg = profile.avg_message_length;
        let new_avg = old_avg + (msg_len - old_avg) / total as f64;
        let old_emoji = profile.emoji_usage_rate;
        let new_emoji = old_emoji + (if has_emoji { 1.0 } else { 0.0 } - old_emoji) / total as f64;

        // 更新计数器（轻量，不阻塞）
        if let Err(e) = self.user_profile_repo.update_stats(qq_user_id, total, new_avg, new_emoji).await {
            error!(qq_user_id, error = %e, "更新画像统计数据失败");
        }

        // 更新 external_user 的 last_seen_at
        let _ = self.external_user_repo.update_last_seen(qq_user_id, msg.sent_at).await;

        // 达到阈值 → 后台构建用户画像（使用群消息样本）
        if total == self.config.user_profile_threshold
            || (total > self.config.user_profile_threshold && total % self.config.user_profile_threshold == 0)
        {
            let builder = CloneInner {
                user_profile_repo: Arc::clone(&self.user_profile_repo),
                message_repo: Arc::clone(&self.message_repo),
                group_memory_repo: Arc::clone(&self.group_memory_repo),
                llm_provider: Arc::clone(&self.llm_provider),
            };
            tokio::spawn(async move {
                if let Err(e) = builder.build_user_profile_from_group(qq_user_id).await {
                    error!(qq_user_id, error = %e, "用户画像构建失败");
                }
            });
        }

        // 群聊级阈值检查
        let group_id = msg.qq_group_id;
        let msg_count_check = total;
        if msg_count_check > 0 && msg_count_check % self.config.group_profile_threshold == 0 {
            let builder = CloneInner {
                user_profile_repo: Arc::clone(&self.user_profile_repo),
                message_repo: Arc::clone(&self.message_repo),
                group_memory_repo: Arc::clone(&self.group_memory_repo),
                llm_provider: Arc::clone(&self.llm_provider),
            };
            tokio::spawn(async move {
                if let Err(e) = builder.build_group_profile(group_id).await {
                    error!(group_id, error = %e, "group profile build failed");
                }
            });
        }
    }

    /// 手动触发群聊画像构建
    pub async fn build_group_profile_manual(&self, qq_group_id: i64) -> Result<(), QqBotError> {
        let inner = CloneInner {
            user_profile_repo: Arc::clone(&self.user_profile_repo),
            message_repo: Arc::clone(&self.message_repo),
            group_memory_repo: Arc::clone(&self.group_memory_repo),
            llm_provider: Arc::clone(&self.llm_provider),
        };
        inner.build_group_profile(qq_group_id).await
    }

    /// 定时清理任务 — 每日调用
    pub async fn cleanup(&self) -> Result<(), QqBotError> {
        info!("profile cleanup tick (no-op)");
        Ok(())
    }

    // ─── 内部辅助 ──────────────────────────────────────────────────────

    async fn create_platform_user(&self, qq_user_id: i64, nickname: Option<&str>) -> Result<crate::domain::user::user::User, QqBotError> {
        let username = format!("qq_{}", qq_user_id);
        let new_user = NewUser::new_without_password(
            &username,
            nickname.map(|s| s.to_string()),
        );
        self.user_repo.save(new_user).await
            .map_err(|e| QqBotError::Internal(format!("failed to create platform user for QQ {}: {}", qq_user_id, e)))
    }
}

/// 用于 tokio::spawn 的轻量克隆
struct CloneInner {
    user_profile_repo: Arc<dyn QqUserProfileRepository>,
    message_repo: Arc<dyn GroupMessageRepository>,
    group_memory_repo: Arc<dyn GroupMemoryRepository>,
    llm_provider: Arc<dyn LlmProvider>,
}

impl CloneInner {
    /// 从群消息样本构建用户画像
    async fn build_user_profile_from_group(&self, qq_user_id: i64) -> Result<(), QqBotError> {
        // 获取该用户所属群的最近消息（简化：取全部活跃群）
        // 注：更精确的实现应新增 `GroupMessageRepository::recent_by_user` 方法
        // 当前简化版本从各群消息中过滤
        let messages = self.message_repo.recent_by_group(0, 100).await
            .map_err(|e| QqBotError::Internal(format!("failed to fetch messages: {e}")))?;

        let user_msgs: Vec<&NormalizedMessage> = messages.iter()
            .filter(|m| m.qq_user_id == Some(qq_user_id))
            .take(100)
            .collect();

        if user_msgs.is_empty() {
            return Ok(());
        }

        let sample: Vec<String> = user_msgs.iter()
            .map(|m| format!("[群{}] {}", m.qq_group_id % 10000, m.normalized_text))
            .collect();
        let conversation_sample = sample.join("\n");

        let prompt = format!(
            r#"你是一个用户画像分析助手。请根据以下 QQ 用户的近期聊天记录，分析该用户的画像。

返回 JSON（不要包含其他文字）：
{{
  "interest_tags": ["编程", "二次元", "音乐"],
  "speaking_style": "简洁",
  "topic_frequency": {{"游戏": 15, "学习": 3, "美食": 8}},
  "raw_profile": "一段自然语言描述（100字以内）"
}}

topic_frequency 是该用户常聊的话题及其估计次数，最多 5 个话题。

聊天记录：
{}"#,
            conversation_sample
        );

        let request = ChatCompletionRequest::new(vec![
            ChatMessage { role: "system".into(), content: prompt, tool_calls: None, tool_call_id: None, name: None },
        ]).with_temperature(0.3);

        let response = self.llm_provider.chat(request).await
            .map_err(|e| QqBotError::Internal(format!("LLM profile build failed: {e}")))?;

        if let Ok(parsed) = Self::parse_json_response(&response.content) {
            let profile = UserProfile {
                qq_user_id,
                interest_tags: parsed.get("interest_tags")
                    .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok()),
                active_hours: None,
                speaking_style: parsed.get("speaking_style")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
                topic_frequency: parsed.get("topic_frequency")
                    .and_then(|v| {
                        if v.is_object() { Some(v.clone()) } else { None }
                    }),
                total_messages: 0,
                avg_message_length: 0.0,
                emoji_usage_rate: 0.0,
                first_seen_at: None,
                last_summary_at: Some(Utc::now().timestamp_millis()),
                raw_profile: parsed.get("raw_profile")
                    .and_then(|v| v.as_str().map(|s| s.to_string())),
            };
            self.user_profile_repo.upsert(&profile).await
                .map_err(|e| QqBotError::Internal(format!("failed to save profile: {e}")))?;
            info!(qq_user_id, "user profile built via LLM");
        }

        Ok(())
    }

    /// 构建群聊画像（写入 qq_group_memories）
    async fn build_group_profile(&self, qq_group_id: i64) -> Result<(), QqBotError> {
        let messages = self.message_repo.recent_by_group(qq_group_id, 100).await
            .map_err(|e| QqBotError::Internal(format!("failed to fetch messages: {e}")))?;

        if messages.is_empty() {
            return Ok(());
        }

        let sample: Vec<String> = messages.iter()
            .map(|m| {
                let user = m.qq_user_id.unwrap_or(0) % 10000;
                format!("[User{}] {}", user, m.normalized_text)
            })
            .collect();
        let conversation_sample = sample.join("\n");

        let prompt = format!(
            r#"你是一个群聊分析助手。从以下聊天记录中提取群聊画像。

返回 JSON 数组（可多条），每条格式：
{{
  "memory_type": "recurring_topic",
  "content": "群友们每天晚上讨论游戏",
  "confidence": 0.8,
  "salience": 0.7
}}

memory_type 可选：group_preference / group_fact / group_rule / recurring_topic / inside_joke
如果没什么值得记录的，返回 [{{"skip": true}}]

聊天记录：
{}"#,
            conversation_sample
        );

        let request = ChatCompletionRequest::new(vec![
            ChatMessage { role: "system".into(), content: prompt, tool_calls: None, tool_call_id: None, name: None },
        ]).with_temperature(0.3);

        let response = self.llm_provider.chat(request).await
            .map_err(|e| QqBotError::Internal(format!("LLM group profile failed: {e}")))?;

        let cleaned = Self::clean_json(&response.content);
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&cleaned) {
            let items = parsed.as_array().cloned().unwrap_or_default();
            for item in &items {
                if item.get("skip").and_then(|v| v.as_bool()).unwrap_or(false) {
                    info!(qq_group_id, "group profile: LLM skipped");
                    return Ok(());
                }
                let memory = GroupMemory {
                    group_memory_id: None,
                    qq_group_id,
                    memory_key: None,
                    canonical_form: None,
                    memory_type: item.get("memory_type").and_then(|v| v.as_str()).unwrap_or("group_fact").to_string(),
                    content: item.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    confidence: item.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.5),
                    salience: item.get("salience").and_then(|v| v.as_f64()).unwrap_or(0.5),
                    source_message_id: None,
                    reinforce_count: 0,
                    status: 1,
                };
                if !memory.content.is_empty() {
                    self.group_memory_repo.upsert(&memory).await
                        .map_err(|e| QqBotError::Internal(format!("failed to save group memory: {e}")))?;
                }
            }
            info!(qq_group_id, count = items.len(), "group profile built");
        }

        Ok(())
    }

    fn clean_json(raw: &str) -> String {
        raw.trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
            .to_string()
    }

    fn parse_json_response(content: &str) -> Result<serde_json::Value, String> {
        let cleaned = Self::clean_json(content);
        serde_json::from_str(&cleaned).map_err(|e| format!("JSON parse error: {e}"))
    }
}
