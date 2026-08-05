//! 线程投影、线程语义、跨会话关联与历史回补 Worker 装配。
//!
//! 这些装配步骤可能因配置或 LLM 客户端构造失败而返回错误；调用方在错误路径
//! 必须回收此前已启动的 Worker（见 `runtime` 编排中的 `shutdown_all`）。

use std::sync::Arc;

use personal_secretary::{
    BackfillGapUseCase, ConservativeMemoryCandidateExtractor, ConservativeThreadSemanticExtractor,
    DeterministicThreadPlanner, DeterministicThreadPolicy, DirectorySyncUseCase,
    MemoryCandidateExtractorT, MemoryCandidateUseCase, ReconcilePendingRepliesUseCase,
    SourceAccountRef, ThreadLinkUseCase, ThreadProjectionUseCase, ThreadSemanticExtractorT,
    ThreadSemanticUseCase,
};
use personal_secretary_mysql::{
    build_mysql_backfill_store, build_mysql_memory_candidate_store,
    build_mysql_reply_reconcile_store, build_mysql_thread_link_store,
    build_mysql_thread_projection_store, build_mysql_thread_semantic_store,
};
use qqbot::napcat::{NapCatDirectoryReadT, NapCatHistoryReadT, NapCatReadOnlyClient};
use sea_orm::DatabaseConnection;

use crate::backfill::spawn_backfill_worker;
use crate::bootstrap::workers::WorkerHandles;
use crate::config::AppConfig;
use crate::llm::{LlmMemoryCandidateExtractor, LlmThreadSemanticExtractor, OpenAiCompatibleClient};
use crate::memory_candidates::spawn_memory_candidates_worker;
use crate::reply_reconcile_worker::spawn_reply_reconcile_worker;
use crate::runtime::RuntimeError;
use crate::thread_links::spawn_thread_links_worker;
use crate::thread_projection::spawn_thread_projection_worker;
use crate::thread_semantics::spawn_thread_semantics_worker;

