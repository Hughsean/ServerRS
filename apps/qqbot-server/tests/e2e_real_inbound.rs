//! 真实消息入站闭环端到端验收。
//!
//! 本测试默认 `#[ignore]`，必须显式设置 `QQBOT_E2E_ENABLED=1` 才会运行。
//! 它启动真实的 `qqbot-server`（通过 `run_with_cancellation`），等待用户在指定
//! 测试群手工发送带唯一 marker 的消息，然后验证完整派生链：
//!
//! ```text
//! NapCat WebSocket 真实消息
//! -> 有界接入队列
//! -> MySQL SourceEvent 幂等持久化
//! -> Actor/Conversation/Reply/At 结构解析
//! -> EventThread 确定性投影
//! -> Ollama 有界线程语义提取
//! -> proposed Claim/Decision/OpenQuestion
//! -> 精确 source_event_id 来源证据
//! ```
//!
//! 随后执行重启验收：第二次启动后验证线程成员、候选和来源没有重复。
//!
//! 约束：
//! - 不主动发送 QQ 消息，只等待用户手工发送；
//! - 不清空或批量删除数据库；
//! - 不打印消息正文，只输出 source_event_id、thread_id、候选类型和来源数量；
//! - 失败时输出缺失的具体链路阶段，而非笼统超时；
//! - 所有断言按 target_event_id / thread_id 限定，不断言全库。

use std::time::{Duration, Instant};

use qqbot_server::{config::AppConfig, runtime};
use sea_orm::{Database, DatabaseBackend, FromQueryResult, Statement};
use tokio::sync::watch;

/// 等待单阶段数据库条件的轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 等待用户手工发送消息的窗口。
const WAIT_FOR_MESSAGE: Duration = Duration::from_secs(120);
/// 等待 Worker 派生（投影/语义）的窗口。语义 Worker 按线程顺序处理，若数据库中有
/// 大量历史遗留线程，目标线程可能需要等待较长时间才轮到。
const WAIT_FOR_DERIVATION: Duration = Duration::from_secs(180);
/// 优雅关闭的窗口（run_with_cancellation 内部有 25s 全局 deadline）。
const WAIT_FOR_SHUTDOWN: Duration = Duration::from_secs(30);
/// 唯一允许的测试群。
const REQUIRED_TEST_GROUP: i64 = 671260344;

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for the E2E test"))
}

fn optional_env_bool(name: &str) -> bool {
    std::env::var(name)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 初始化 tracing，使运行时的连接/Worker 日志在测试输出中可见。
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_test_writer()
        .try_init();
}

// ===== 查询结果类型 =====

#[derive(Debug, FromQueryResult)]
struct StringValue {
    value: String,
}

#[derive(Debug, FromQueryResult)]
struct CountValue {
    value: i64,
}

/// 用于从 `BIGINT UNSIGNED` 列读取内部 ID（如 `secretary_accounts.id`）。
/// `i64` 与 `BIGINT UNSIGNED` 不兼容（sqlx 报 mismatched types），必须用 `u64`。
/// `String` 也不可靠。因此使用 `u64`。
#[derive(Debug, FromQueryResult)]
struct IdValue {
    value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, FromQueryResult)]
struct CursorValue {
    last_added_at: Option<chrono::NaiveDateTime>,
    last_source_event_id: Option<String>,
}

#[derive(Debug, FromQueryResult)]
#[allow(dead_code)]
struct EpochInfo {
    connection_epoch_id: String,
    status: String,
    started_at: chrono::NaiveDateTime,
    connected_at: Option<chrono::NaiveDateTime>,
    ended_at: Option<chrono::NaiveDateTime>,
    end_reason: Option<String>,
    last_event_at: Option<chrono::NaiveDateTime>,
}

#[derive(Debug, FromQueryResult)]
#[allow(dead_code)]
struct CursorInfo {
    last_source_event_id: Option<String>,
    last_platform_event_id: Option<String>,
    last_occurred_at_unix_secs: Option<i64>,
}

// ===== 查询函数（错误不吞，失败即 panic）=====

