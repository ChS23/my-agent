# my-agent

Personal AI agent living in Telegram. Built with Rust.

## What it does

- Streams responses via native Telegram draft bubble (Bot API 9.5)
- Remembers facts about you across sessions (SQLite + embeddings)
- Manages tasks via TickTick (OAuth2)
- Schedules reminders (cron, one-shot, interval)
- Manages forum topics
- Reads URLs, searches the web, analyzes photos
- Extensible via markdown skills (agent can write its own)

## Stack

| Component | Library |
|-----------|---------|
| Runtime | `tokio` |
| Telegram | `frankenstein` (Bot API 9.5) |
| LLM | `async-openai` via OpenRouter/Groq/any OpenAI-compatible |
| STT | Groq Whisper |
| Storage | `tokio-rusqlite` (SQLite) |
| Config | `config` + TOML |

## Setup

```bash
cp .env.example .env
# fill in secrets
```

```env
LLM_API_KEY=sk-...
TELEGRAM_BOT_TOKEN=...
TELEGRAM_ALLOWED_USERS=your_username
GROQ_API_KEY=...              # optional, for voice
TICKTICK_CLIENT_ID=...        # optional
TICKTICK_CLIENT_SECRET=...    # optional
```

## Run

**Local:**
```bash
cargo run
```

**Docker (dev):**
```bash
docker compose up -d --build
```

**Docker (prod):**
```bash
docker compose -f compose.prod.yml up -d --build
```

## Project structure

```
src/
  main.rs              # entrypoint, init, shutdown
  config.rs            # config structs + validation
  agent/core.rs        # LLM loop, streaming, tool dispatch, memory extraction
  channels/telegram.rs # polling, draft streaming, buttons, voice, vision
  tools/               # memory, ticktick, schedule, topics, dice, skills, web, url
  scheduler/           # cron/at/every jobs
  ticktick/            # OAuth2 + REST client
  memory/store.rs      # SQLite: memories + messages + FTS5
  skills.rs            # markdown skill loader
  llm/                 # LLM client, embeddings, STT

skills/               # markdown skills (YAML frontmatter)
SOUL.md               # personality, language
IDENTITY.md           # role, rules
FORMAT.md             # Telegram HTML formatting
config.toml           # config (no secrets)
```

## License

Private.
