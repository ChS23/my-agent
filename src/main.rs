mod agent;
mod channels;
mod config;
mod error;
mod llm;
mod memory;
mod observability;
mod scheduler;
mod skills;
mod ticktick;
mod tools;

use std::sync::Arc;

use anyhow::Result;
use opentelemetry::trace::TracerProvider;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env
    dotenvy::dotenv().ok();

    // Init Langfuse/OTel (before tracing subscriber so we can add the layer)
    let langfuse_provider = observability::init_langfuse()?;

    // Init tracing (with optional OTel layer for Langfuse)
    let env_filter = EnvFilter::try_from_env("RUST_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,agent=debug"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .compact();

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer);

    if let Some(ref provider) = langfuse_provider {
        let tracer = provider.tracer("my-agent");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
        registry.with(otel_layer).init();
    } else {
        registry.init();
    }

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

    // Load skills (descriptions are injected via use_skill tool spec)
    let skills_list = skills::load_skills(std::path::Path::new("skills"))?;

    // Init memory store
    let memory = memory::MemoryStore::new(&cfg.memory.db_path).await?;
    tracing::info!(db = %cfg.memory.db_path, "memory store initialized");

    // Init scheduler store (separate DB to avoid SQLite lock conflicts)
    let sched_db = cfg.memory.db_path.replace(".db", "_sched.db");
    let sched = scheduler::Scheduler::new(&sched_db, cfg.scheduler.clone()).await?;
    let schedule_store = sched.store().clone();
    tracing::info!(db = %sched_db, "scheduler store initialized");

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

    // Init embedding client (optional)
    let embeddings = if cfg.embeddings.enabled {
        let emb = llm::EmbeddingClient::new(&api_key, &cfg.llm.api_base, &cfg.embeddings.model);
        tracing::info!(model = %cfg.embeddings.model, "embeddings enabled");
        Some(emb)
    } else {
        tracing::info!("embeddings disabled");
        None
    };

    // Init TickTick (optional)
    let ticktick_client = match (
        std::env::var("TICKTICK_CLIENT_ID"),
        std::env::var("TICKTICK_CLIENT_SECRET"),
    ) {
        (Ok(client_id), Ok(client_secret)) if !client_id.is_empty() => {
            let tt_db = cfg.memory.db_path.replace(".db", "_oauth.db");
            let token_store =
                ticktick::TokenStore::new(&tt_db, client_id, client_secret).await?;
            let client = ticktick::TickTickClient::new(token_store);
            tracing::info!("TickTick enabled");
            Some(client)
        }
        _ => {
            tracing::info!("TickTick disabled (no TICKTICK_CLIENT_ID/SECRET)");
            None
        }
    };

    // Bot token (needed for skill commands and later for bot init)
    let bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
        .map_err(|_| anyhow::anyhow!("TELEGRAM_BOT_TOKEN not set"))?;

    // Register skill commands in Telegram bot menu
    {
        use frankenstein::client_reqwest::Bot;
        use frankenstein::methods::SetMyCommandsParams;
        use frankenstein::types::BotCommand;
        use frankenstein::AsyncTelegramApi;

        let commands: Vec<BotCommand> = skills_list
            .iter()
            .map(|s| {
                let desc = if s.description.len() > 256 {
                    format!("{}…", &s.description[..255])
                } else {
                    s.description.clone()
                };
                BotCommand::builder().command(&s.name).description(desc).build()
            })
            .collect();

        if !commands.is_empty() {
            let bot = Bot::new(&bot_token);
            let params = SetMyCommandsParams::builder().commands(commands.clone()).build();
            match bot.set_my_commands(&params).await {
                Ok(_) => tracing::info!(count = commands.len(), "bot commands registered"),
                Err(e) => tracing::warn!(error = %e, "failed to register bot commands"),
            }
        }
    }

    // Init agent
    let agent = Arc::new(agent::Agent::new(
        llm,
        embeddings,
        memory,
        schedule_store,
        ticktick_client,
        identity,
        cfg.agent.clone(),
        skills_list,
    ));

    // Init STT client (optional — for voice messages)
    let stt = std::env::var("GROQ_API_KEY").ok().map(|key| {
        tracing::info!(model = %cfg.stt.model, "STT enabled (Groq)");
        llm::SttClient::new(&key, &cfg.stt.api_base, &cfg.stt.model)
    });

    // Init telegram bot
    let telegram = Arc::new(channels::TelegramBot::new(&bot_token, &cfg.telegram, stt));

    // Graceful shutdown
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let tg_shutdown = shutdown_tx.subscribe();
    let sched_shutdown = shutdown_tx.subscribe();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("shutdown signal received");
        shutdown_tx.send(()).ok();
    });

    // Start scheduler (if enabled)
    let bot_for_sched = Arc::new(frankenstein::client_reqwest::Bot::new(&bot_token));
    if cfg.scheduler.enabled {
        let agent_clone = Arc::clone(&agent);
        tokio::spawn(async move {
            if let Err(e) = sched.run(agent_clone, bot_for_sched, sched_shutdown).await {
                tracing::error!(error = %e, "scheduler error");
            }
        });
    }

    // Run telegram polling (blocks until shutdown)
    telegram.run(agent, tg_shutdown).await?;

    // Flush Langfuse traces before exit
    if let Some(provider) = langfuse_provider {
        if let Err(e) = provider.shutdown() {
            tracing::warn!(error = %e, "langfuse shutdown error");
        }
    }

    tracing::info!("agent stopped");
    Ok(())
}
