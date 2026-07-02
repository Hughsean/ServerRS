use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::app::qq_bot::context_builder::ContextBuilder;
use crate::app::qq_bot::emotional_state_service::EmotionalStateService;
use crate::app::qq_bot::reply_generator::ReplyGenerator;
use crate::app::qq_bot::segment_dispatcher::SegmentDispatcher;
use crate::app::qq_bot::topic_service::TopicService;
use crate::domain::llm::{ChatCompletionRequest, LlmProvider};
use crate::domain::qq_bot::config::GroupConfig;
use crate::domain::qq_bot::message::NormalizedMessage;
use crate::domain::qq_bot::proactive::{ProactiveAction, ProactiveIntent};
use crate::domain::qq_bot::repository::{GroupMessageRepository, GroupRepository};
use crate::domain::qq_bot::{AttentionStore, QqBotError};

/// 主动行为评估器 — 后台轮询，判断猫猫是否需要主动说话
///
/// 两层设计：
/// - Layer 1：零 token 规则过滤器（检查最近消息模式）
/// - Layer 2：轻量 LLM 语义判断（max_tokens=50, temp=0.3）
///
/// 通过后直接走 ContextBuilder + ReplyGenerator 管道生成回复并分发。
pub struct ProactiveEvaluator {
    // Repositories
    group_repo: Arc<dyn GroupRepository>,
    message_repo: Arc<dyn GroupMessageRepository>,

    // Core services
    context_builder: Arc<ContextBuilder>,
    reply_generator: Arc<ReplyGenerator>,
    segment_dispatcher: Arc<SegmentDispatcher>,

    // Supporting services
    attention_store: Arc<dyn AttentionStore>,
    topic_service: Arc<TopicService>,
    emotional_service: Arc<EmotionalStateService>,

    // LLM for Layer 2 judgment
    llm_provider: Arc<dyn LlmProvider>,

    // Config
    #[allow(dead_code)]
    bot_qq_id: i64,
    bot_account_id: u64,
    check_interval: Duration,
    proactive_cooldown: Duration,

    /// 群 → 上次主动行为的时间戳（秒）
    last_proactive: Arc<RwLock<HashMap<i64, i64>>>,
}

