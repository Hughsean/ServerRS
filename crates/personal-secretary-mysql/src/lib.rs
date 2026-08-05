//! Personal Secretary 的 MySQL 出站适配器。
//!
//! 本 crate 依赖 `personal-secretary` 中定义的应用端口并提供 SeaORM/MySQL 实现；
//! 领域与应用 crate 不得反向依赖本 crate。

mod repo;

use std::sync::Arc;

use personal_secretary::*;
use sea_orm::DatabaseConnection;

struct MySqlActionCheckpointStoreFactory {
    db: DatabaseConnection,
}

impl ActionCheckpointStoreFactoryT for MySqlActionCheckpointStoreFactory {
    fn for_run(
        &self,
        action_run_id: &ActionRunId,
    ) -> Arc<dyn agent_core::graph::CheckpointStore<SecretaryAgentState>> {
        Arc::new(repo::BoundActionCheckpointStore::new(
            self.db.clone(),
            action_run_id.clone(),
        ))
    }
}

/// 构造 Action Planner 使用的持久化 CheckpointStore 工厂。
pub fn build_mysql_action_checkpoint_store_factory(
    db: DatabaseConnection,
) -> Arc<dyn ActionCheckpointStoreFactoryT> {
    Arc::new(MySqlActionCheckpointStoreFactory { db })
}

/// 构造同时实现 [`InboundEventStoreT`]、[`crate::IngestionContinuityStoreT`] 和
/// [`crate::BackfillStateStoreT`] 的 MySQL 仓储，供 `qqbot-server` 装配实时入库与历史回补。
pub fn build_mysql_inbound_event_store(db: DatabaseConnection) -> Arc<dyn PersonalSecretaryStoreT> {
    Arc::new(repo::MySqlInboundEventStore::new(db))
}

/// 构造普通消息 Spool 启动恢复仓储；租约用于 fencing 遗留 epoch 的最终收口。
pub fn build_mysql_realtime_spool_recovery_store(
    db: DatabaseConnection,
    lease_secs: u64,
) -> Arc<dyn RealtimeSpoolRecoveryStoreT> {
    Arc::new(repo::MySqlInboundEventStore::new_for_realtime_spool_recovery(db, lease_secs))
}

/// 构造延迟 Reply 修复仓储（unresolved 候选 + 租约/退避簿），供后台修复 Worker 装配。
/// 与实时入库仓储共享同一 schema 与同一结构体，端口按需裁剪。
pub fn build_mysql_reply_reconcile_store(db: DatabaseConnection) -> Arc<dyn ReplyReconcileStoreT> {
    Arc::new(repo::MySqlInboundEventStore::new(db))
}

/// 构造支持历史回补状态仓储的组合实现：实时入库 + 连续性 + 回补状态。
/// `lease_secs` 来自 `[backfill]` 配置，用于回补运行续租。返回
/// `Arc<dyn BackfillStateStoreWithIngestionT>` 以满足用例对统一幂等入口与回补状态的双重需求。
pub fn build_mysql_backfill_store(
    db: DatabaseConnection,
    lease_secs: u64,
) -> Arc<dyn crate::BackfillStateStoreWithIngestionT> {
    Arc::new(repo::MySqlInboundEventStore::new_for_backfill(
        db, lease_secs,
    ))
}

/// 构造独立的确定性线程投影仓储。它只消费 `secretary_source_events`，并使用自己的
/// 租约表，不占用通用 `processing_status`，因此可与后续摘要、提醒消费者并行演进。
pub fn build_mysql_thread_projection_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::ThreadProjectionStoreT> {
    Arc::new(repo::MySqlThreadProjectionStore::new(db))
}

/// 构造线程类型化语义状态仓储。语义游标与确定性线程投影游标相互独立。
pub fn build_mysql_thread_semantic_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::ThreadSemanticStoreT> {
    Arc::new(repo::MySqlThreadSemanticStore::new(db))
}

/// 构造跨会话线程关联候选仓储。仅写入不可逆提示指纹和 proposed 候选，绝不改写线程成员。
pub fn build_mysql_thread_link_store(db: DatabaseConnection) -> Arc<dyn crate::ThreadLinkStoreT> {
    Arc::new(repo::MySqlThreadLinkStore::new(db))
}

/// 构造 Owner 审批后的线程逻辑变更仓储。原始成员投影保持不变，执行结果通过 Effect ID 幂等。
pub fn build_mysql_thread_mutation_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::ThreadMutationStoreT> {
    Arc::new(repo::MySqlThreadMutationStore::new(db))
}

/// 构造 Owner 线程语义/生命周期控制仓储；业务变更、审计与 Action Receipt 共用事务。
pub fn build_mysql_thread_control_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::ThreadControlStoreT> {
    Arc::new(repo::MySqlThreadControlStore::new(db))
}

/// 构造线程变更专用的持久化 Graph Checkpoint 仓储；与数字人 Checkpoint 完全隔离。
pub fn build_mysql_thread_mutation_checkpoint_store(
    db: DatabaseConnection,
) -> Arc<dyn agent_core::graph::CheckpointStore<crate::ThreadMutationAgentState>> {
    Arc::new(repo::MySqlThreadMutationStore::new(db))
}

