# Changelog

## Phase 6 — Skills + Streaming + Polish

- `sendMessageDraft` streaming (Bot API 9.5, native draft bubble)
- Inline keyboard buttons from LLM response (`buttons` blocks)
- Strip buttons JSON from draft stream
- Vision — photo analysis (base64 multimodal)
- URL reader tool
- Callback query handling (inline button clicks)
- Skills system (markdown + YAML frontmatter, lazy loading)
- `write_skill` / `restart_agent` tools — agent writes its own skills
- `send_dice` tool (animated Telegram dice)
- Bot commands menu auto-registered from skills
- PreCompact pattern — critical prompt sections survive history compression
- `ticktick_callback` tool — OAuth via chat (paste redirect URL)
- `allowed_users` moved to env (`TELEGRAM_ALLOWED_USERS`)
- `compose.prod.yml` with Docker named volumes
- System prompt: grouped memories by category, day of week, thread context

## Phase 5 — Polish

- FTS5 full-text search (memories + messages)
- Embeddings semantic search (OpenRouter, cosine similarity, hybrid with FTS5)
- `memory_search` tool (hybrid: FTS5 + semantic)
- LLM-managed memory extraction (Mem0 pattern, background)
- Config validation at startup
- Hot reload model (`set_model` / `get_model`)
- `memory_export` tool

## Phase 4 — Docker

- Multi-stage Docker build (rust:alpine)
- `compose.yml` with volumes
- `.dockerignore`

## Phase 3 — TickTick + Enhancements

- TickTick OAuth2 flow + refresh tokens in SQLite
- TickTick CRUD: create/list/complete/delete tasks
- Auto-naming forum topics (emoji + short title)
- Configurable timezone

## Phase 2 — Scheduler + Web Search

- Web search (DuckDuckGo HTML scraping)
- Schedule tools: add/list/cancel
- 3 schedule types: cron, at (one-shot), every (interval)
- Scheduler poll loop
- One-shot auto-cleanup

## Phase 1 — Core

- Frankenstein long poll + allowlist
- Configurable LLM provider (api_base)
- Always-streaming via async-openai + tool call assembly
- SQLite: core_memories + messages (per-thread)
- memory_store / memory_forget tools
- Forum topic tools
- Voice messages via Groq Whisper STT
- Prompt architecture: SOUL.md + IDENTITY.md + FORMAT.md
- Telegram HTML output
- Graceful shutdown
- Structured logging (tracing)
