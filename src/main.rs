mod agent;
mod channels;
mod config;
mod error;
mod llm;
mod memory;
mod tools;

use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env
    dotenvy::dotenv().ok();

    // Init tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("RUST_LOG")
                .unwrap_or_else(|_| EnvFilter::new("info,agent=debug")),
        )
        .with_target(true)
        .compact()
        .init();

    tracing::info!("starting agent");

    // Load config
    let cfg = config::Config::load()?;
    tracing::info!(model = %cfg.llm.model, "config loaded");

    // Load prompt files
    let mut identity = String::new();
    for path in &cfg.agent.prompt_files {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read {path}: {e}"))?;
        identity.push_str(&content);
        identity.push_str("\n\n");
        tracing::info!(path = %path, "prompt loaded");
    }

    // Init memory store
    let memory = memory::MemoryStore::new(&cfg.memory.db_path).await?;
    tracing::info!(db = %cfg.memory.db_path, "memory store initialized");

    // Init LLM client
    let api_key = std::env::var("LLM_API_KEY")
        .or_else(|_| std::env::var("OPENROUTER_API_KEY"))
        .map_err(|_| anyhow::anyhow!("LLM_API_KEY or OPENROUTER_API_KEY not set"))?;
    let llm = llm::LlmClient::new(
        &api_key,
        &cfg.llm.api_base,
        &cfg.llm.model,
        cfg.llm.temperature,
        cfg.llm.max_tokens,
    );

    // Init agent
    let agent = Arc::new(agent::Agent::new(llm, memory, identity, cfg.agent.clone()));

    // Init STT client (optional — for voice messages)
    let stt = std::env::var("GROQ_API_KEY").ok().map(|key| {
        tracing::info!(model = %cfg.stt.model, "STT enabled (Groq)");
        llm::SttClient::new(&key, &cfg.stt.api_base, &cfg.stt.model)
    });

    // Init telegram bot
    let bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("TELEGRAM_BOT_TOKEN not set"))?;
    let telegram = Arc::new(channels::TelegramBot::new(&bot_token, &cfg.telegram, stt));

    // Graceful shutdown
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let shutdown_rx = shutdown_tx.subscribe();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Run telegram polling (blocks until shutdown)
    telegram.run(agent, shutdown_rx).await?;

    tracing::info!("agent stopped");
    Ok(())
}
