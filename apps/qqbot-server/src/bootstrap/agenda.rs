//! Agenda 到期通知扫描装配。

use std::sync::Arc;

use personal_secretary::{AgendaUseCase, SystemClock, build_mysql_agenda_store};
use sea_orm::DatabaseConnection;

use crate::agenda_notification_worker::spawn_agenda_notification_worker;
use crate::bootstrap::workers::WorkerHandles;
use crate::config::AppConfig;
use crate::runtime::RuntimeError;

/// 创建唯一的 Agenda 到期扫描任务；该任务仅生成统一策略候选与求值请求。
pub(crate) fn assemble_agenda_notification_worker(
    handles: &mut WorkerHandles,
    db: DatabaseConnection,
    config: &AppConfig,
) -> Result<(), RuntimeError> {
    if !config.agenda.enabled {
        tracing::info!("Agenda 到期通知扫描已禁用（agenda.enabled=false）");
        return Ok(());
    }
    let agenda = Arc::new(AgendaUseCase::new(
        build_mysql_agenda_store(db),
        Arc::new(SystemClock),
    ));
    handles.agenda_notification = Some(spawn_agenda_notification_worker(
        agenda,
        config.agenda.clone(),
    ));
    tracing::info!(
        scan_interval_ms = config.agenda.scan_interval_ms,
        batch_size = config.agenda.batch_size,
        "Agenda 到期通知扫描已启用；仅生成统一策略候选与求值请求"
    );
    Ok(())
}