/// 装配线程相关 Worker（投影、语义、关联、回补）。
pub(crate) async fn assemble_thread_workers(
    handles: &mut WorkerHandles,
    db: DatabaseConnection,
    config: &AppConfig,
    account: SourceAccountRef,
) -> Result<(), RuntimeError> {
    if config.thread_projection.enabled {
        let policy =
            DeterministicThreadPolicy::new(config.thread_projection.same_conversation_window_secs)
                .map_err(|error| RuntimeError::Config(error.to_string()))?;
        let use_case = Arc::new(
            ThreadProjectionUseCase::new(
                build_mysql_thread_projection_store(db.clone()),
                DeterministicThreadPlanner::new(policy),
                config.thread_projection.batch_size,
                config.thread_projection.lease_secs,
                config.thread_projection.same_conversation_window_secs,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        handles.thread_projection = Some(spawn_thread_projection_worker(
            use_case,
            config.thread_projection.clone(),
        ));
    } else {
        tracing::info!("确定性线程投影已禁用（thread_projection.enabled=false）");
    }

    if config.thread_semantics.enabled {
        let extractor: Arc<dyn ThreadSemanticExtractorT> = if config.llm.enabled {
            let client = Arc::new(
                OpenAiCompatibleClient::new(&config.llm)
                    .map_err(|error| RuntimeError::Llm(error.to_string()))?,
            );
            tracing::info!(
                model = config.llm.model,
                endpoint_host = client.endpoint_host(),
                max_input_chars = config.llm.max_input_chars,
                max_output_tokens = config.llm.max_output_tokens,
                max_candidates_per_kind = config.llm.max_candidates_per_kind,
                "LLM 有界线程语义提取已启用；模型输出仍须通过来源与领域策略校验"
            );
            Arc::new(
                LlmThreadSemanticExtractor::from_openai(client, config.llm.max_candidates_per_kind)
                    .map_err(|error| RuntimeError::Llm(error.to_string()))?,
            )
        } else {
            tracing::info!("LLM 已禁用；线程语义使用保守零模型提取器");
            Arc::new(
                ConservativeThreadSemanticExtractor::new(config.thread_semantics.max_event_chars)
                    .map_err(|error| RuntimeError::Config(error.to_string()))?,
            )
        };
        let use_case = Arc::new(
            ThreadSemanticUseCase::new(
                build_mysql_thread_semantic_store(db.clone()),
                extractor,
                config.thread_semantics.max_events,
                config.thread_semantics.max_total_chars,
                config.thread_semantics.lease_secs,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        handles.thread_semantics = Some(spawn_thread_semantics_worker(
            use_case,
            config.thread_semantics.clone(),
        ));
    } else {
        tracing::info!("线程类型化语义已禁用（thread_semantics.enabled=false）");
    }

    if config.thread_links.enabled {
        let use_case = Arc::new(
            ThreadLinkUseCase::new(
                build_mysql_thread_link_store(db.clone()),
                config.thread_links.max_events,
                config.thread_links.max_total_chars,
                config.thread_links.lease_secs,
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        handles.thread_links = Some(spawn_thread_links_worker(
            use_case,
            config.thread_links.clone(),
        ));
    } else {
        tracing::info!("跨会话线程关联候选已禁用（thread_links.enabled=false）");
    }

    // 结构化记忆候选提取：独立持久游标 Worker（normal 总是进入；local_only 仅当
    // 本地端点已验证回环；envelope_only/never_long_term 与已撤回事件不进入模型）。
    if config.memory_candidates.enabled {
        let extractor: Arc<dyn MemoryCandidateExtractorT> = if config.llm.enabled {
            let client = Arc::new(
                OpenAiCompatibleClient::new(&config.llm)
                    .map_err(|error| RuntimeError::Llm(error.to_string()))?,
            );
            tracing::info!(
                model = config.llm.model,
                endpoint_host = client.endpoint_host(),
                extractor_version = config.memory_candidates.extractor_version,
                "LLM 记忆候选提取已启用；模型输出仍须通过来源与领域策略校验"
            );
            Arc::new(
                LlmMemoryCandidateExtractor::from_openai(
                    client,
                    config.memory_candidates.max_event_chars as usize,
                    config.memory_candidates.extractor_version.clone(),
                )
                .map_err(|error| RuntimeError::Config(error.to_string()))?,
            )
        } else {
            tracing::info!("LLM 已禁用；记忆候选使用保守零模型提取器");
            Arc::new(
                ConservativeMemoryCandidateExtractor::new(
                    config.memory_candidates.max_event_chars as usize,
                    config.memory_candidates.extractor_version.clone(),
                )
                .map_err(|error| RuntimeError::Config(error.to_string()))?,
            )
        };
        let use_case = Arc::new(
            MemoryCandidateUseCase::new(
                build_mysql_memory_candidate_store(db.clone()),
                extractor,
                account.clone(),
                config.memory_candidates.max_events_per_batch,
                config.memory_candidates.max_event_chars,
                config.memory_candidates.max_total_input_chars,
                config.memory_candidates.lease_secs,
                config.llm_endpoint_verified_loopback(),
            )
            .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        handles.memory_candidates = Some(spawn_memory_candidates_worker(
            use_case,
            config.memory_candidates.clone(),
        ));
    } else {
        tracing::info!("结构化记忆候选提取已禁用（memory_candidates.enabled=false）");
    }

    // 装配历史回补：只读 NapCat 客户端 + 回补状态仓储 + 协议无关用例 + 独立 Worker。
    // 分页算法、Gap 完整性判定和 SQL 不在 runtime 内，而分别在领域层与 MySQL 仓储。
    if config.backfill.enabled {
        let backfill_store = build_mysql_backfill_store(db.clone(), config.backfill.lease_secs);
        let napcat_readonly: Arc<dyn NapCatHistoryReadT> = Arc::new(NapCatReadOnlyClient::new(
            config.napcat.http_base_url.clone(),
        ));
        let history_source = Arc::new(crate::napcat_history_source::NapCatHistorySource::new(
            napcat_readonly,
            account.clone(),
            config.napcat.self_qq_id,
        ));
        let budget = config
            .backfill
            .budget()
            .map_err(|error| RuntimeError::Config(error.to_string()))?;
        let use_case = Arc::new(BackfillGapUseCase::new(
            backfill_store,
            history_source,
            budget,
        ));
        let handle = spawn_backfill_worker(use_case, config.backfill.clone());
        tracing::info!(
            page_size = config.backfill.page_size,
            max_pages_per_scope = config.backfill.max_pages_per_scope,
            max_events_per_run = config.backfill.max_events_per_run,
            max_concurrency = config.backfill.max_concurrency,
            lease_secs = config.backfill.lease_secs,
            "历史回补 Worker 已装配，与实时 WebSocket 接收解耦"
        );
        handles.backfill = Some(handle);
    } else {
        tracing::info!("历史回补已禁用（backfill.enabled=false）");
    }

    // 装配延迟 Reply 修复：若父事件永不重放，后台按退避周期领取 unresolved
    // 候选并重试解析（Codex 复核 P1-1），保证待解析关系不会永久滞留。
    if config.reply_reconcile.enabled {
        let reconcile_store = build_mysql_reply_reconcile_store(db.clone());
        let use_case = Arc::new(ReconcilePendingRepliesUseCase::new(
            reconcile_store,
            config.reply_reconcile.budget(),
        ));
        let handle = spawn_reply_reconcile_worker(use_case, config.reply_reconcile.clone());
        tracing::info!(
            batch_size = config.reply_reconcile.batch_size,
            lease_secs = config.reply_reconcile.lease_secs,
            retry_initial_ms = config.reply_reconcile.retry_initial_ms,
            retry_max_ms = config.reply_reconcile.retry_max_ms,
            "延迟 Reply 修复 Worker 已装配，与实时消息接收解耦"
        );
        handles.reply_reconcile = Some(handle);
    } else {
        tracing::info!("延迟 Reply 修复已禁用（reply_reconcile.enabled=false）");
    }

    // B4 账号会话目录同步：只读 NapCat 列表 API -> 目录快照 -> MySQL。
    // 不在每次 WebSocket 重连时无条件下载完整目录（TTL 内跳过）。
    // 1 MiB 上限拒绝时保持 uncertain，不提高上限、不转空数组。
    if config.directory_sync.enabled {
        let directory_store = personal_secretary_mysql::build_mysql_directory_store(db.clone());
        let napcat_readonly: Arc<dyn NapCatDirectoryReadT> = Arc::new(NapCatReadOnlyClient::new(
            config.napcat.http_base_url.clone(),
        ));
        let directory_source = Arc::new(crate::napcat_directory::NapCatDirectorySource::new(
            napcat_readonly,
        ));
        let budget = config.directory_sync.budget();
        let use_case = Arc::new(
            DirectorySyncUseCase::new(directory_source, directory_store, budget)
                .map_err(|error| RuntimeError::Config(error.to_string()))?,
        );
        let handle = crate::directory_sync::spawn_directory_sync_worker(
            use_case,
            account.clone(),
            config.directory_sync.clone(),
        );
        tracing::info!(
            snapshot_ttl_secs = config.directory_sync.snapshot_ttl_secs,
            sync_deadline_secs = config.directory_sync.sync_deadline_secs,
            max_entries = config.directory_sync.max_entries,
            "B4 账号会话目录同步 Worker 已装配（TTL 内跳过完整下载）"
        );
        handles.directory_sync = Some(handle);
    } else {
        tracing::info!("B4 账号会话目录同步已禁用（directory_sync.enabled=false）");
    }

    Ok(())
}
