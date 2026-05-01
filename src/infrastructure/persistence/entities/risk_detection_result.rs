use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, DeriveEntityModel)]
#[sea_orm(table_name = "risk_detection_results")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: u64,
    pub user_id: u64,
    pub message_id: Option<u64>,
    pub conversation_id: Option<u64>,
    pub risk_level: String,
    pub polarity: String,
    pub intent: String,
    pub target: String,
    pub confidence: f64,
    pub evidence: String,
    pub reason: Option<String>,
    pub raw_payload: Option<String>,
    pub model_name: Option<String>,
    pub detector_version: Option<String>,
    pub is_processed: bool,
    pub process_notes: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
