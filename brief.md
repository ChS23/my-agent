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
| Telegram | `frankenstein` 0.48 | Bot API 9.5, `client-reqwest` feature, topics |
| LLM | `async-openai` 0.33 | OpenRouter/Groq/любой OpenAI-совместимый, `chat-completion` + `rustls` |
| STT | Groq Whisper API | бесплатное распознавание голосовых, multipart upload |
| HTTP клиент | `frankenstein::reqwest` (re-export) | без отдельного dep |
| Память | `tokio-rusqlite` 0.7 | async обёртка над rusqlite, один connection на фоновом thread |
| Конфиг | `config` 0.15 + TOML | env override из коробки |
| Сериализация | `serde` + `serde_json` | |
| Логирование | `tracing` + `tracing-subscriber` | structured logs |
| Ошибки | `anyhow` (app) + `thiserror` (lib) | |

---

## Архитектура

```
Telegram (frankenstein long poll + editMessageText streaming)
      │
      ├── Voice? → Groq Whisper STT → text
      │
      ▼
  Message Router (allowlist check, thread_id extraction)
      │
      ├──► Scheduler (Phase 2) ──► inject как сообщение
      │
      ▼
  Agent Core (always-streaming)
      ├── Context Builder (identity + core memories + last N messages per thread)
      ├── Tool Dispatcher (loop до max_tool_iterations)
      │       ├── Memory (store/forget)
      │       ├── Forum Topics (create/rename/close/reopen/delete)
      │       ├── Schedule (cron/at/every)
      │       ├── WebSearch (DuckDuckGo)
      │       ├── TickTick (OAuth2 + CRUD tasks)
      │       └── ... расширяемо через tool_specs + execute_tool
      └── LLM Provider (configurable api_base: OpenRouter, Groq, etc.)
            │
            ▼
      Stream deltas ──► editMessageText (throttled) ──► final edit с HTML
```

### Ключевые принципы

- **Стриминг через `editMessageText`** — placeholder "⏳" → throttled edits с cursor "▍" → final edit с `ParseMode::Html`
- **Non-blocking edits** — каждый edit спавнится как отдельная tokio task, не блокирует receive loop
- **Per-thread sessions** — `(chat_id, thread_id)` как ключ сессии в SQLite
- **Tool dispatch в цикле** — лимит `max_tool_iterations` защищает от бесконечного цикла
- **Security: allowlist на входе** — проверка username перед обработкой
- **Core memories всегда в контексте** — все факты о пользователе в system prompt
- **LLM управляет памятью** — через tool calls `memory_store`/`memory_forget`
- **LLM выводит Telegram HTML напрямую** — без markdown→HTML конвертации
- **Prompt = конкатенация файлов** — `SOUL.md` + `IDENTITY.md` + `FORMAT.md`, загружаются при старте
- **Configurable LLM provider** — `api_base` в конфиге, поддержка любого OpenAI-совместимого API
- **Graceful shutdown** — tokio signal handler

---

## Структура проекта

```
src/
├── main.rs                # точка входа, инициализация, prompt loading, graceful shutdown
├── config.rs              # Config struct: agent, llm, stt, telegram, memory
├── error.rs               # thiserror типы ошибок
│
├── agent/
│   ├── mod.rs
│   └── core.rs            # Agent: always-streaming, tool call assembly из stream chunks
│
├── channels/
│   ├── mod.rs
│   ├── telegram.rs        # long poll, editMessageText streaming, voice transcription
│   └── format.rs          # md_to_telegram_html (unused, kept as fallback)
│
├── tools/
│   ├── mod.rs             # ToolContext + tool_specs() + execute_tool()
│   ├── memory.rs          # memory_store / memory_forget
│   ├── topics.rs          # create/rename/close/reopen/delete forum topics
│   ├── web_search.rs      # DuckDuckGo HTML scraping
│   ├── schedule.rs        # schedule_add / schedule_list / schedule_cancel
│   ├── model.rs           # set_model / get_model (hot reload)
│   └── ticktick.rs        # TickTick auth/create/list/complete/delete
│
├── scheduler/
│   ├── mod.rs             # poll loop, job execution
│   └── store.rs           # SQLite: scheduled_jobs (cron/at/every)
│
├── ticktick/
│   ├── mod.rs
│   ├── client.rs          # TickTick REST API client
│   └── oauth.rs           # OAuth2 flow + token storage
│
├── memory/
│   ├── mod.rs
│   └── store.rs           # SQLite: core_memories + messages (per-thread)
│
└── llm/
    ├── mod.rs
    ├── openrouter.rs       # LlmClient: configurable api_base, streaming, tool calls
    └── stt.rs              # SttClient: Groq Whisper multipart upload (in-memory)

SOUL.md                    # личность, язык, тон
IDENTITY.md                # роль, правила поведения
FORMAT.md                  # Telegram HTML formatting rules
config.toml                # конфиг без секретов
.env                       # секреты, в .gitignore
Dockerfile                 # multi-stage build
compose.yml                # docker compose
Cargo.toml
```