impl ProactiveEvaluator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        group_repo: Arc<dyn GroupRepository>,
        message_repo: Arc<dyn GroupMessageRepository>,
        context_builder: Arc<ContextBuilder>,
        reply_generator: Arc<ReplyGenerator>,
        segment_dispatcher: Arc<SegmentDispatcher>,
        attention_store: Arc<dyn AttentionStore>,
        topic_service: Arc<TopicService>,
        emotional_service: Arc<EmotionalStateService>,
        llm_provider: Arc<dyn LlmProvider>,
        bot_qq_id: i64,
        bot_account_id: u64,
        check_interval: Duration,
        proactive_cooldown: Duration,
    ) -> Self {
        Self {
            group_repo,
            message_repo,
            context_builder,
            reply_generator,
            segment_dispatcher,
            attention_store,
            topic_service,
            emotional_service,
            llm_provider,
            bot_qq_id,
            bot_account_id,
            check_interval,
            proactive_cooldown,
            last_proactive: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 启动主动行为轮询（后台任务入口）
    pub async fn run(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(self.check_interval);
        info!(
            interval_secs = self.check_interval.as_secs(),
            "proactive evaluator started"
        );

        loop {
            ticker.tick().await;
            if let Err(e) = self.tick().await {
                warn!(error = %e, "proactive evaluator tick failed");
            }
        }
    }

    /// 一轮检查：遍历所有启用 + allow_proactive 的群
    async fn tick(&self) -> Result<(), QqBotError> {
        let groups = self
            .group_repo
            .find_enabled_by_bot(self.bot_account_id)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to load groups: {e}")))?;

        let now = chrono::Utc::now().timestamp();

        for group in &groups {
            if !group.enabled || !group.reply_policy.allow_proactive {
                continue;
            }

            // Check proactive cooldown
            let last_map = self.last_proactive.read().await;
            let last_time = last_map.get(&group.qq_group_id).copied().unwrap_or(0);
            drop(last_map);

            if now - last_time < self.proactive_cooldown.as_secs() as i64 {
                continue; // Still in cooldown
            }

            // Evaluate this group
            if let Err(e) = self.evaluate_group(group, now).await {
                warn!(
                    group_id = group.qq_group_id,
                    error = %e,
                    "proactive evaluation failed for group"
                );
            }
        }

        Ok(())
    }

    /// 对单个群执行主动行为评估
    async fn evaluate_group(&self, group: &GroupConfig, now: i64) -> Result<(), QqBotError> {
        let group_id = group.qq_group_id;

        // 获取最近消息
        let recent = self
            .message_repo
            .recent_by_group(group_id, 5)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to load recent messages: {e}")))?;

        if recent.is_empty() {
            return Ok(()); // 无消息，跳过
        }

        // ── Layer 1：规则过滤器 ──────────────────────────────────
        let layer1_result = self.layer1_filter(group_id, &recent, now);

        let action = match layer1_result {
            Some(a) => a,
            None => return Ok(()), // 规则层未命中
        };

        // ── Layer 2：LLM 语义判断 ─────────────────────────────────
        let should_proceed = self.layer2_judge(group_id, &recent, &action).await?;

        if !should_proceed {
            return Ok(());
        }

        // ── 生成并发送主动回复 ────────────────────────────────────
        self.execute_proactive(group_id, &action).await
    }

    /// Layer 1：零 token 规则过滤器
    fn layer1_filter(
        &self,
        _group_id: i64,
        recent: &[NormalizedMessage],
        now: i64,
    ) -> Option<ProactiveIntent> {
        let last_msg = recent.last()?;
        let last_msg_ago_secs = now - last_msg.sent_at;

        // 只检查群友发的消息（忽略自己发的）
        let user_msgs: Vec<&NormalizedMessage> = recent
            .iter()
            .filter(|m| m.direction == crate::domain::qq_bot::message::MessageDirection::Inbound)
            .collect();

        if user_msgs.is_empty() {
            // 最近都是自己发的 → 没人说话，检查超时沉默
            if last_msg_ago_secs > 1800 {
                // 30 分钟无人说话
                return Some(ProactiveIntent {
                    group_id: _group_id,
                    action_type: ProactiveAction::Bubble,
                    reasoning: format!("群里已经 {} 分钟没人说话了", last_msg_ago_secs / 60),
                });
            }
            return None;
        }

        let last_user_msg = user_msgs.last()?;

        // 规则 1：上图冷场 — 最后一条是图片且 0 人回应
        let has_image = last_user_msg.segments.iter().any(|s| {
            matches!(
                s,
                crate::domain::qq_bot::message::MessageSegment::Image { .. }
            )
        });
        if has_image && user_msgs.len() < 2 {
            return Some(ProactiveIntent {
                group_id: _group_id,
                action_type: ProactiveAction::FillSilence,
                reasoning: "最后一条消息是图片，无人回应".into(),
            });
        }

        // 规则 2：提问冷场 — 最后一条是问句且 0 人回应
        let is_question = last_user_msg.normalized_text.contains('?')
            || last_user_msg.normalized_text.contains('？')
            || last_user_msg.normalized_text.contains("吗")
            || last_user_msg.normalized_text.contains("吧");
        if is_question && user_msgs.len() < 2 {
            return Some(ProactiveIntent {
                group_id: _group_id,
                action_type: ProactiveAction::FillSilence,
                reasoning: format!(
                    "有人提问「{}」但无人回应",
                    &last_user_msg
                        .normalized_text
                        .chars()
                        .take(20)
                        .collect::<String>()
                ),
            });
        }

        // 规则 3：情绪未回应 — 最后一条有负面情绪且无人回应
        let has_negative = last_user_msg.normalized_text.contains("难过")
            || last_user_msg.normalized_text.contains("烦")
            || last_user_msg.normalized_text.contains("累")
            || last_user_msg.normalized_text.contains("伤心")
            || last_user_msg.normalized_text.contains("😢")
            || last_user_msg.normalized_text.contains("😭");
        if has_negative && user_msgs.len() < 2 {
            return Some(ProactiveIntent {
                group_id: _group_id,
                action_type: ProactiveAction::ShowConcern,
                reasoning: "检测到群友有负面情绪".into(),
            });
        }

        // 规则 4：@未回 — 有人@猫猫但猫猫没回
        if last_user_msg.at_bot {
            return Some(ProactiveIntent {
                group_id: _group_id,
                action_type: ProactiveAction::FillSilence,
                reasoning: format!(
                    "有人@了我，消息是「{}」",
                    &last_user_msg
                        .normalized_text
                        .chars()
                        .take(30)
                        .collect::<String>()
                ),
            });
        }

        // 规则 5：话题中断 — 最后 3 条消息来自同一个人连续说，突然停了
        let same_user_consecutive = user_msgs
            .windows(3)
            .any(|w| w.iter().all(|m| m.qq_user_id == w[0].qq_user_id));
        if same_user_consecutive && user_msgs.len() >= 3 {
            return Some(ProactiveIntent {
                group_id: _group_id,
                action_type: ProactiveAction::FillSilence,
                reasoning: "有人连续说了几句后突然停了，可能需要接话".into(),
            });
        }

        // 规则 6：分享兴趣 — 最后一条消息包含链接、推荐或明显的分享意图
        let has_url = last_user_msg.normalized_text.contains("http")
            || last_user_msg.normalized_text.contains("www.")
            || last_user_msg.normalized_text.contains("推荐")
            || last_user_msg.normalized_text.contains("安利")
            || last_user_msg.normalized_text.contains("分享")
            || last_user_msg.raw_text.contains("CQ:share")
            || last_user_msg.raw_text.contains("CQ:music");
        let has_recommendation = last_user_msg.normalized_text.contains("好用")
            || last_user_msg.normalized_text.contains("好看")
            || last_user_msg.normalized_text.contains("好玩")
            || last_user_msg.normalized_text.contains("好吃")
            || last_user_msg.normalized_text.contains("种草");
        if (has_url || has_recommendation) && user_msgs.len() < 2 {
            return Some(ProactiveIntent {
                group_id: _group_id,
                action_type: ProactiveAction::ShareInterest,
                reasoning: "群友分享了有趣的内容，可以参与讨论".into(),
            });
        }

        // 规则 7：超时沉默
        if last_msg_ago_secs > 1800 {
            return Some(ProactiveIntent {
                group_id: _group_id,
                action_type: ProactiveAction::Bubble,
                reasoning: format!("群里已经 {} 分钟没人说话了", last_msg_ago_secs / 60),
            });
        }

        None
    }

    /// Layer 2：轻量 LLM 语义判断
    async fn layer2_judge(
        &self,
        group_id: i64,
        recent: &[NormalizedMessage],
        intent: &ProactiveIntent,
    ) -> Result<bool, QqBotError> {
        // 构建最近消息摘要
        let recent_summary: Vec<String> = recent
            .iter()
            .rev()
            .take(5)
            .map(|m| {
                let sender = match m.direction {
                    crate::domain::qq_bot::message::MessageDirection::Inbound => {
                        format!("用户{}", m.qq_user_id.unwrap_or(0))
                    }
                    crate::domain::qq_bot::message::MessageDirection::Outbound => "猫猫自己".into(),
                };
                format!("[{}] {}", sender, m.normalized_text)
            })
            .collect();

        let topic = self
            .topic_service
            .get_current_topic_label(group_id)
            .await
            .unwrap_or_else(|| "无特定话题".to_string());

        let prompt = format!(
            r#"你是群里的猫娘机器人。请判断以下场景是否需要你主动说话来活跃气氛。

规则：
- 如果群里有人说话但没人回应，冷场了，你应该说话 → yes
- 如果群里长时间没人说话，你应该冒泡 → yes  
- 如果群里本来就有多人聊天中，不需要你插嘴 → no
- 如果已经有人在回应这个话题了 → no

只需回复一个单词：yes / no

场景：{action}
理由：{reasoning}

当前话题：{topic}

最近消息：
{recent}

请输出 yes 或 no。"#,
            action = intent.action_type.label(),
            reasoning = intent.reasoning,
            topic = topic,
            recent = recent_summary.join("\n"),
        );

        let mut request = ChatCompletionRequest::new(vec![crate::domain::llm::ChatMessage {
            role: "system".into(),
            content: prompt,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }]);
        request.temperature = 0.3;
        request.max_tokens = Some(10);

        let response = self
            .llm_provider
            .chat(request)
            .await
            .map_err(|e| QqBotError::Internal(format!("Layer 2 LLM judgment failed: {e}")))?;

        let answer = response.content.trim().to_lowercase();
        Ok(answer.contains("yes"))
    }

    /// 执行主动回复 — 构建上下文 → 生成回复 → 分发
    async fn execute_proactive(
        &self,
        group_id: i64,
        _intent: &ProactiveIntent,
    ) -> Result<(), QqBotError> {
        // 检查注意力状态
        if !self.attention_store.can_process(group_id).await {
            return Ok(()); // 冷却中或正在处理其他群
        }

        // 构建一个"虚拟"消息供 ContextBuilder 使用
        let now_ts = chrono::Utc::now().timestamp();
        let dummy_msg = NormalizedMessage {
            id: None,
            bot_account_id: self.bot_account_id,
            qq_group_id: group_id,
            qq_user_id: None,
            platform_message_id: format!("proactive_{}", now_ts),
            direction: crate::domain::qq_bot::message::MessageDirection::Inbound,
            raw_text: String::new(),
            normalized_text: String::new(),
            segments: Vec::new(),
            at_bot: false,
            command_name: None,
            sent_at: now_ts,
        };

        // 尝试吸引注意力
        if !self.attention_store.try_engage(group_id).await {
            return Ok(());
        }

        // 构建上下文
        let context = self.context_builder.build_context(&dummy_msg, None).await?;

        // 生成主动回复
        let reply = self
            .reply_generator
            .generate_proactive_reply(context)
            .await?;

        // 分发
        match self
            .segment_dispatcher
            .send_direct(group_id, &reply, None)
            .await
        {
            Ok(_) => {
                info!(
                    group_id,
                    segment_count = reply.segments.len(),
                    "proactive reply dispatched"
                );

                // 更新情绪
                if let Some(ref ec) = reply.emotion_change {
                    self.emotional_service
                        .trigger_emotion(group_id, ec.mood, ec.intensity, ec.reason.clone())
                        .await;
                }
            }
            Err(e) => {
                warn!(
                    group_id,
                    error = %e,
                    "proactive reply dispatch failed, fallback to outbox"
                );
                let _ = self
                    .segment_dispatcher
                    .enqueue_reply(group_id, &reply, None)
                    .await;
            }
        }

        // 更新上次主动时间
        let mut last_map = self.last_proactive.write().await;
        last_map.insert(group_id, chrono::Utc::now().timestamp());

        // 启动冷却
        self.attention_store.start_cooldown().await;

        Ok(())
    }
}
