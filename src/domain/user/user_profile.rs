use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// User psychological profile / behavioral traits.
/// JSON-array fields are stored as `Vec<String>` in Rust.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: u64,
    pub user_id: u64,
    pub interests: Option<Vec<String>>,
    pub personality_traits: Option<Vec<String>>,
    pub interaction_preferences: Option<Vec<String>>,
    pub emotional_tendency: Option<Vec<String>>,
    pub learning_records: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewUserProfile {
    pub user_id: u64,
    pub interests: Option<Vec<String>>,
    pub personality_traits: Option<Vec<String>>,
    pub interaction_preferences: Option<Vec<String>>,
    pub emotional_tendency: Option<Vec<String>>,
    pub learning_records: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct UserProfileUpdate {
    pub interests: Option<Option<Vec<String>>>,
    pub personality_traits: Option<Option<Vec<String>>>,
    pub interaction_preferences: Option<Option<Vec<String>>>,
    pub emotional_tendency: Option<Option<Vec<String>>>,
    pub learning_records: Option<Option<Vec<String>>>,
}

impl UserProfileUpdate {
    pub fn has_any(&self) -> bool {
        self.interests.is_some()
            || self.personality_traits.is_some()
            || self.interaction_preferences.is_some()
            || self.emotional_tendency.is_some()
            || self.learning_records.is_some()
    }
}
