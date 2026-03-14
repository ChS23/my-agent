#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LLM provider error: {0}")]
    Provider(String),

    #[error("Tool execution failed: {tool} — {reason}")]
    Tool { tool: String, reason: String },

    #[error("Telegram error: {0}")]
    Telegram(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Database error: {0}")]
    Database(String),
}
