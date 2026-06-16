use serde::Deserialize;

/// QQ Bot 模块顶级配置
#[derive(Debug, Clone, Deserialize)]
pub struct QqBotConfig {
    /// 主开关（默认关闭）
    #[serde(default)]
    pub enabled: bool,
    /// NapCat 反向 WebSocket 监听地址（NapCat 连我们）
    #[serde(default = "default_ws_host")]
    pub ws_host: String,
    /// NapCat 反向 WebSocket 监听端口
    #[serde(default = "default_ws_port")]
    pub ws_port: u16,
    /// NapCat HTTP API 地址
    #[serde(default = "default_http_base_url")]
    pub http_base_url: String,
    /// HTTP API Authorization Token（可选）
    #[serde(default)]
    pub http_token: String,
    /// 机器人 QQ 号
    #[serde(default = "default_self_qq_id")]
    pub self_qq_id: i64,
    /// 注意力冷却时间（秒）
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    /// 空闲超时时间（秒）— 超过此时间无消息自动解除关注
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// 回复分段间默认延迟（毫秒）
    #[serde(default = "default_inter_segment_delay_ms")]
    pub inter_segment_delay_ms: u64,
    /// 回复首段前"思考"延迟（毫秒）
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    /// 出站 worker 轮询间隔（秒）
    #[serde(default = "default_outbox_poll_interval_secs")]
    pub outbox_poll_interval_secs: u64,
    /// 出站 worker 每次批处理条数
    #[serde(default = "default_outbox_batch_size")]
    pub outbox_batch_size: u32,
    /// 触发决策用的 LLM 配置（空 = 使用全局 llm）
    #[serde(default)]
    pub trigger_llm: QqBotTriggerLlmConfig,
    /// 回复生成用的 LLM 配置（空 = 使用全局 llm）
    #[serde(default)]
    pub reply_llm: QqBotReplyLlmConfig,
    // ── 画像构建服务配置 ──
    /// 是否启用后台画像构建（默认启用）
    #[serde(default = "default_profile_enabled")]
    pub profile_enabled: bool,
    /// 触发用户画像 LLM 构建的消息数阈值
    #[serde(default = "default_user_profile_threshold")]
    pub user_profile_threshold: u32,
    /// 触发群聊画像 LLM 构建的消息数阈值
    #[serde(default = "default_group_profile_threshold")]
    pub group_profile_threshold: u32,
    /// 画像定时清理间隔（秒），默认 86400（24h）
    #[serde(default = "default_profile_cleanup_interval_secs")]
    pub profile_cleanup_interval_secs: u64,
    /// 画像使用的 LLM 配置（空 = 使用全局 llm）
    #[serde(default)]
    pub profile_llm: QqBotProfileLlmConfig,
    // ── TTS / 语音消息配置 ──
    /// TTS 音频文件输出目录
    #[serde(default = "default_tts_output_dir")]
    pub tts_output_dir: String,
    /// TTS 音频文件对外提供的 URL 前缀
    #[serde(default = "default_tts_public_url_base")]
    pub tts_public_url_base: String,
    // ── 主动行为（ProactiveEvaluator）配置 ──
    /// 主动行为检查间隔（秒），设为 0 表示禁用
    #[serde(default = "default_proactive_check_interval_secs")]
    pub proactive_check_interval_secs: u64,
    /// 主动行为冷却时间（秒），同一群两次主动行为的最小间隔
    #[serde(default = "default_proactive_cooldown_secs")]
    pub proactive_cooldown_secs: u64,
}

impl Default for QqBotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ws_host: default_ws_host(),
            ws_port: default_ws_port(),
            http_base_url: default_http_base_url(),
            http_token: String::new(),
            self_qq_id: default_self_qq_id(),
            cooldown_secs: default_cooldown_secs(),
            idle_timeout_secs: default_idle_timeout_secs(),
            inter_segment_delay_ms: default_inter_segment_delay_ms(),
            initial_delay_ms: default_initial_delay_ms(),
            outbox_poll_interval_secs: default_outbox_poll_interval_secs(),
            outbox_batch_size: default_outbox_batch_size(),
            trigger_llm: QqBotTriggerLlmConfig::default(),
            reply_llm: QqBotReplyLlmConfig::default(),
            profile_enabled: default_profile_enabled(),
            user_profile_threshold: default_user_profile_threshold(),
            group_profile_threshold: default_group_profile_threshold(),
            profile_cleanup_interval_secs: default_profile_cleanup_interval_secs(),
            profile_llm: QqBotProfileLlmConfig::default(),
            tts_output_dir: default_tts_output_dir(),
            tts_public_url_base: default_tts_public_url_base(),
            proactive_check_interval_secs: default_proactive_check_interval_secs(),
            proactive_cooldown_secs: default_proactive_cooldown_secs(),
        }
    }
}

