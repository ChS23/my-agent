use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub agent: AgentConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub stt: SttConfig,
    pub telegram: TelegramConfig,
    pub memory: MemoryConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentConfig {
    pub max_tool_iterations: usize,
    pub max_history_messages: usize,
    pub prompt_files: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    #[serde(default = "default_api_base")]
    pub api_base: String,
    pub model: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SttConfig {
    #[serde(default = "default_stt_api_base")]
    pub api_base: String,
    #[serde(default = "default_stt_model")]
    pub model: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelegramConfig {
    pub allowed_users: Vec<String>,
    #[serde(default = "default_stream_throttle")]
    pub stream_throttle_ms: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MemoryConfig {
    pub db_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SchedulerConfig {
    #[serde(default = "default_scheduler_enabled")]
    pub enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: default_scheduler_enabled(),
            poll_interval_secs: default_poll_interval(),
        }
    }
}

fn default_scheduler_enabled() -> bool {
    true
}
fn default_poll_interval() -> u64 {
    15
}

fn default_stt_api_base() -> String {
    "https://api.groq.com/openai/v1".to_string()
}
fn default_stt_model() -> String {
    "whisper-large-v3".to_string()
}
impl Default for SttConfig {
    fn default() -> Self {
        Self {
            api_base: default_stt_api_base(),
            model: default_stt_model(),
        }
    }
}
fn default_api_base() -> String {
    "https://openrouter.ai/api/v1".to_string()
}
fn default_temperature() -> f32 {
    0.7
}
fn default_max_tokens() -> u32 {
    4096
}
fn default_stream_throttle() -> u64 {
    300
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name("config").required(true))
            .add_source(config::Environment::with_prefix("AGENT").separator("__"))
            .build()?;

        let config: Config = settings.try_deserialize()?;
        Ok(config)
    }
}
