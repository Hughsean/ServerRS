use async_trait::async_trait;
use qq_open_platform::{
    GatewayRunError, GatewaySession, GatewaySessionStoreT, QqGatewayEvent, QqGatewayEventKind,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement};

use crate::qq_open_platform::OfficialRawEventStoreT;

pub(crate) struct MySqlGatewaySessionStore {
    db: DatabaseConnection,
}

impl MySqlGatewaySessionStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GatewaySessionStoreT for MySqlGatewaySessionStore {
    async fn load(&self, app_id: &str) -> Result<Option<GatewaySession>, GatewayRunError> {
        Ok(SessionRow::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::MySql,
            "SELECT app_id, session_id, last_sequence FROM secretary_qq_gateway_sessions WHERE app_id = ?",
            [app_id.into()],
        ))
        .one(&self.db)
        .await
        .map_err(|error| GatewayRunError::Persistence(error.to_string()))?
        .map(|row| GatewaySession {
            app_id: row.app_id,
            session_id: row.session_id,
            sequence: row.last_sequence,
        }))
    }

    async fn save(&self, session: &GatewaySession) -> Result<(), GatewayRunError> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_qq_gateway_sessions (app_id, session_id, last_sequence)
               VALUES (?, ?, ?)
               ON DUPLICATE KEY UPDATE
                 last_sequence = IF(session_id = VALUES(session_id),
                   GREATEST(last_sequence, VALUES(last_sequence)), VALUES(last_sequence)),
                 session_id = VALUES(session_id)"#,
                [
                    session.app_id.clone().into(),
                    session.session_id.clone().into(),
                    session.sequence.into(),
                ],
            ))
            .await
            .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
        Ok(())
    }

    async fn clear(&self, app_id: &str) -> Result<(), GatewayRunError> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                "DELETE FROM secretary_qq_gateway_sessions WHERE app_id = ?",
                [app_id.into()],
            ))
            .await
            .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, FromQueryResult)]
struct SessionRow {
    app_id: String,
    session_id: String,
    last_sequence: u64,
}

pub(crate) struct MySqlRawEventStore {
    db: DatabaseConnection,
}

impl MySqlRawEventStore {
    pub(crate) fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OfficialRawEventStoreT for MySqlRawEventStore {
    async fn persist(
        &self,
        source_event_id: &str,
        event: &QqGatewayEvent,
    ) -> Result<(), GatewayRunError> {
        let kind = match event.event_kind {
            QqGatewayEventKind::C2cMessage => "c2c_message",
            QqGatewayEventKind::GroupAtMessage => "group_at_message",
            QqGatewayEventKind::GroupMessage => "group_message",
        };
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::MySql,
                r#"INSERT INTO secretary_qq_raw_events
                 (source_event_id, app_id, event_kind, envelope_json)
               VALUES (?, ?, ?, CAST(? AS JSON))
               ON DUPLICATE KEY UPDATE source_event_id = VALUES(source_event_id)"#,
                [
                    source_event_id.into(),
                    event.app_id.clone().into(),
                    kind.into(),
                    event.raw_envelope.clone().into(),
                ],
            ))
            .await
            .map_err(|error| GatewayRunError::Persistence(error.to_string()))?;
        Ok(())
    }
}