/// 定位 marker 消息对应的 source_event_id。
///
/// marker 位于消息正文（`secretary_message_contents.normalized_text`），不是
/// `platform_event_id`。通过账号 + 会话 + 正文 LIKE 定位，不输出完整正文。
///
/// 返回 `(account_id, conversation_id, source_event_id)` 或 `None`（尚未入库）。
async fn find_source_event_by_marker(
    db: &sea_orm::DatabaseConnection,
    self_qq_id: i64,
    group_id: i64,
    marker: &str,
) -> Option<(u64, u64, String)> {
    // 1. 找到 self_qq_id 对应的内部 account_id（BIGINT UNSIGNED，用 IdValue 读取）
    let account = IdValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id AS value FROM secretary_accounts \
         WHERE source_channel='napcat' AND platform_account_id = ?",
        [self_qq_id.to_string().into()],
    ))
    .one(db)
    .await
    .expect("account 查询必须成功")?;
    let account_id = account.value;

    // 2. 找到测试群对应的 conversation_id
    let conversation = IdValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT id AS value FROM secretary_conversations \
         WHERE account_id=? AND conversation_kind='group' AND platform_conversation_id=?",
        [account_id.into(), group_id.to_string().into()],
    ))
    .one(db)
    .await
    .expect("conversation 查询必须成功")?;
    let conversation_id = conversation.value;

    // 3. 用 marker 在 normalized_text 中定位 source_event_id
    let event = StringValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT e.source_event_id AS value \
         FROM secretary_source_events e \
         JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id \
         WHERE e.account_id=? AND e.conversation_id=? \
         AND mc.normalized_text LIKE CONCAT('%', ?, '%') \
         ORDER BY e.received_at DESC LIMIT 1",
        [account_id.into(), conversation_id.into(), marker.into()],
    ))
    .one(db)
    .await
    .expect("event 查询必须成功")?;

    Some((account_id, conversation_id, event.value))
}

/// 断言 marker 消息恰好产生一条 SourceEvent（COUNT(*) = 1），返回该 source_event_id。
async fn assert_exactly_one_source_event(
    db: &sea_orm::DatabaseConnection,
    account_id: u64,
    conversation_id: u64,
    marker: &str,
) -> String {
    let count = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_source_events e \
         JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id \
         WHERE e.account_id=? AND e.conversation_id=? \
         AND mc.normalized_text LIKE CONCAT('%', ?, '%')",
        [account_id.into(), conversation_id.into(), marker.into()],
    ))
    .one(db)
    .await
    .expect("count 查询必须成功")
    .expect("count 必须返回一行");
    assert_eq!(
        count.value, 1,
        "marker 消息必须恰好产生一条 SourceEvent，实际找到 {} 条",
        count.value
    );

    let event = StringValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT e.source_event_id AS value \
         FROM secretary_source_events e \
         JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id \
         WHERE e.account_id=? AND e.conversation_id=? \
         AND mc.normalized_text LIKE CONCAT('%', ?, '%')",
        [account_id.into(), conversation_id.into(), marker.into()],
    ))
    .one(db)
    .await
    .expect("event 查询必须成功")
    .expect("event 必须存在");
    event.value
}

