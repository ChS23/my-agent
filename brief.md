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
      │       ├── Schedule (Phase 2)
      │       ├── WebSearch (Phase 2)
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
│   └── topics.rs          # create/rename/close/reopen/delete forum topics
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

[memory]
db_path = "data/agent.db"
```

```bash
# .env
LLM_API_KEY=sk-...          # или OPENROUTER_API_KEY (fallback)
TELEGRAM_BOT_TOKEN=...
GROQ_API_KEY=...             # optional, для STT
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

**Phase 2 — Scheduler + Web Search**
- [ ] Schedule tool (cron + SQLite, паттерн ZeroClaw)
- [ ] Web search tool (DuckDuckGo scraping)
- [ ] Scheduler inject: cron jobs → agent messages

**Phase 3 — TickTick + Smart Memory**
- [ ] OAuth2 flow + refresh tokens в SQLite
- [ ] TickTick CRUD tasks tool
- [ ] FTS5 + embeddings hybrid search
- [ ] LLM-managed memory extraction (Mem0 паттерн)

**Phase 4 — Polish**
- [ ] Multi-stage Docker build
- [ ] Config валидация при старте (fail fast)
- [ ] Hot reload модели без рестарта
- [ ] MEMORY_SNAPSHOT.md export/hydration