/// 构造来源化结构记忆仓储；人物、项目和承诺只保存有界类型化状态与 SourceEvent 引用。
pub fn build_mysql_memory_store(db: DatabaseConnection) -> Arc<dyn crate::MemoryStoreT> {
    Arc::new(repo::MySqlMemoryStore::new(db))
}

/// 构造承诺跟进调度仓储；仅生成持久化待发送项，不直接调用任何消息平台。
pub fn build_mysql_follow_up_store(db: DatabaseConnection) -> Arc<dyn crate::FollowUpStoreT> {
    Arc::new(repo::MySqlFollowUpStore::new(db))
}

/// 构造 Owner FollowUp 控制仓储；业务变更、不可变审计与 Action Receipt 共用事务。
pub fn build_mysql_follow_up_control_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::FollowUpControlStoreT> {
    Arc::new(repo::MySqlFollowUpControlStore::new(db))
}

/// 构造 Owner ResponseExpectation 控制仓储；授权/Receipt 逻辑与 FollowUp 控制共享。
pub fn build_mysql_response_expectation_control_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::ResponseExpectationControlStoreT> {
    Arc::new(repo::MySqlResponseExpectationControlStore::new(db))
}

/// 构造结构化记忆候选提取仓储（批次/游标/租约/失效/列表）。
pub fn build_mysql_memory_candidate_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::MemoryCandidateStoreT> {
    Arc::new(repo::MySqlMemoryCandidateStore::new(db))
}

/// 构造 Owner 记忆候选控制仓储；授权/Receipt 逻辑与 FollowUp 控制共享。
pub fn build_mysql_memory_candidate_control_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::MemoryCandidateControlStoreT> {
    Arc::new(repo::MySqlMemoryCandidateControlStore::new(db))
}

/// 构造本地 Owner 身份绑定仓储；绑定由本地配置建立，不从聊天正文推断。
pub fn build_mysql_owner_binding_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::OwnerBindingStoreT> {
    Arc::new(repo::MySqlOwnerBindingStore::new(db))
}

/// 构造账号会话目录快照仓储。快照绑定 account_id，幂等，跨重启恢复。
pub fn build_mysql_directory_store(db: DatabaseConnection) -> Arc<dyn crate::DirectoryStoreT> {
    Arc::new(repo::MySqlDirectoryStore::new(db))
}

/// 构造消息撤回仓储。不物理删除审计历史；关联键禁止单 message_id 跨账号。
pub fn build_mysql_recall_store(db: DatabaseConnection) -> Arc<dyn crate::RecallStoreT> {
    Arc::new(repo::MySqlRecallStore::new(db))
}

/// 构造富消息 Artifact 引用仓储。不自动下载；有界；TTL；撤回失效传播。
pub fn build_mysql_artifact_store(db: DatabaseConnection) -> Arc<dyn crate::ArtifactStoreT> {
    Arc::new(repo::MySqlArtifactStore::new(db))
}

/// 构造通知策略仓储。策略 Family/Revision、账号 epoch 与反馈写入均通过短事务持久化。
#[allow(dead_code)]
pub fn build_mysql_notification_policy_store(
    db: DatabaseConnection,
) -> Arc<dyn crate::NotificationPolicyStoreT> {
    Arc::new(repo::MySqlNotificationPolicyStore::new(db))
}

/// 构造 Owner Retriever 仓储。查询严格限定在账号作用域内，跨账号查询被 SQL 拒绝。
pub fn build_mysql_retriever_store(db: DatabaseConnection) -> Arc<dyn crate::RetrieverStoreT> {
    Arc::new(repo::MySqlRetrieverStore::new(db))
}

/// 构造 Owner Agenda 仓储；mutation、审计与 Action Receipt 共用事务。
pub fn build_mysql_agenda_store(db: DatabaseConnection) -> Arc<dyn crate::AgendaStoreT> {
    Arc::new(repo::MySqlAgendaStore::new(db))
}

/// 构造 Action Planner 运行仓储。CAS 领取 + lease fencing + 幂等 Effect。
pub fn build_mysql_action_store(db: DatabaseConnection) -> Arc<dyn crate::ActionStoreT> {
    Arc::new(repo::MySqlActionStore::new(db))
}

/// 构造绑定业务 ActionRunId 的持久化 Graph Checkpoint 仓储。
/// P0 修复：checkpoint.run_id() 是 Graph 内部 RunId，不是业务 run_id；
/// 此工厂接收业务 ActionRunId，确保 FK 引用 secretary_action_runs.run_id。
pub fn build_bound_action_checkpoint_store(
    db: DatabaseConnection,
    action_run_id: crate::ActionRunId,
) -> Arc<dyn agent_core::graph::CheckpointStore<crate::SecretaryAgentState>> {
    Arc::new(repo::BoundActionCheckpointStore::new(db, action_run_id))
}
