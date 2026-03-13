use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub agent: AgentConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub stt: SttConfig,
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub embeddings: EmbeddingsConfig,
    pub memory: MemoryConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentConfig {
    pub max_tool_iterations: usize,
    pub max_history_messages: usize,
    pub prompt_files: Vec<String>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
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

#[derive(Debug, Deserialize, Clone)]
pub struct EmbeddingsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_embedding_model")]
    pub model: String,
}

impl Default for EmbeddingsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: default_embedding_model(),
        }
    }
}

fn default_embedding_model() -> String {
    "qwen/qwen3-embedding-8b".to_string()
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
fn default_timezone() -> String {
    "Europe/Moscow".to_string()
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
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        // Timezone
        self.agent
            .timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| anyhow::anyhow!("invalid timezone: {}", self.agent.timezone))?;

        // Prompt files exist
        for path in &self.agent.prompt_files {
            if !std::path::Path::new(path).exists() {
                anyhow::bail!("prompt file not found: {path}");
            }
        }

        // Limits
        if self.agent.max_tool_iterations == 0 {
            anyhow::bail!("max_tool_iterations must be > 0");
        }
        if self.agent.max_history_messages == 0 {
            anyhow::bail!("max_history_messages must be > 0");
        }

        // LLM
        if self.llm.model.is_empty() {
            anyhow::bail!("llm.model is required");
        }
        if self.llm.api_base.is_empty() {
            anyhow::bail!("llm.api_base is required");
        }

        // Telegram
        if self.telegram.allowed_users.is_empty() {
            anyhow::bail!("telegram.allowed_users must not be empty");
        }

        // DB path parent
        if let Some(parent) = std::path::Path::new(&self.memory.db_path).parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                tracing::warn!(path = %parent.display(), "db parent dir doesn't exist, will be created");
            }
        }

        Ok(())
    }
}
