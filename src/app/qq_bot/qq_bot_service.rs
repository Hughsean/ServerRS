use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::domain::qq_bot::attention::{BotAccount, TriggerDecision};
use crate::domain::qq_bot::message::{NormalizedMessage, ProcessStatus};
use crate::domain::qq_bot::persona::BotPersona;
use crate::domain::qq_bot::repository::{
    AgentTurnRepository, BotAccountRepository, GroupMessageRepository, GroupRepository,
};
use crate::domain::qq_bot::turn::{AgentTurn, TriggerType, TurnStatus};
use crate::domain::qq_bot::QqBotError;
use crate::infra::qq_bot::attention_store::InMemoryAttentionStore;
use crate::infra::qq_bot::napcat::api::NapCatApiClient;
use crate::infra::qq_bot::napcat::listener::GroupMessageHandler;

use super::context_builder::ContextBuilder;
use super::emotional_state_service::EmotionalStateService;
use super::message_ingestion::MessageIngestionService;
use super::outbox_worker::OutboxWorker;
use super::profile_builder::ProfileBuilder;
use super::relationship_service::RelationshipService;
use super::reply_generator::ReplyGenerator;
use super::segment_dispatcher::SegmentDispatcher;
use super::topic_service::TopicService;
use super::trigger_evaluator::TriggerEvaluator;

/// Main orchestrator for the QQ Bot (赛博猫猫).
///
/// Coordinates the full message lifecycle:
/// 1. Ingest raw message (dedup, normalize, persist)
/// 2. Evaluate trigger (Layer 1 rules → Layer 2 LLM)
/// 3. Build conversation context (time, emotion, topic, relationship)
/// 4. Generate reply
/// 5. Dispatch reply segments
/// 6. Record agent turn
pub struct QqBotService {
    // Domain services
    ingestion: Arc<MessageIngestionService>,
    trigger: Arc<TriggerEvaluator>,
    context_builder: Arc<ContextBuilder>,
    reply_generator: Arc<ReplyGenerator>,
    segment_dispatcher: Arc<SegmentDispatcher>,

    // Profile builder
    profile_builder: Option<Arc<ProfileBuilder>>,

    // Repositories
    bot_account_repo: Arc<dyn BotAccountRepository>,
    group_repo: Arc<dyn GroupRepository>,
    turn_repo: Arc<dyn AgentTurnRepository>,
    #[allow(dead_code)]
    message_repo: Arc<dyn GroupMessageRepository>,

    // Attention
    attention_store: Arc<InMemoryAttentionStore>,

    // Emotional state
    emotional_service: Arc<EmotionalStateService>,

    // Topic tracking
    topic_service: Arc<TopicService>,

    // Relationship tracking
    relationship_service: Option<Arc<RelationshipService>>,

    // Bot identity
    bot_account: Arc<RwLock<Option<BotAccount>>>,
    persona: BotPersona,
}

