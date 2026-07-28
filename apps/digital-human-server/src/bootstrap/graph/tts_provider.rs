use std::sync::Arc;

use digital_human::domain::tts::TtsProvider;
use digital_human::infra::tts::volcengine_provider::VolcengineTtsProvider;

use crate::app::tts::tts_service::TtsService;

use super::BootstrapContext;

/// 数字人对话语音服务；仅在显式启用时创建，避免影响未配置部署。
pub fn build_tts_service(
    ctx: &BootstrapContext<'_>,
) -> Result<Option<Arc<TtsService>>, std::io::Error> {
    if !ctx.config.tts.enabled {
        return Ok(None);
    }

    let provider: Arc<dyn TtsProvider> = Arc::new(VolcengineTtsProvider::new(&ctx.config.tts));
    let service = TtsService::new(&ctx.config.tts, provider)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(Some(Arc::new(service)))
}