/// 等待条件满足，有界轮询。`check` 返回 `Some(T)` 时成功。
async fn wait_for<T, F, Fut>(label: &str, timeout: Duration, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let started = Instant::now();
    loop {
        if let Some(value) = check().await {
            return value;
        }
        if started.elapsed() >= timeout {
            panic!("等待 {label} 超时（{timeout:?}），链路阶段缺失");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// 查询 target_event_id 所属的 thread_id。
async fn find_thread_for_event(
    db: &sea_orm::DatabaseConnection,
    target_event_id: &str,
) -> Option<String> {
    StringValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT thread_id AS value FROM secretary_thread_events WHERE source_event_id=? LIMIT 1",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("thread 查询必须成功")
    .map(|r| r.value)
}

/// 统计 target_event_id 关联的候选来源数量（三类合计）。
async fn count_candidate_sources_for_event(
    db: &sea_orm::DatabaseConnection,
    target_event_id: &str,
) -> i64 {
    let claims = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_claim_sources WHERE source_event_id=?",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("claim sources 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    let decisions = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_decision_sources WHERE source_event_id=?",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("decision sources 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    let questions = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_question_sources WHERE source_event_id=?",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("question sources 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    claims + decisions + questions
}

/// 统计 target_event_id 关联的 thread_events 成员行数（应始终为 1）。
async fn count_thread_members_for_event(
    db: &sea_orm::DatabaseConnection,
    target_event_id: &str,
) -> i64 {
    CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_events WHERE source_event_id=?",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("thread members 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0)
}

/// 读取语义游标（last_added_at, last_source_event_id）用于重启后比较不回退。
async fn read_semantic_cursor(db: &sea_orm::DatabaseConnection, thread_id: &str) -> CursorValue {
    CursorValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT last_added_at, last_source_event_id FROM secretary_thread_semantic_state \
         WHERE thread_id=?",
        [thread_id.into()],
    ))
    .one(db)
    .await
    .expect("semantic cursor 查询必须成功")
    .unwrap_or(CursorValue {
        last_added_at: None,
        last_source_event_id: None,
    })
}

/// 统计 target_event_id 关联的 proposed 候选数（按来源表精确限定，不按 thread）。
async fn count_proposed_candidates_for_event(
    db: &sea_orm::DatabaseConnection,
    target_event_id: &str,
) -> i64 {
    // 通过来源表反查候选，确保只统计引用了 target_event_id 的候选。
    let claims = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(DISTINCT cs.claim_id) AS value \
         FROM secretary_thread_claim_sources cs \
         JOIN secretary_thread_claims c ON c.claim_id=cs.claim_id \
         WHERE cs.source_event_id=? AND c.status='proposed'",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("proposed claims 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    let decisions = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(DISTINCT ds.decision_id) AS value \
         FROM secretary_thread_decision_sources ds \
         JOIN secretary_thread_decisions d ON d.decision_id=ds.decision_id \
         WHERE ds.source_event_id=? AND d.status='proposed'",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("proposed decisions 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    let questions = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(DISTINCT qs.question_id) AS value \
         FROM secretary_thread_question_sources qs \
         JOIN secretary_thread_open_questions q ON q.question_id=qs.question_id \
         WHERE qs.source_event_id=? AND q.status='open'",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("proposed questions 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    claims + decisions + questions
}

/// 统计 target_event_id 关联的 confirmed 决策数（应为 0）。
async fn count_confirmed_decisions_for_event(
    db: &sea_orm::DatabaseConnection,
    target_event_id: &str,
) -> i64 {
    CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(DISTINCT ds.decision_id) AS value \
         FROM secretary_thread_decision_sources ds \
         JOIN secretary_thread_decisions d ON d.decision_id=ds.decision_id \
         WHERE ds.source_event_id=? AND d.status='confirmed'",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("confirmed decisions 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0)
}

/// 统计 target_event_id 关联的悬空来源数（source_event_id 不存在于 secretary_source_events）。
async fn count_dangling_sources_for_event(
    db: &sea_orm::DatabaseConnection,
    target_event_id: &str,
) -> i64 {
    // 只检查引用了 target_event_id 的候选的来源表，不检查整个 thread。
    let claims = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_claim_sources cs \
         WHERE cs.claim_id IN (SELECT claim_id FROM secretary_thread_claim_sources WHERE source_event_id=?) \
         AND cs.source_event_id NOT IN (SELECT source_event_id FROM secretary_source_events)",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("dangling claim sources 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    let decisions = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_decision_sources ds \
         WHERE ds.decision_id IN (SELECT decision_id FROM secretary_thread_decision_sources WHERE source_event_id=?) \
         AND ds.source_event_id NOT IN (SELECT source_event_id FROM secretary_source_events)",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("dangling decision sources 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    let questions = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_thread_question_sources qs \
         WHERE qs.question_id IN (SELECT question_id FROM secretary_thread_question_sources WHERE source_event_id=?) \
         AND qs.source_event_id NOT IN (SELECT source_event_id FROM secretary_source_events)",
        [target_event_id.into()],
    ))
    .one(db)
    .await
    .expect("dangling question sources 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);

    claims + decisions + questions
}