impl QqBotService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ingestion: Arc<MessageIngestionService>,
        trigger: Arc<TriggerEvaluator>,
        context_builder: Arc<ContextBuilder>,
        reply_generator: Arc<ReplyGenerator>,
        segment_dispatcher: Arc<SegmentDispatcher>,
        profile_builder: Option<Arc<ProfileBuilder>>,
        bot_account_repo: Arc<dyn BotAccountRepository>,
        group_repo: Arc<dyn GroupRepository>,
        turn_repo: Arc<dyn AgentTurnRepository>,
        message_repo: Arc<dyn GroupMessageRepository>,
        attention_store: Arc<InMemoryAttentionStore>,
        persona: BotPersona,
        emotional_service: Arc<EmotionalStateService>,
        topic_service: Arc<TopicService>,
        relationship_service: Option<Arc<RelationshipService>>,
    ) -> Self {
        Self {
            ingestion,
            trigger,
            context_builder,
            reply_generator,
            segment_dispatcher,
            profile_builder,
            bot_account_repo,
            group_repo,
            turn_repo,
            message_repo,
            attention_store,
            emotional_service,
            topic_service,
            relationship_service,
            bot_account: Arc::new(RwLock::new(None)),
            persona,
        }
    }

    /// Initialise the bot account cache.
    pub async fn init(&self, self_qq_id: i64) -> Result<(), QqBotError> {
        let account = self.bot_account_repo
            .find_by_self_qq_id(self_qq_id)
            .await
            .map_err(|e| QqBotError::Internal(format!("failed to load bot account: {e}")))?;

        if let Some(account) = account {
            *self.bot_account.write().await = Some(account);
            info!(self_qq_id, "QQ 机器人账号已加载");
            Ok(())
        } else {
            warn!(self_qq_id, "数据库中未找到 QQ 机器人账号，使用临时配置");
            Ok(())
        }
    }

    /// Handle an incoming group message — full lifecycle.
    ///
    /// This is the main entry point called by the WebSocket listener.
    pub async fn handle_group_message(&self, msg: NormalizedMessage) {
        let group_id = msg.qq_group_id;

        // 1. Ingest the message
        let persisted = match self.ingestion.ingest(
            msg.bot_account_id,
            msg.qq_group_id,
            msg.qq_user_id.unwrap_or(0),
            &msg.platform_message_id,
            &msg.raw_text,
            msg.sent_at,
        )
        .await
        {
            Ok(m) => m,
            Err(e) => {
                error!(group_id, error = %e, "消息接收失败");
                return;
            }
        };

        // 1b. Profile building: auto-register user + observe message
        if let Some(ref pb) = self.profile_builder {
            if let Some(qq_user_id) = persisted.qq_user_id {
                // Auto-register if first time
                if let Err(e) = pb.ensure_user_registered(
                    qq_user_id,
                    None, // nickname will be filled from group member info later
                    persisted.sent_at,
                ).await {
                    error!(qq_user_id, error = %e, "QQ 用户自动注册失败");
                }
                // Lightweight observation
                pb.observe_message(&persisted).await;
            }
        }

        // 2. Load bot account + group config
        let bot_account = self.bot_account.read().await.clone().unwrap_or(BotAccount {
            bot_account_id: 0,
            platform: "qq".into(),
            self_qq_id: 0,
            display_name: Some(self.persona.nickname.clone()),
            adapter: "napcat".into(),
            connection_mode: "websocket".into(),
            enabled: true,
        });

        let group_config = self.group_repo.find_by_group_id(group_id).await.ok()
            .and_then(|g| g);

        // 从用户消息中反推情绪变化
        if let Some((mood, intensity, event)) = EmotionalStateService::detect_mood_from_text(&persisted.normalized_text) {
            self.emotional_service.trigger_emotion(
                group_id,
                mood,
                intensity,
                Some(format!("群友 {}: {}", persisted.qq_user_id.unwrap_or(0), event)),
            ).await;
        }

        // 关系更新：每次收到消息更新互动计数
        if let Some(ref rs) = self.relationship_service {
            if let Some(qq_user_id) = persisted.qq_user_id {
                if let Err(e) = rs.update_interaction(group_id, qq_user_id, Some(persisted.sent_at)).await {
                    error!(group_id, user_id = qq_user_id, error = %e, "failed to update relationship");
                }
            }
        }

        // 话题分析：每次收到消息更新话题状态（在 trigger 之前，供 trigger 使用）
        let recent_for_topic = self.message_repo.recent_by_group(group_id, 20).await
            .unwrap_or_default();
        self.topic_service.analyze(group_id, &recent_for_topic, persisted.sent_at).await;

        // 3. Evaluate trigger
        let decision = match self.trigger.evaluate(&persisted, &bot_account, group_config.as_ref()).await {
            Ok(d) => d,
            Err(e) => {
                error!(group_id, error = %e, "trigger evaluation failed");
                let _ = self.ingestion.mark_processed(
                    persisted.id.unwrap_or(0),
                    ProcessStatus::Failed,
                    Some(&e.to_string()),
                ).await;
                return;
            }
        };

        match decision {
            TriggerDecision::Skip | TriggerDecision::Wait => {
                let _ = self.ingestion.mark_processed(
                    persisted.id.unwrap_or(0),
                    ProcessStatus::Ignored,
                    None,
                ).await;
                return;
            }
            TriggerDecision::Respond => {
                // Proceed
                let _ = self.ingestion.mark_processed(
                    persisted.id.unwrap_or(0),
                    ProcessStatus::Processed,
                    None,
                ).await;
            }
        }

        // Confirm attention engagement
        self.attention_store.confirm_engagement(group_id).await;

        // 4. Build context
        let context = match self.context_builder.build_context(&persisted, group_config.as_ref()).await {
            Ok(c) => c,
            Err(e) => {
                error!(group_id, error = %e, "context building failed");
                return;
            }
        };

        // 5. Generate reply
        let reply = match self.reply_generator.generate_reply(&persisted, context, group_config.as_ref()).await {
            Ok(r) => r,
            Err(e) => {
                error!(group_id, error = %e, "reply generation failed");
                return;
            }
        };

        // 6. Create agent turn record
        let turn = AgentTurn {
            turn_id: None,
            bot_account_id: bot_account.bot_account_id,
            qq_group_id: group_id,
            trigger_message_id: persisted.id.unwrap_or(0),
            response_message_id: None,
            trigger_type: if persisted.at_bot { TriggerType::Mention } else { TriggerType::Keyword },
            qq_user_id: persisted.qq_user_id,
            internal_user_id: None,
            prompt_version: None,
            model_name: None,
            reasoning_enabled: None,
            input_token_count: None,
            output_token_count: None,
            latency_ms: None,
            status: TurnStatus::Created,
            error_message: None,
            trace_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let turn = match self.turn_repo.insert(&turn).await {
            Ok(t) => t,
            Err(e) => {
                error!(group_id, error = %e, "failed to record agent turn");
                return;
            }
        };

        // 7. Dispatch reply segments
        let related_turn_id = turn.turn_id;
        match self.segment_dispatcher.send_direct(group_id, &reply, related_turn_id).await {
            Ok(_platform_ids) => {
                info!(
                    group_id,
                    turn_id = ?related_turn_id,
                    "reply dispatched successfully"
                );

                // Update turn status
                let _ = self.turn_repo.update_response(
                    related_turn_id.unwrap_or(0),
                    0, // response_message_id (not available from NapCat API easily)
                    TurnStatus::Responded,
                ).await;

                // 从回复的 emotion_change 字段更新情绪状态
                if let Some(ref ec) = reply.emotion_change {
                    self.emotional_service.trigger_emotion(
                        group_id,
                        ec.mood,
                        ec.intensity,
                        ec.reason.clone(),
                    ).await;
                }

                // 从回复的 relationship_hints 字段自动更新关系
                if let Some(ref hints) = reply.relationship_hints {
                    if let Some(ref rs) = self.relationship_service {
                        if let Some(ref nickname) = hints.nickname_preference {
                            let _ = rs.update_nickname_preference(
                                group_id,
                                hints.target_user_id,
                                nickname.clone(),
                            ).await;
                        }
                        if !hints.known_interests.is_empty() || !hints.known_avoid_topics.is_empty() {
                            // 获取现有关系，合并兴趣和回避话题
                            if let Ok(Some(mut rel)) = rs.get_relationship(group_id, hints.target_user_id).await {
                                for interest in &hints.known_interests {
                                    if !rel.known_interests.contains(interest) {
                                        rel.known_interests.push(interest.clone());
                                    }
                                }
                                for topic in &hints.known_avoid_topics {
                                    if !rel.known_avoid_topics.contains(topic) {
                                        rel.known_avoid_topics.push(topic.clone());
                                    }
                                }
                                // 持久化更新（通过 upsert）
                                let _ = rs.update_known_info(
                                    group_id,
                                    hints.target_user_id,
                                    &hints.known_interests,
                                    &hints.known_avoid_topics,
                                ).await;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!(group_id, error = %e, "reply dispatch failed, fallback to outbox");

                // Fallback: enqueue remaining segments to outbox
                let _ = self.segment_dispatcher.enqueue_reply(group_id, &reply, related_turn_id).await;

                let _ = self.turn_repo.update_status(
                    related_turn_id.unwrap_or(0),
                    TurnStatus::Failed,
                    Some(&e.to_string()),
                ).await;
            }
        }

        // Start cooldown after responding
        self.attention_store.start_cooldown().await;
    }

    /// Periodic tick for attention store idle timeout.
    pub async fn tick(&self) {
        self.attention_store.tick_idle().await;
    }

    /// Create the outbox worker for this service.
    pub fn create_outbox_worker(
        self: &Arc<Self>,
        napcat_api: Option<Arc<NapCatApiClient>>,
        poll_interval_secs: u64,
        batch_size: u32,
    ) -> Arc<OutboxWorker> {
        Arc::new(OutboxWorker::new(
            Arc::clone(&self.segment_dispatcher.outbox_repo()),
            napcat_api,
            poll_interval_secs,
            batch_size,
        ))
    }
}

// Internal helper — used by create_outbox_worker above

/// Bridge: implement GroupMessageHandler so QqBotService can be used as a NapCatListener handler.
#[async_trait]
impl GroupMessageHandler for QqBotService {
    async fn handle_group_message(&self, msg: NormalizedMessage, _raw_json: Value) {
        self.handle_group_message(msg).await;
    }
}