fn default_ws_host() -> String { "0.0.0.0".into() }
fn default_ws_port() -> u16 { 6700 }
fn default_http_base_url() -> String { "http://127.0.0.1:3000".into() }
fn default_self_qq_id() -> i64 { 0 }
fn default_cooldown_secs() -> u64 { 30 }
fn default_idle_timeout_secs() -> u64 { 120 }
fn default_inter_segment_delay_ms() -> u64 { 800 }
fn default_initial_delay_ms() -> u64 { 1500 }
fn default_outbox_poll_interval_secs() -> u64 { 3 }
fn default_outbox_batch_size() -> u32 { 10 }

fn default_profile_enabled() -> bool { true }
fn default_user_profile_threshold() -> u32 { 50 }
fn default_group_profile_threshold() -> u32 { 500 }
fn default_profile_cleanup_interval_secs() -> u64 { 86400 }

// ── Trigger LLM 子配置 ──

#[derive(Debug, Clone, Deserialize)]
pub struct QqBotTriggerLlmConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_trigger_temperature")]
    pub temperature: f64,
    #[serde(default = "default_trigger_top_p")]
    pub top_p: f64,
    #[serde(default = "default_trigger_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for QqBotTriggerLlmConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            chat_model: String::new(),
            api_key: String::new(),
            temperature: default_trigger_temperature(),
            top_p: default_trigger_top_p(),
            timeout_secs: default_trigger_timeout_secs(),
        }
    }
}

fn default_trigger_temperature() -> f64 { 0.3 }
fn default_trigger_top_p() -> f64 { 0.9 }
fn default_trigger_timeout_secs() -> u64 { 10 }

// ── Reply LLM 子配置 ──

#[derive(Debug, Clone, Deserialize)]
pub struct QqBotReplyLlmConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_reply_temperature")]
    pub temperature: f64,
    #[serde(default = "default_reply_top_p")]
    pub top_p: f64,
    #[serde(default = "default_reply_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_reply_max_tokens")]
    pub max_tokens: u32,
}

impl Default for QqBotReplyLlmConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            chat_model: String::new(),
            api_key: String::new(),
            temperature: default_reply_temperature(),
            top_p: default_reply_top_p(),
            timeout_secs: default_reply_timeout_secs(),
            max_tokens: default_reply_max_tokens(),
        }
    }
}

fn default_reply_temperature() -> f64 { 0.7 }
fn default_reply_top_p() -> f64 { 0.9 }
fn default_reply_timeout_secs() -> u64 { 30 }
fn default_reply_max_tokens() -> u32 { 1024 }

// ── Profile LLM 子配置 ──

#[derive(Debug, Clone, Deserialize)]
pub struct QqBotProfileLlmConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_profile_temperature")]
    pub temperature: f64,
    #[serde(default = "default_profile_top_p")]
    pub top_p: f64,
    #[serde(default = "default_profile_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_profile_max_tokens")]
    pub max_tokens: u32,
}

impl Default for QqBotProfileLlmConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            chat_model: String::new(),
            api_key: String::new(),
            temperature: default_profile_temperature(),
            top_p: default_profile_top_p(),
            timeout_secs: default_profile_timeout_secs(),
            max_tokens: default_profile_max_tokens(),
        }
    }
}

fn default_profile_temperature() -> f64 { 0.3 }
fn default_profile_top_p() -> f64 { 0.9 }
fn default_profile_timeout_secs() -> u64 { 30 }
fn default_profile_max_tokens() -> u32 { 2048 }

// ── TTS 文件输出配置 ──

fn default_tts_output_dir() -> String { "./data/tts/".into() }
fn default_tts_public_url_base() -> String { "http://127.0.0.1:3001/tts/".into() }

// ── 主动行为默认值 ──

fn default_proactive_check_interval_secs() -> u64 { 30 }
fn default_proactive_cooldown_secs() -> u64 { 120 }
