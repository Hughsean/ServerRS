mod repo;

use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::PersonalSecretaryStoreT;

/// 构造同时实现 [`InboundEventStoreT`]、[`crate::IngestionContinuityStoreT`] 和
/// [`crate::BackfillStateStoreT`] 的 MySQL 仓储，供 `qqbot-server` 装配实时入库与历史回补。
pub fn build_mysql_inbound_event_store(db: DatabaseConnection) -> Arc<dyn PersonalSecretaryStoreT> {
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

/// 构造线程变更专用的持久化 Graph Checkpoint 仓储；与数字人 Checkpoint 完全隔离。
pub fn build_mysql_thread_mutation_checkpoint_store(
    db: DatabaseConnection,
) -> Arc<dyn agent_core::graph::CheckpointStore<crate::ThreadMutationAgentState>> {
    Arc::new(repo::MySqlThreadMutationStore::new(db))
}