/// 读取 ConnectionEpoch 信息。
async fn read_epoch(db: &sea_orm::DatabaseConnection, epoch_id: &str) -> EpochInfo {
    EpochInfo::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT connection_epoch_id, status, started_at, connected_at, ended_at, end_reason, \
         last_event_at FROM secretary_connection_epochs WHERE connection_epoch_id=?",
        [epoch_id.into()],
    ))
    .one(db)
    .await
    .expect("epoch 查询必须成功")
    .unwrap_or_else(|| panic!("ConnectionEpoch {epoch_id} 必须存在"))
}

/// 读取 ingestion cursor（account 级和 conversation 级）。
async fn read_cursor(
    db: &sea_orm::DatabaseConnection,
    account_id: u64,
    scope_key: &str,
) -> CursorInfo {
    CursorInfo::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT last_source_event_id, last_platform_event_id, last_occurred_at_unix_secs \
         FROM secretary_ingestion_cursors WHERE account_id=? AND scope_key=?",
        [account_id.into(), scope_key.into()],
    ))
    .one(db)
    .await
    .expect("cursor 查询必须成功")
    .unwrap_or(CursorInfo {
        last_source_event_id: None,
        last_platform_event_id: None,
        last_occurred_at_unix_secs: None,
    })
}

/// RAII 守卫：持有运行时 JoinHandle，Drop 时 abort。
/// 正常关闭时调用 `take()` 取出任务，防止测试 panic 时后台运行时 detach。
struct RuntimeGuard {
    task: Option<tokio::task::JoinHandle<Result<(), qqbot_server::runtime::RuntimeError>>>,
    label: String,
}

impl RuntimeGuard {
    fn new(
        label: &str,
        task: tokio::task::JoinHandle<Result<(), qqbot_server::runtime::RuntimeError>>,
    ) -> Self {
        Self {
            task: Some(task),
            label: label.to_owned(),
        }
    }

    /// 取出任务用于正常关闭。取出后 Drop 不再 abort。
    fn take(&mut self) -> tokio::task::JoinHandle<Result<(), qqbot_server::runtime::RuntimeError>> {
        self.task
            .take()
            .unwrap_or_else(|| panic!("{}: 运行时任务已被取出", self.label))
    }

    /// 检查任务是否已结束（不消费）。
    fn is_finished(&self) -> bool {
        self.task.as_ref().is_some_and(|t| t.is_finished())
    }
}

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            tracing::warn!("{}: 测试结束但运行时任务未关闭，强制 abort", self.label);
            task.abort();
        }
    }
}

