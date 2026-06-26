use std::sync::Arc;

use crate::domain::diary::{DiaryRepoT, NewUserDiary, UserDiary, UserDiaryUpdate};
use crate::domain::llm::{ChatMessage, LlmClient};
use crate::shared::error::AppError;

pub struct DiaryService {
    pub repo: Arc<dyn DiaryRepoT>,
    pub llm: Option<Arc<dyn LlmClient>>,
}

impl DiaryService {
    pub fn new(repo: Arc<dyn DiaryRepoT>, llm: Option<Arc<dyn LlmClient>>) -> Self {
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
        if content.trim().is_empty() {
            return Err(AppError::Validation("diary content cannot be empty".into()));
        }
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

        self.schedule_mood_analysis(diary.id, diary.content.clone());

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
        if title.is_none() && content.is_none() {
            return Err(AppError::Validation(
                "at least one diary field must be provided".into(),
            ));
        }
        if content
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(AppError::Validation("diary content cannot be empty".into()));
        }
        let diary = self.get(user_id, id).await?;
        let content_for_analysis = content.clone();
        let updated = self
            .repo
            .update(
                diary.id,
                UserDiaryUpdate {
                    title: title.map(|value| {
                        if value.trim().is_empty() {
                            "无标题".to_string()
                        } else {
                            value
                        }
                    }),
                    content,
                    mood_description: content_for_analysis.as_ref().map(|_| None),
                },
            )
            .await?;
        if let Some(content) = content_for_analysis {
            self.schedule_mood_analysis(updated.id, content);
        }
        Ok(updated)
    }

    pub async fn delete(&self, user_id: u64, id: u64) -> Result<(), AppError> {
        let diary = self.get(user_id, id).await?;
        let _ = self.repo.delete_by_id(diary.id).await?;
        Ok(())
    }

    fn schedule_mood_analysis(&self, diary_id: u64, content: String) {
        let Some(llm) = self.llm.clone() else {
            return;
        };
        let repo = self.repo.clone();
        tokio::spawn(async move {
            let prompt = format!(
                "请分析以下日记的情绪，返回JSON格式 {{\"mood_description\": \"...\"}}。\n\n{content}"
            );
            let messages = vec![ChatMessage {
                role: "user".to_string(),
                content: prompt,
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }];
            let response = llm.chat(&messages).await;
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&response) {
                if let Some(mood_description) = parsed
                    .get("mood_description")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                {
                    if let Err(e) = repo
                        .update_mood(diary_id, mood_description.to_string())
                        .await
                    {
                        tracing::warn!("mood update failed for diary {diary_id}: {e}");
                    }
                } else {
                    tracing::warn!("mood analysis missing mood_description for diary {diary_id}");
                }
            } else {
                tracing::warn!("mood analysis parse failed for diary {diary_id}");
            }
        });
    }
}
