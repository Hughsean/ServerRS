use std::sync::Arc;

use crate::domain::diary::{DiaryRepository, NewUserDiary, UserDiary, UserDiaryUpdate};
use crate::domain::llm::{ChatMessage, LlmClient};
use crate::shared::error::AppError;

pub struct DiaryService {
    pub repo: Arc<dyn DiaryRepository>,
    pub llm: Option<Arc<dyn LlmClient>>,
}

impl DiaryService {
    pub fn new(repo: Arc<dyn DiaryRepository>, llm: Option<Arc<dyn LlmClient>>) -> Self {
        Self { repo, llm }
    }

    /// Creates a diary entry. Title defaults to "无标题" if empty.
    /// Tags are NOT persisted — the user_diaries table has no tags column.
    /// Spawns a background task for mood analysis if an LLM client is available.
    pub async fn create(
        &self,
        user_id: u64,
        title: String,
        content: String,
    ) -> Result<UserDiary, AppError> {
        let title = if title.trim().is_empty() {
            "无标题".to_string()
        } else {
            title
        };

        let diary = self
            .repo
            .save(NewUserDiary {
                user_id,
                title,
                content,
            })
            .await?;

        if let Some(llm) = self.llm.clone() {
            let diary_id = diary.id;
            let repo = self.repo.clone();
            let content_clone = diary.content.clone();
            tokio::spawn(async move {
                let prompt = format!(
                    "请分析以下日记的情绪，返回JSON格式 {{\"mood_description\": \"...\"}}。\n\n{content_clone}"
                );
                let messages = vec![ChatMessage {
                    role: "user".to_string(),
                    content: prompt,
                    tool_calls: None,
                    tool_call_id: None,
                }];
                let response = llm.chat(&messages).await;
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&response) {
                    let mood_description = parsed
                        .get("mood_description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    if let Err(e) = repo.update_mood(diary_id, mood_description).await {
                        tracing::warn!("mood update failed for diary {diary_id}: {e}");
                    }
                } else {
                    tracing::warn!("mood analysis parse failed for diary {diary_id}");
                }
            });
        }

        Ok(diary)
    }

    pub async fn list(
        &self,
        user_id: u64,
        page: u64,
        size: u64,
    ) -> Result<(Vec<UserDiary>, u64), AppError> {
        let offset = page.saturating_sub(1) * size;
        self.repo.find_by_user_id(user_id, size, offset).await
    }

    pub async fn get(&self, user_id: u64, id: u64) -> Result<UserDiary, AppError> {
        let diary = self
            .repo
            .find_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("diary not found".into()))?;
        if diary.user_id != user_id {
            return Err(AppError::Forbidden("not your diary".into()));
        }
        Ok(diary)
    }

    pub async fn update(
        &self,
        user_id: u64,
        id: u64,
        title: Option<String>,
        content: Option<String>,
    ) -> Result<UserDiary, AppError> {
        let diary = self.get(user_id, id).await?;
        self.repo
            .update(
                diary.id,
                UserDiaryUpdate {
                    title,
                    content,
                    mood_description: None,
                },
            )
            .await
    }

    pub async fn delete(&self, user_id: u64, id: u64) -> Result<(), AppError> {
        let diary = self.get(user_id, id).await?;
        let _ = self.repo.delete_by_id(diary.id).await?;
        Ok(())
    }
}