---

## Конфиг

```toml
# config.toml
[agent]
max_tool_iterations = 10
max_history_messages = 50
prompt_files = ["SOUL.md", "IDENTITY.md", "FORMAT.md"]

[llm]
api_base = "https://openrouter.ai/api/v1"  # любой OpenAI-совместимый
model = "google/gemini-3.1-flash-lite-preview"
temperature = 0.7
max_tokens = 4096

[stt]
# api_base = "https://api.groq.com/openai/v1"  (default)
# model = "whisper-large-v3"                     (default)

[telegram]
allowed_users = ["username"]
stream_throttle_ms = 300

[scheduler]
enabled = true
poll_interval_secs = 15

[memory]
db_path = "data/agent.db"
```

```bash
# .env
LLM_API_KEY=sk-...          # или OPENROUTER_API_KEY (fallback)
TELEGRAM_BOT_TOKEN=...
GROQ_API_KEY=...             # optional, для STT
TICKTICK_CLIENT_ID=...       # optional, для TickTick
TICKTICK_CLIENT_SECRET=...   # optional, для TickTick
RUST_LOG=info,agent=debug
```

---

## Фазы реализации

**Phase 1 — Core ✅**
- [x] frankenstein long poll + allowed_users
- [x] Configurable LLM provider (api_base в конфиге)
- [x] Always-streaming через async-openai + tool call assembly
- [x] editMessageText стриминг (placeholder → throttled edits → final HTML)
- [x] SQLite: core_memories + messages (per-thread sessions)
- [x] memory_store / memory_forget tools
- [x] Forum topic tools (create/rename/close/reopen/delete)
- [x] Voice messages → Groq Whisper STT (in-memory, без диска)
- [x] Prompt architecture: SOUL.md + IDENTITY.md + FORMAT.md
- [x] LLM выводит Telegram HTML напрямую
- [x] Context builder: identity + core memories + last N messages
- [x] Graceful shutdown
- [x] Structured logging (tracing)

**Phase 2 — Scheduler + Web Search ✅**
- [x] Web search tool (DuckDuckGo HTML scraping + redirect URL decoding)
- [x] Schedule tools: schedule_add / schedule_list / schedule_cancel
- [x] 3 типа расписаний: cron (с timezone), at (one-shot), every (interval)
- [x] Scheduler poll loop: inject → agent → send response в Telegram
- [x] One-shot auto-cleanup, валидация cron/timezone

**Phase 3 — TickTick + Enhancements ✅**
- [x] TickTick OAuth2 flow + refresh tokens в SQLite
- [x] TickTick CRUD: create/list/complete/delete tasks
- [x] Auto-naming forum topics (emoji + short title после первого обмена)
- [x] Configurable timezone (chrono_tz, Moscow time в system prompt)

**Phase 4 — Docker ✅**
- [x] Multi-stage Docker build (rust:1-alpine3.22 → alpine:3.22)
- [x] compose.yml с volumes для data/config/prompts
- [x] .dockerignore

**Phase 5 — Polish ✅**
- [x] FTS5 full-text search (core_memories + messages, auto-synced triggers)
- [x] Embeddings semantic search (OpenRouter, cosine similarity, hybrid with FTS5)
- [x] `memory_search` tool (hybrid: FTS5 + semantic, scope: memories/messages/all)
- [x] LLM-managed memory extraction (Mem0 паттерн, background after each exchange)
- [x] Config валидация при старте (timezone, prompt files, limits, model)
- [x] Hot reload модели (`set_model` / `get_model` tools, RwLock swap)
- [x] `memory_export` tool (snapshot all memories grouped by category)
