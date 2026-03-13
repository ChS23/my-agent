# Personal AI Agent — Brief

> Личный Telegram-агент на Rust. Дистилляция лучшего из ZeroClaw, без лишнего.

---

## Суть

Автономный агент который живёт в Telegram, управляет задачами через TickTick, умеет напоминать по расписанию и помнит контекст между сессиями.

**Не строим:** dashboard, multi-user, gateway, composio, browser, OTP.
**Строим:** минимальный, надёжный, расширяемый core.

---

## Стек

| Компонент | Библиотека | Почему |
|-----------|-----------|--------|
| Async runtime | `tokio` 1 | стандарт |
| Telegram | `frankenstein` 0.48 | Bot API 9.5, `sendMessageDraft` для нативного стриминга, topics в приватных чатах |
| LLM | `async-openai` 0.33 | OpenRouter совместим, granular features (`chat-completion` + `rustls`) |
| HTTP клиент | `reqwest` 0.13 (rustls) | TickTick API + web search, без OpenSSL |
| Scheduler | `cron` 0.15 + свой код | паттерн из ZeroClaw: cron + SQLite + tokio poll loop |
| Память | `tokio-rusqlite` 0.7 | async обёртка над rusqlite, один connection на фоновом thread |
| Конфиг | `config` 0.15 + TOML | env override из коробки |
| Сериализация | `serde` + `serde_json` | |
| Логирование | `tracing` + `tracing-subscriber` | structured logs, как в ZeroClaw |
| Ошибки | `anyhow` (app) + `thiserror` (lib) | разделение как в ZeroClaw |
| HTML парсинг | `scraper` 0.25 | DuckDuckGo results |

---

## Архитектура

```
Telegram (frankenstein long poll + sendMessageDraft streaming)
      │
      ▼
  Message Router (allowlist check)
      │
      ├──► Scheduler (cron + SQLite) ──► inject как сообщение с from="system"
      │
      ▼
  Agent Core
      ├── Context Builder (core memories + last N messages из SQLite)
      ├── Tool Dispatcher (loop до max_tool_iterations)
      │       ├── TickTick (reqwest REST)
      │       ├── Schedule (add/list/cancel)
      │       ├── WebSearch (DuckDuckGo scraping)
      │       ├── Memory (store/recall/forget)
      │       └── ... расширяемо через Tool trait
      └── LLM Provider (OpenRouter, streaming)
            │
            ▼
      Response ──► sendMessageDraft (стриминг) ──► sendMessage (финал)
```

### Ключевые принципы

- **Scheduler инжектирует как обычное сообщение** — один путь через Agent Core, не два
- **Tool dispatch в цикле** — лимит `max_tool_iterations` защищает от бесконечного цикла
- **Security: allowlist на входе** — проверка username перед обработкой
- **Core memories всегда в контексте** — все факты о пользователе в system prompt
- **LLM управляет памятью** — через tool calls `memory_store`/`memory_forget`
- **Стриминг через `sendMessageDraft`** — нативный Bot API 9.5, без edit loop
- **Graceful shutdown** — tokio signal handler, дожидаемся in-flight сообщений

---

## Память — трёхфазная архитектура

### Phase 1: Core Memory в контексте (текущая)

Все факты загружаются в system prompt (~1500 токенов при 50 фактах).
LLM сам решает что запоминать через tool calls.

```sql
-- Факты о пользователе
CREATE TABLE core_memories (
    id TEXT PRIMARY KEY,
    key TEXT UNIQUE NOT NULL,
    content TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'core',  -- core | preference | decision
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- История диалогов
CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id INTEGER NOT NULL,
    role TEXT NOT NULL,       -- user | assistant | system
    content TEXT NOT NULL,
    timestamp TEXT NOT NULL
);

CREATE INDEX idx_messages_chat ON messages(chat_id, timestamp);
```

**Tools для LLM:**
- `memory_store(key, content, category)` — сохранить/обновить факт
- `memory_forget(key)` — удалить факт

### Phase 2: Hybrid Search (когда фактов > 100)

- FTS5 keyword search в SQLite (бесплатно)
- Embeddings как BLOB (OpenRouter: `qwen/qwen3-embedding-8b`, $0.01/1M токенов)
- Cosine similarity в Rust (порт из ZeroClaw, ~130 строк)
- Selective recall: top-5 по релевантности вместо загрузки всего

### Phase 3: LLM-Managed Memory (Mem0 паттерн)

- После каждого диалога — дешёвый LLM call для извлечения фактов
- Conflict resolution: UPDATE/REPLACE/SKIP
- Memory decay: архивация неиспользуемых фактов
- MEMORY_SNAPSHOT.md — human-readable бэкап "души" агента

---

## Структура проекта

```
src/
├── main.rs                # точка входа, инициализация, graceful shutdown
├── config.rs              # Config struct, валидация при старте
├── error.rs               # thiserror типы ошибок
│
├── agent/
│   ├── mod.rs
│   └── core.rs            # Agent struct, process_message(), streaming
│
├── channels/
│   ├── mod.rs
│   └── telegram.rs        # frankenstein: long poll, sendMessageDraft, message routing
│
├── tools/
│   ├── mod.rs             # Tool trait + Registry
│   ├── memory.rs          # memory_store / memory_forget
│   ├── ticktick.rs        # TickTick REST API (Phase 3)
│   ├── schedule.rs        # add/list/cancel (Phase 2)
│   └── web_search.rs      # DuckDuckGo scraping (Phase 2)
│
├── scheduler/
│   └── mod.rs             # cron + SQLite jobs, inject в agent (Phase 2)
│
├── memory/
│   └── store.rs           # SQLite: core_memories + messages, context builder
│
└── llm/
    └── openrouter.rs      # async-openai, OpenRouter config, streaming

config.toml                # конфиг без секретов
.env                       # секреты, в .gitignore
Cargo.toml
Dockerfile
compose.yml
IDENTITY.md                # системный промпт / личность агента
```

