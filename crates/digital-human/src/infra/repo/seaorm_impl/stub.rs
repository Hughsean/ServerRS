//! Temporary stub repositories — replaced by real SeaORM implementations in production.
//! Retained only for test contexts that don't have a database connection.

use crate::domain::depression::{
    DepressionAssessment, DepressionRepoT, DepressionScale, NewDepressionAssessment,
};
use crate::domain::diary::{DiaryRepoT, NewUserDiary, UserDiary, UserDiaryUpdate};
use crate::domain::music::{
    MusicRepoT, MusicTrack, MusicTrackListItem, MusicTrackUpdate, NewMusicTrack,
};
use crate::domain::psychology::{
    KnowledgeFavorite, NewContentLike, NewKnowledgeFavorite, NewPsychologyArticle,
    NewPsychologyCategory, NewPsychologyQna, NewPsychologyResource, PsychologyArticle,
    PsychologyCategory, PsychologyQna, PsychologyRepoT, PsychologyResource,
};
use crate::shared::error::AppError;
use async_trait::async_trait;

macro_rules! stub_err {
    () => {
        Err(AppError::Internal(
            "stub repository — not connected to database".into(),
        ))
    };
}

pub struct StubDepressionRepo;
#[async_trait]
impl DepressionRepoT for StubDepressionRepo {
    async fn find_scale_by_id(&self, _: u16) -> Result<Option<DepressionScale>, AppError> {
        Ok(None)
    }
    async fn list_scales(&self) -> Result<Vec<DepressionScale>, AppError> {
        Ok(vec![])
    }
    async fn save_assessment(
        &self,
        _: NewDepressionAssessment,
        _: i16,
    ) -> Result<DepressionAssessment, AppError> {
        stub_err!()
    }
    async fn find_assessment_by_id(
        &self,
        _: u64,
    ) -> Result<Option<DepressionAssessment>, AppError> {
        Ok(None)
    }
    async fn find_assessments_by_user_id(
        &self,
        _: u64,
        _: u64,
        _: u64,
    ) -> Result<(Vec<DepressionAssessment>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn update_assessment(
        &self,
        _: u64,
        _: Option<String>,
    ) -> Result<DepressionAssessment, AppError> {
        stub_err!()
    }
    async fn delete_assessment(&self, _: u64) -> Result<u64, AppError> {
        stub_err!()
    }
}

pub struct StubDiaryRepo;
#[async_trait]
impl DiaryRepoT for StubDiaryRepo {
    async fn save(&self, _: NewUserDiary) -> Result<UserDiary, AppError> {
        stub_err!()
    }
    async fn find_by_id(&self, _: u64) -> Result<Option<UserDiary>, AppError> {
        Ok(None)
    }
    async fn find_by_user_id(
        &self,
        _: u64,
        _: u64,
        _: u64,
    ) -> Result<(Vec<UserDiary>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn update(&self, _: u64, _: UserDiaryUpdate) -> Result<UserDiary, AppError> {
        stub_err!()
    }
    async fn update_mood(&self, _: u64, _: String) -> Result<(), AppError> {
        stub_err!()
    }
    async fn delete_by_id(&self, _: u64) -> Result<bool, AppError> {
        Ok(false)
    }
}

pub struct StubMusicRepo;
#[async_trait]
impl MusicRepoT for StubMusicRepo {
    async fn save(&self, _: NewMusicTrack) -> Result<MusicTrack, AppError> {
        stub_err!()
    }
    async fn find_by_id(&self, _: u64) -> Result<Option<MusicTrack>, AppError> {
        Ok(None)
    }
    async fn find_all(
        &self,
        _: Option<String>,
        _: Option<String>,
        _: u64,
        _: u64,
    ) -> Result<(Vec<MusicTrackListItem>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn find_all_admin(
        &self,
        _: Option<String>,
        _: Option<String>,
        _: Option<i8>,
        _: u64,
        _: u64,
    ) -> Result<(Vec<MusicTrackListItem>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn update(&self, _: u64, _: MusicTrackUpdate) -> Result<MusicTrack, AppError> {
        stub_err!()
    }
    async fn delete_by_id(&self, _: u64) -> Result<bool, AppError> {
        Ok(false)
    }

    async fn count_all(&self) -> Result<u64, AppError> {
        stub_err!()
    }

    async fn count_trend(&self, _days: u32) -> Result<Vec<(String, u64)>, AppError> {
        stub_err!()
    }
}

pub struct StubPsychologyRepo;
#[async_trait]
impl PsychologyRepoT for StubPsychologyRepo {
    async fn find_category_by_id(&self, _: u64) -> Result<Option<PsychologyCategory>, AppError> {
        Ok(None)
    }
    async fn list_categories(&self) -> Result<Vec<PsychologyCategory>, AppError> {
        Ok(vec![])
    }
    async fn list_categories_admin(&self) -> Result<Vec<PsychologyCategory>, AppError> {
        Ok(vec![])
    }
    async fn create_category(
        &self,
        _: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        stub_err!()
    }
    async fn update_category(
        &self,
        _: u64,
        _: NewPsychologyCategory,
    ) -> Result<PsychologyCategory, AppError> {
        stub_err!()
    }
    async fn delete_category(&self, _: u64) -> Result<bool, AppError> {
        stub_err!()
    }
    async fn find_article_by_id(&self, _: u64) -> Result<Option<PsychologyArticle>, AppError> {
        Ok(None)
    }
    async fn list_articles(
        &self,
        _: u64,
        _: u64,
        _: Option<String>,
        _: Option<u64>,
        _: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn find_article_by_id_admin(
        &self,
        _: u64,
    ) -> Result<Option<PsychologyArticle>, AppError> {
        Ok(None)
    }
    async fn list_articles_admin(
        &self,
        _: u64,
        _: u64,
        _: Option<String>,
        _: Option<u64>,
        _: Option<bool>,
    ) -> Result<(Vec<PsychologyArticle>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn create_article(&self, _: NewPsychologyArticle) -> Result<PsychologyArticle, AppError> {
        stub_err!()
    }
    async fn update_article(
        &self,
        _: u64,
        _: NewPsychologyArticle,
    ) -> Result<PsychologyArticle, AppError> {
        stub_err!()
    }
    async fn delete_article(&self, _: u64) -> Result<bool, AppError> {
        stub_err!()
    }
    async fn find_qna_by_id(&self, _: u64) -> Result<Option<PsychologyQna>, AppError> {
        Ok(None)
    }
    async fn list_qnas(
        &self,
        _: u64,
        _: u64,
        _: Option<u64>,
        _: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn find_qna_by_id_admin(&self, _: u64) -> Result<Option<PsychologyQna>, AppError> {
        Ok(None)
    }
    async fn list_qnas_admin(
        &self,
        _: u64,
        _: u64,
        _: Option<u64>,
        _: Option<bool>,
        _: Option<bool>,
    ) -> Result<(Vec<PsychologyQna>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn create_qna(&self, _: NewPsychologyQna) -> Result<PsychologyQna, AppError> {
        stub_err!()
    }
    async fn update_qna(&self, _: u64, _: NewPsychologyQna) -> Result<PsychologyQna, AppError> {
        stub_err!()
    }
    async fn delete_qna(&self, _: u64) -> Result<bool, AppError> {
        stub_err!()
    }
    async fn find_resource_by_id(&self, _: u64) -> Result<Option<PsychologyResource>, AppError> {
        Ok(None)
    }
    async fn list_resources(
        &self,
        _: u64,
        _: u64,
        _: Option<u64>,
        _: Option<String>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn find_resource_by_id_admin(
        &self,
        _: u64,
    ) -> Result<Option<PsychologyResource>, AppError> {
        Ok(None)
    }
    async fn list_resources_admin(
        &self,
        _: u64,
        _: u64,
        _: Option<u64>,
        _: Option<String>,
        _: Option<bool>,
    ) -> Result<(Vec<PsychologyResource>, u64), AppError> {
        Ok((vec![], 0))
    }
    async fn create_resource(
        &self,
        _: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        stub_err!()
    }
    async fn update_resource(
        &self,
        _: u64,
        _: NewPsychologyResource,
    ) -> Result<PsychologyResource, AppError> {
        stub_err!()
    }
    async fn delete_resource(&self, _: u64) -> Result<bool, AppError> {
        stub_err!()
    }
    async fn toggle_favorite(&self, _: NewKnowledgeFavorite) -> Result<bool, AppError> {
        stub_err!()
    }
    async fn check_favorite(&self, _: u64, _: &str, _: u64) -> Result<bool, AppError> {
        Ok(false)
    }
    async fn list_favorites(
        &self,
        _: u64,
        _: Option<&str>,
    ) -> Result<Vec<KnowledgeFavorite>, AppError> {
        Ok(vec![])
    }
    async fn toggle_like(&self, _: NewContentLike) -> Result<bool, AppError> {
        Err(AppError::Internal("likes not supported".into()))
    }
}