/// 优雅关闭运行时任务，检查返回值。超时或 panic 都会清理。
async fn shutdown_runtime(
    shutdown_tx: watch::Sender<bool>,
    mut runtime_task: tokio::task::JoinHandle<Result<(), qqbot_server::runtime::RuntimeError>>,
    label: &str,
) {
    let _ = shutdown_tx.send(true);
    // 先保存 abort_handle，超时后强制中止，避免后台任务 detach。
    let abort_handle = runtime_task.abort_handle();
    match tokio::time::timeout(WAIT_FOR_SHUTDOWN, &mut runtime_task).await {
        Ok(Ok(Ok(()))) => {
            tracing::info!("{label}: 优雅关闭成功");
        }
        Ok(Ok(Err(error))) => {
            panic!("{label}: 运行时返回错误: {error}");
        }
        Ok(Err(panic_error)) => {
            panic!("{label}: 运行时任务 panic: {panic_error}");
        }
        Err(_) => {
            tracing::warn!("{label}: 运行时在 {WAIT_FOR_SHUTDOWN:?} 内未关闭，强制 abort");
            abort_handle.abort();
            let _ = runtime_task.await;
            panic!("{label}: 运行时在 {WAIT_FOR_SHUTDOWN:?} 内未关闭（已 abort）");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires real NapCat/MySQL/Ollama; run manually with QQBOT_E2E_* env vars; sends no QQ messages"]
async fn real_inbound_to_llm_proposed_candidate_evidence_chain_with_restart() {
    init_tracing();

    // 门禁：必须显式启用。
    if !optional_env_bool("QQBOT_E2E_ENABLED") {
        panic!("set QQBOT_E2E_ENABLED=1 to run this E2E test");
    }
    let test_group_id: i64 = required_env("QQBOT_E2E_TEST_GROUP_ID")
        .parse()
        .expect("QQBOT_E2E_TEST_GROUP_ID must be an integer");

    // 硬性断言测试群只能是 671260344。
    assert_eq!(
        test_group_id, REQUIRED_TEST_GROUP,
        "E2E 测试群必须为 {REQUIRED_TEST_GROUP}，防止误操作其他群"
    );

    let (config, config_dir) = AppConfig::load().expect("AppConfig must load from qqbot.toml/.env");
    assert!(
        config.llm.enabled,
        "LLM must be enabled for E2E (configure [llm].enabled=true in qqbot.toml)"
    );
    assert!(
        !config.llm.model.is_empty(),
        "LLM model must be configured for E2E"
    );
    let self_qq_id = config.napcat.self_qq_id;

    tracing::info!(
        self_qq_id,
        test_group_id,
        model = config.llm.model,
        "E2E 验收启动；配置已加载"
    );

    // 连接 MySQL 用于证据查询。
    let db = Database::connect(&config.database.url)
        .await
        .expect("must connect to MySQL for evidence queries");

    // 警告：检查是否有已存在的 connected epoch（可能是另一个进程在运行）。
    // 这只是警告，不是硬门禁：崩溃遗留的 epoch 也可能保持 connected。
    let existing = CountValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT COUNT(*) AS value FROM secretary_connection_epochs ce \
         JOIN secretary_accounts a ON a.id=ce.account_id \
         WHERE a.source_channel='napcat' AND a.platform_account_id=? AND ce.status='connected'",
        [self_qq_id.to_string().into()],
    ))
    .one(&db)
    .await
    .expect("existing epoch 查询必须成功")
    .map(|r| r.value)
    .unwrap_or(0);
    if existing > 0 {
        tracing::warn!(
            existing,
            "数据库中已有 connected 状态的 ConnectionEpoch；请确认没有另一个 qqbot-server 实例在运行同一账号"
        );
    }

    // 生成唯一 marker（不打印完整消息正文到日志/文档，只打印 marker 前缀）。
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let marker = format!("[E2E-{timestamp}]");

    // 记录启动时间，用于后续只认本次新建的 epoch。
    // 在 spawn 之前记录，确保不会漏掉刚创建的 epoch。
    let runtime_start = chrono::Utc::now().naive_utc();

    // ===== 第一阶段：首次启动 =====

    let (shutdown_tx_1, shutdown_rx_1) = watch::channel(false);
    let runtime_task_1 = tokio::spawn(runtime::run_with_cancellation(
        config.clone(),
        config_dir.clone(),
        shutdown_rx_1,
    ));
    let mut guard_1 = RuntimeGuard::new("第一次运行时", runtime_task_1);

    // 等待 NapCat WS 连接成功后再打印提示，避免用户过早发送导致丢消息。
    tracing::info!("等待 NapCat WebSocket 连接成功...");
    wait_for("NapCat WS 连接", WAIT_FOR_MESSAGE, || async {
        let count = CountValue::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM secretary_connection_epochs ce \
             JOIN secretary_accounts a ON a.id=ce.account_id \
             WHERE a.source_channel='napcat' AND a.platform_account_id=? \
             AND ce.status='connected' AND ce.started_at >= ?",
            [self_qq_id.to_string().into(), runtime_start.into()],
        ))
        .one(&db)
        .await
        .expect("WS 连接查询必须成功")
        .map(|r| r.value)
        .unwrap_or(0);
        (count > 0).then_some(())
    })
    .await;

    // 连接成功后才打印提示，防止用户在 WS 连接建立前发送消息导致丢消息。
    println!();
    println!("========================================================");
    println!("NapCat WS 已连接。请在测试群 {test_group_id} 中手工发送");
    println!("以 {marker} 开头的请求消息。");
    println!(
        "（程序不会主动发送 QQ 消息。等待最多 {} 秒。）",
        WAIT_FOR_MESSAGE.as_secs()
    );
    println!("========================================================");
    println!();

    // 等待 marker 消息入库。
    tracing::info!("等待 marker 消息入库...");
    let (account_id, conversation_id, target_event_id) =
        wait_for("marker 消息入库", WAIT_FOR_MESSAGE, || async {
            find_source_event_by_marker(&db, self_qq_id, test_group_id, &marker).await
        })
        .await;

    tracing::info!(
        target_event_id = %target_event_id,
        account_id,
        conversation_id,
        "SourceEvent 已入库"
    );

    // 断言恰好一条（COUNT=1 后已读取）。
    let verified_event_id =
        assert_exactly_one_source_event(&db, account_id, conversation_id, &marker).await;
    assert_eq!(
        verified_event_id, target_event_id,
        "COUNT=1 查询与定位查询返回的 source_event_id 必须一致"
    );

    // 通过 secretary_event_ingestion 证明 target_event_id 属于本次新建的 epoch。
    let ingestion_epoch = StringValue::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::MySql,
        "SELECT ie.connection_epoch_id AS value FROM secretary_event_ingestion ie \
         JOIN secretary_connection_epochs ce ON ce.connection_epoch_id=ie.connection_epoch_id \
         WHERE ie.source_event_id=? AND ce.started_at >= ?",
        [target_event_id.clone().into(), runtime_start.into()],
    ))
    .one(&db)
    .await
    .expect("ingestion epoch 查询必须成功");
    assert!(
        ingestion_epoch.is_some(),
        "target_event_id 必须关联到本次启动后新建的 ConnectionEpoch"
    );
    let epoch_id_1 = ingestion_epoch.unwrap().value;

    // 验证 ConnectionEpoch 的 last_event_at 已推进。
    let epoch_1 = read_epoch(&db, &epoch_id_1).await;
    assert!(
        epoch_1.last_event_at.is_some(),
        "ConnectionEpoch 的 last_event_at 必须已推进"
    );

    // 验证 account 级和 conversation 级游标存在且指向 target_event_id。
    let account_cursor = read_cursor(&db, account_id, "account").await;
    assert_eq!(
        account_cursor.last_source_event_id.as_deref(),
        Some(target_event_id.as_str()),
        "account 级游标必须指向 target_event_id"
    );

    let conv_scope = format!("group:{test_group_id}");
    let conv_cursor = read_cursor(&db, account_id, &conv_scope).await;
    assert_eq!(
        conv_cursor.last_source_event_id.as_deref(),
        Some(target_event_id.as_str()),
        "conversation 级游标必须指向 target_event_id"
    );

    // 等待 Thread 投影。
    tracing::info!("等待 EventThread 投影...");
    let thread_id = wait_for("EventThread 投影", WAIT_FOR_DERIVATION, || async {
        find_thread_for_event(&db, &target_event_id).await
    })
    .await;

    tracing::info!(thread_id = %thread_id, "Thread 投影完成");

    // 等待 LLM proposed 候选（按 target_event_id 精确验证）。
    // 语义 Worker 按线程顺序处理（LIMIT 1），若数据库中有大量历史遗留线程，
    // 目标线程可能需要等待较长时间才轮到。
    tracing::info!("等待 LLM proposed 候选...");
    let poll_started = Instant::now();
    wait_for("LLM proposed 候选", WAIT_FOR_DERIVATION, || async {
        let count = count_candidate_sources_for_event(&db, &target_event_id).await;
        if count > 0 {
            return Some(count);
        }
        // 每 10 秒输出一次进度，帮助定位卡在哪。
        if poll_started.elapsed().as_secs().is_multiple_of(10) {
            let pending = CountValue::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "SELECT COUNT(*) AS value FROM secretary_event_threads t \
                 LEFT JOIN secretary_thread_semantic_state s ON s.thread_id = t.thread_id \
                 WHERE (s.thread_id IS NULL OR s.lease_token IS NULL OR s.lease_expires_at < NOW()) \
                 AND EXISTS (SELECT 1 FROM secretary_effective_thread_events te \
                     JOIN secretary_source_events e ON e.source_event_id = te.source_event_id \
                     JOIN secretary_conversations c ON c.id = e.conversation_id \
                     JOIN secretary_message_contents mc ON mc.source_event_id = e.source_event_id \
                     WHERE te.thread_id = t.thread_id AND c.memory_mode IN ('normal','local_only') \
                     AND mc.content_mode IN ('normal','local_only') \
                     AND (s.last_added_at IS NULL OR te.added_at > s.last_added_at))",
                [],
            ))
            .one(&db)
            .await
            .map(|r| r.map(|v| v.value).unwrap_or(0))
            .unwrap_or(0);
            tracing::info!(
                elapsed_secs = poll_started.elapsed().as_secs(),
                pending_semantic_threads = pending,
                "仍在等待 LLM 候选，目标线程排队中"
            );
        }
        None
    })
    .await;

    let candidate_sources = count_candidate_sources_for_event(&db, &target_event_id).await;
    tracing::info!(candidate_sources, "候选来源已生成，验证精确引用");

    // 验证无悬空引用（scoped 到 target_event_id 的候选来源）。
    let dangling = count_dangling_sources_for_event(&db, &target_event_id).await;
    assert_eq!(dangling, 0, "target_event_id 关联的候选来源不应有悬空引用");

    // 验证无 confirmed 决策（按 target_event_id 精确限定）。
    let confirmed = count_confirmed_decisions_for_event(&db, &target_event_id).await;
    assert_eq!(
        confirmed, 0,
        "target_event_id 关联的候选不应有 confirmed 决策"
    );

    // 记录 baseline 供重启验收比较（全部按 target_event_id 精确限定）。
    let baseline_thread_members = count_thread_members_for_event(&db, &target_event_id).await;
    let baseline_candidate_sources = count_candidate_sources_for_event(&db, &target_event_id).await;
    let baseline_proposed = count_proposed_candidates_for_event(&db, &target_event_id).await;
    let baseline_cursor = read_semantic_cursor(&db, &thread_id).await;

    assert_eq!(
        baseline_thread_members, 1,
        "target_event_id 应恰好属于一个线程成员关系"
    );
    assert!(
        baseline_candidate_sources > 0,
        "target_event_id 应有候选来源"
    );

    tracing::info!(
        baseline_thread_members,
        baseline_candidate_sources,
        baseline_proposed,
        ?baseline_cursor,
        "第一阶段验收通过，记录 baseline"
    );

    // ===== 优雅关闭（第一次） =====
    tracing::info!("发送关闭信号，验证优雅关闭...");
    let runtime_task_1 = guard_1.take();
    shutdown_runtime(shutdown_tx_1, runtime_task_1, "第一次运行时").await;
    drop(guard_1);

    // 验证 ConnectionEpoch 已结束。
    let epoch_1_after = read_epoch(&db, &epoch_id_1).await;
    assert!(
        epoch_1_after.ended_at.is_some(),
        "第一次关闭后 ConnectionEpoch 必须有 ended_at"
    );
    assert!(
        epoch_1_after.end_reason.is_some(),
        "第一次关闭后 ConnectionEpoch 必须有 end_reason"
    );

    // ===== 第二阶段：重启验收 =====
    tracing::info!("第二次启动，验证重启幂等性...");

    // 在 spawn 之前记录启动时间，确保不会漏掉刚创建的 epoch。
    let restart_start = chrono::Utc::now().naive_utc();

    let (shutdown_tx_2, shutdown_rx_2) = watch::channel(false);
    let runtime_task_2 = tokio::spawn(runtime::run_with_cancellation(
        config.clone(),
        config_dir.clone(),
        shutdown_rx_2,
    ));
    let mut guard_2 = RuntimeGuard::new("第二次运行时", runtime_task_2);

    // 等待第二次 WS 连接成功。
    wait_for("第二次 NapCat WS 连接", WAIT_FOR_MESSAGE, || async {
        let count = CountValue::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT COUNT(*) AS value FROM secretary_connection_epochs ce \
             JOIN secretary_accounts a ON a.id=ce.account_id \
             WHERE a.source_channel='napcat' AND a.platform_account_id=? \
             AND ce.status='connected' AND ce.started_at >= ?",
            [self_qq_id.to_string().into(), restart_start.into()],
        ))
        .one(&db)
        .await
        .expect("第二次 WS 连接查询必须成功")
        .map(|r| r.value)
        .unwrap_or(0);
        (count > 0).then_some(())
    })
    .await;

    // 跨扫描周期重启稳定性验证：等待至少两个 thread_semantics.scan_interval_ms 周期，
    // 期间确认运行时任务未意外结束。这不是"重启幂等性"验证（第二次启动不会重放同一个
    // 平台事件），而是确认重启后 Worker 正常恢复运行且没有对目标线程产生重复派生。
    // 不依赖 updated_at 推进（空扫描不会写该字段），改为按配置周期等待足够时间。
    let scan_interval = config.thread_semantics.scan_interval_ms;
    let two_scan_cycles = Duration::from_millis(scan_interval.saturating_mul(2).max(2000));
    tracing::info!(
        scan_interval_ms = scan_interval,
        wait_duration = ?two_scan_cycles,
        "等待两个语义扫描周期（跨扫描周期重启稳定性）"
    );
    let scan_deadline = Instant::now() + two_scan_cycles;
    while Instant::now() < scan_deadline {
        assert!(
            !guard_2.is_finished(),
            "第二次运行时任务在等待扫描周期期间意外结束"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // 验证重启后 scoped 计数未增加（全部按 target_event_id 精确限定）。
    let after_thread_members = count_thread_members_for_event(&db, &target_event_id).await;
    let after_candidate_sources = count_candidate_sources_for_event(&db, &target_event_id).await;
    let after_proposed = count_proposed_candidates_for_event(&db, &target_event_id).await;
    let after_cursor = read_semantic_cursor(&db, &thread_id).await;

    assert_eq!(
        after_thread_members, baseline_thread_members,
        "重启后 target_event_id 的线程成员关系不应增加：baseline={} after={}",
        baseline_thread_members, after_thread_members
    );
    assert_eq!(
        after_candidate_sources, baseline_candidate_sources,
        "重启后 target_event_id 的候选来源不应增加：baseline={} after={}",
        baseline_candidate_sources, after_candidate_sources
    );
    assert_eq!(
        after_proposed, baseline_proposed,
        "重启后 target_event_id 的 proposed 候选不应增加：baseline={} after={}",
        baseline_proposed, after_proposed
    );

    // 游标不回退：比较完整二元组 (last_added_at, last_source_event_id)。
    // last_added_at 可以前进（新消息），但不能回退。
    if let (Some(baseline_ts), Some(after_ts)) =
        (baseline_cursor.last_added_at, after_cursor.last_added_at)
    {
        assert!(
            after_ts >= baseline_ts,
            "语义游标 last_added_at 不应回退：baseline={} after={}",
            baseline_ts,
            after_ts
        );
    }
    // last_source_event_id 不应回退为更早的值（允许前进或保持不变）。
    if let (Some(baseline_sid), Some(after_sid)) = (
        baseline_cursor.last_source_event_id,
        after_cursor.last_source_event_id,
    ) {
        // 如果时间相同，source_event_id 不应变；如果时间前进，source_event_id 可变。
        if baseline_cursor.last_added_at == after_cursor.last_added_at {
            assert_eq!(
                after_sid, baseline_sid,
                "语义游标 last_source_event_id 在时间不变时不应变化"
            );
        }
    }

    tracing::info!("跨扫描周期重启稳定性验证通过：无重复投影、无重复候选、游标未回退");

    // ===== 第二次优雅关闭 =====
    tracing::info!("第二次优雅关闭...");
    let runtime_task_2 = guard_2.take();
    shutdown_runtime(shutdown_tx_2, runtime_task_2, "第二次运行时").await;
    drop(guard_2);

    tracing::info!("E2E 验收全部通过");
    println!("E2E 验收成功：真实消息入站闭环 + 跨扫描周期重启稳定性全部通过。");
}