---

## Tool trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn execute(&self, params: serde_json::Value) -> anyhow::Result<ToolResult>;
}

pub struct ToolResult {
    pub success: bool,
    pub output: String,
}
```

Новый инструмент = реализуй trait, зарегистрируй в Registry. Больше ничего.

---

## Стриминг в Telegram

```
1. Пользователь пишет сообщение
2. Agent начинает streaming от OpenRouter
3. Каждые ~300 мс → sendMessageDraft(chat_id, accumulated_text)
   - Telegram показывает "draft" bubble с растущим текстом
4. Когда стрим закончен → sendMessage(chat_id, final_text, parse_mode=MarkdownV2)
   - Draft исчезает, финальное сообщение с форматированием
```

---

## TickTick — OAuth2 + Refresh flow (Phase 3)

Токены живут в SQLite, не в env. Env хранит только `client_id` и `client_secret`.

```
Первый запуск:
  Authorization URL → пользователь в браузере → code
  → POST /oauth/token → access_token + refresh_token
  → сохранить в SQLite с expires_at

Каждый запрос:
  if now() > expires_at - 5 min:
      POST /oauth/token (grant_type=refresh_token)
      → обновить в SQLite
  → использовать access_token
```

---

## Error handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LLM provider error: {0}")]
    Provider(String),
    #[error("Tool execution failed: {tool} — {reason}")]
    Tool { tool: String, reason: String },
    #[error("TickTick API error: {status} — {message}")]
    TickTick { status: u16, message: String },
    #[error("Telegram error: {0}")]
    Telegram(String),
    #[error("Config error: {0}")]
    Config(String),
    #[error("Database error: {0}")]
    Database(String),
}

// Ошибки tool call не роняют агента — возвращают error string в LLM
// LLM видит ошибку и адаптируется
```

---

## Конфиг

```toml
# config.toml — в репе, без секретов
[agent]
max_tool_iterations = 10
max_history_messages = 50
identity_path = "IDENTITY.md"

[llm]
model = "google/gemini-2.0-flash"
temperature = 0.7
max_tokens = 4096

[telegram]
allowed_users = ["your_username"]
stream_throttle_ms = 300

[memory]
db_path = "data/agent.db"

[scheduler]
enabled = true
poll_interval_secs = 15

[web_search]
max_results = 5
timeout_secs = 10
```

```bash
# .env — никогда в репе
OPENROUTER_API_KEY=sk-...
TELEGRAM_BOT_TOKEN=...
TICKTICK_CLIENT_ID=...
TICKTICK_CLIENT_SECRET=...
RUST_LOG=info,agent=debug
```

---

## Cargo.toml

```toml
[package]
name = "agent"
version = "0.1.0"
edition = "2021"

[dependencies]
# Async
tokio = { version = "1", features = ["full"] }
futures = "0.3"

# Telegram (Bot API 9.5)
frankenstein = { version = "0.48", features = ["async-http-client"] }

# LLM (OpenRouter-compatible)
async-openai = { version = "0.33", default-features = false, features = ["chat-completion", "rustls"] }

# HTTP
reqwest = { version = "0.13", features = ["json", "rustls-tls"], default-features = false }

# Database (async SQLite)
tokio-rusqlite = "0.7"
rusqlite = { version = "0.38", features = ["bundled"] }

# Scheduler (Phase 2)
# cron = "0.15"

# Config
config = { version = "0.15", features = ["toml"] }
dotenvy = "0.15"

# Serde
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Errors
anyhow = "1"
thiserror = "2"

# Utils
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

---

## Dockerfile (Phase 4)

```dockerfile
FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM alpine:3.21
RUN apk add --no-cache ca-certificates tzdata
WORKDIR /app
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/agent .
COPY config.toml IDENTITY.md ./
CMD ["./agent"]
```

## compose.yml (Phase 4)

```yaml
services:
  agent:
    build: .
    container_name: ai-agent
    restart: unless-stopped
    volumes:
      - ./data:/app/data
      - ./config.toml:/app/config.toml
      - ./IDENTITY.md:/app/IDENTITY.md
    env_file: .env
    environment:
      - RUST_LOG=info,agent=debug
```

---

## Фазы реализации

**Phase 1 — Core**
- [ ] frankenstein long poll + allowed_users
- [ ] OpenRouter streaming через async-openai
- [ ] sendMessageDraft стриминг в Telegram
- [ ] SQLite: core_memories + messages
- [ ] memory_store / memory_forget tools
- [ ] IDENTITY.md как системный промпт
- [ ] Context builder: identity + core memories + last N messages
- [ ] Graceful shutdown
- [ ] Structured logging (tracing)

**Phase 2 — Tools + Scheduler**
- [ ] Tool trait + Registry
- [ ] Tool dispatch loop (max_tool_iterations)
- [ ] Schedule tool (cron + SQLite, паттерн ZeroClaw)
- [ ] Web search tool (DuckDuckGo scraping)

**Phase 3 — TickTick + Smart Memory**
- [ ] OAuth2 flow + refresh tokens в SQLite
- [ ] TickTick CRUD tasks tool
- [ ] FTS5 + embeddings hybrid search
- [ ] LLM-managed memory extraction (Mem0 паттерн)

**Phase 4 — Polish**
- [ ] Config валидация при старте (fail fast)
- [ ] Hot reload модели без рестарта
- [ ] Multi-stage Docker build
- [ ] MEMORY_SNAPSHOT.md export/hydration
