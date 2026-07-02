use std::sync::Arc;

use crate::app::depression::depression_service::DepressionService;
use crate::app::diary::diary_service::DiaryService;
use crate::app::psychology::psychology_service::PsychologyService;

use super::BootstrapContext;

pub struct WellbeingServices {
    pub psychology: Arc<PsychologyService>,
    pub depression: Arc<DepressionService>,
    pub diaries: Arc<DiaryService>,
}

pub fn build_wellbeing_services(ctx: &BootstrapContext<'_>) -> WellbeingServices {
    let psychology = Arc::new(PsychologyService::new(Arc::clone(
        &ctx.repos.psychology_repo,
    )));
    let depression = Arc::new(DepressionService::new(Arc::clone(
        &ctx.repos.depression_repo,
    )));
    let diaries = Arc::new(DiaryService::new(
        Arc::clone(&ctx.repos.diary_repo),
        Some(Arc::clone(&ctx.infra.ollama_provider)),
    ));

    WellbeingServices {
        psychology,
        depression,
        diaries,
    }
}
