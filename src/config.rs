use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub agent: AgentConfig,
    pub llm: LlmConfig,
    pub telegram: TelegramConfig,
    pub memory: MemoryConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentConfig {
    pub max_tool_iterations: usize,
    pub max_history_messages: usize,
    pub identity_path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LlmConfig {
    pub model: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
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
