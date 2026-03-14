# Multi-Tenant Architecture Plan

## Overview

Full user isolation with PostgreSQL RLS, pgvector embeddings, per-user prompts/skills.
One bot instance serves all users. UserContext resolved per-request.

## Database Schema (PostgreSQL + pgvector + RLS)

### Extensions

```sql
CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS vector;
```

### users

```sql
CREATE TABLE users (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username    TEXT UNIQUE NOT NULL,  -- Telegram username, tenant key
    role        TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    timezone    TEXT NOT NULL DEFAULT 'Europe/Moscow',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    enabled     BOOLEAN NOT NULL DEFAULT true
);
```

### user_prompts (soul, identity, format -- per user)

```sql
CREATE TABLE user_prompts (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    slot        TEXT NOT NULL CHECK (slot IN ('soul', 'identity', 'format')),
    content     TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, slot)
);
```

### core_memories

```sql
CREATE TABLE core_memories (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    content     TEXT NOT NULL,
    category    TEXT NOT NULL DEFAULT 'core',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    search_vec  TSVECTOR GENERATED ALWAYS AS (to_tsvector('simple', key || ' ' || content)) STORED,
    UNIQUE (user_id, key)
);

CREATE INDEX idx_core_memories_user ON core_memories(user_id);
CREATE INDEX idx_core_memories_fts ON core_memories USING gin(search_vec);
```

### messages

```sql
CREATE TABLE messages (
    id          BIGSERIAL PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chat_id     BIGINT NOT NULL,
    thread_id   INTEGER,
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    search_vec  TSVECTOR GENERATED ALWAYS AS (to_tsvector('simple', content)) STORED
);

CREATE INDEX idx_messages_session ON messages(user_id, chat_id, thread_id, id);
CREATE INDEX idx_messages_fts ON messages USING gin(search_vec);
```

### memory_embeddings (pgvector)

```sql
CREATE TABLE memory_embeddings (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    memory_key  TEXT NOT NULL,
    embedding   vector(4096) NOT NULL,  -- adjust to model dimension
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, memory_key),
    FOREIGN KEY (user_id, memory_key) REFERENCES core_memories(user_id, key) ON DELETE CASCADE
);

CREATE INDEX idx_embeddings_vector ON memory_embeddings
    USING hnsw (embedding vector_cosine_ops);
```

Semantic search query:
```sql
SELECT cm.key, cm.content, cm.category,
       1 - (me.embedding <=> $1::vector) AS similarity
FROM memory_embeddings me
JOIN core_memories cm ON cm.user_id = me.user_id AND cm.key = me.memory_key
WHERE me.user_id = $2
ORDER BY me.embedding <=> $1::vector
LIMIT $3;
```

### scheduled_jobs

```sql
CREATE TABLE scheduled_jobs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    schedule    JSONB NOT NULL,
    prompt      TEXT NOT NULL,
    chat_id     BIGINT NOT NULL,
    thread_id   INTEGER,
    enabled     BOOLEAN NOT NULL DEFAULT true,
    next_run    TIMESTAMPTZ,
    last_run    TIMESTAMPTZ,
    last_status TEXT,
    last_output TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_jobs_due ON scheduled_jobs(next_run) WHERE enabled = true;
CREATE INDEX idx_jobs_user ON scheduled_jobs(user_id);
```

### oauth_tokens

```sql
CREATE TABLE oauth_tokens (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    service         TEXT NOT NULL,
    access_token    TEXT NOT NULL,
    refresh_token   TEXT NOT NULL,
    expires_at      TIMESTAMPTZ NOT NULL,
    UNIQUE (user_id, service)
);
```

### skills

```sql
CREATE TABLE skills (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    trigger     TEXT NOT NULL DEFAULT 'manual' CHECK (trigger IN ('manual', 'auto')),
    content     TEXT NOT NULL,
    enabled     BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);
```

## Row Level Security

```sql
-- Enable RLS on all tenant tables
ALTER TABLE core_memories ENABLE ROW LEVEL SECURITY;
ALTER TABLE messages ENABLE ROW LEVEL SECURITY;
ALTER TABLE memory_embeddings ENABLE ROW LEVEL SECURITY;
ALTER TABLE scheduled_jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE oauth_tokens ENABLE ROW LEVEL SECURITY;
ALTER TABLE skills ENABLE ROW LEVEL SECURITY;
ALTER TABLE user_prompts ENABLE ROW LEVEL SECURITY;

-- App role
CREATE ROLE agent_app LOGIN PASSWORD 'xxx';

-- RLS policy (same for all tables)
-- Rust sets: SET LOCAL app.current_user_id = '<uuid>';
CREATE POLICY tenant_isolation ON core_memories
    FOR ALL TO agent_app
    USING (user_id = current_setting('app.current_user_id')::uuid)
    WITH CHECK (user_id = current_setting('app.current_user_id')::uuid);

-- (repeat for all tables)

GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO agent_app;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO agent_app;
```

## Rust Architecture

### New Dependencies

```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "tls-rustls", "postgres", "uuid", "chrono", "json"] }
pgvector = { version = "0.4", features = ["sqlx"] }
```

Removes: `tokio-rusqlite`

### UserContext (per-request)

```rust
pub struct UserContext {
    pub user_id: Uuid,
    pub username: String,
    pub role: UserRole,
    pub timezone: String,
    pub identity: String,       // concatenated soul + identity + format
    pub precompact: String,     // extracted from identity
    pub skills: Vec<Skill>,
}
```

### UserResolver (cached)

```rust
pub struct UserResolver {
    pool: PgPool,
    cache: RwLock<HashMap<String, CachedUser>>,
}
// TTL: 5 min. invalidate() on prompt/skill edit.
```

### Agent (shared infrastructure)

```rust
pub struct Agent {
    llm: LlmClient,
    embeddings: Option<EmbeddingClient>,
    pool: PgPool,
    user_resolver: UserResolver,
    config: AgentConfig,  // only non-user-specific parts
}
```

### Tenant-scoped transactions

```rust
impl MemoryStore {
    async fn with_tenant<F, T>(&self, user_id: Uuid, f: F) -> Result<T> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL app.current_user_id = $1")
            .bind(user_id.to_string())
            .execute(&mut *tx)
            .await?;
        let result = f(&mut tx).await?;
        tx.commit().await?;
        Ok(result)
    }
}
```

### Module layout

```
src/
  agent/core.rs       -- Agent struct, process_message (takes UserContext)
  channels/telegram.rs -- UserResolver instead of allowed_users
  config.rs           -- remove prompt_files, allowed_users; add database_url
  db/
    mod.rs            -- pool init, migration runner
    migrations/       -- sqlx migrations
  llm/                -- unchanged
  memory/store.rs     -- sqlx + PgPool + user_id on every method
  scheduler/store.rs  -- sqlx + user_id
  skills.rs           -- DB load/save + file-based seeding
  ticktick/oauth.rs   -- per-user token resolution
  tools/              -- ToolContext gains user: &UserContext
  user.rs             -- UserContext, UserResolver, UserRole
```

### ToolContext

```rust
pub struct ToolContext<'a> {
    pub store: &'a MemoryStore,
    pub pool: &'a PgPool,
    pub bot: &'a Bot,
    pub chat_id: i64,
    pub thread_id: Option<i32>,
    pub user: &'a UserContext,
    pub llm: &'a LlmClient,
    pub embeddings: Option<&'a EmbeddingClient>,
    pub ticktick: Option<TickTickClient>,
    pub skills: &'a [Skill],
}
```

## Config Changes

```toml
[agent]
max_tool_iterations = 10
max_history_messages = 50
# prompt_files REMOVED (per-user in DB)
# timezone REMOVED (per-user in DB)

[database]
url = "postgresql://agent_app:xxx@localhost:5432/agent"
max_connections = 20

[telegram]
stream_throttle_ms = 300
# allowed_users REMOVED (users table)
```

## User Management

### Phase 1: Seed file
`users.toml` or env var with initial usernames. Upserted on startup.

### Phase 2: Admin bot command
`/add_user @username` -- only for role=admin users.

### Phase 3: Self-registration (optional)
Unknown users get created with `enabled=false`, admin notified.

### Authorization flow
```rust
let user = match user_resolver.resolve(username).await? {
    Some(u) if u.enabled => u,
    Some(_) => return,  // disabled
    None => return,     // unknown
};
```

## User Seeding

On new user creation, seed defaults from disk:
- `SOUL.md`, `IDENTITY.md`, `FORMAT.md` -> `user_prompts`
- `skills/` directory -> `skills` table

## Scheduler in Multi-Tenant

`get_due_jobs()` must bypass RLS (superuser or BYPASSRLS role) to find all due jobs.
When executing, resolve UserContext for that job's user_id.

## OAuth in Multi-Tenant

Replace localhost callback with persistent axum server:
`GET /oauth/ticktick/callback?code=X&state=USER_ID`
Works behind reverse proxy in production.

## Migration Strategy (SQLite -> PostgreSQL)

1. Add `sqlx` alongside `tokio-rusqlite`
2. Write PostgreSQL migrations in `migrations/`
3. Write `src/bin/migrate_to_pg.rs` binary
4. Run migration tool (maps existing data to admin user UUID)
5. Switch all reads/writes to PostgreSQL
6. Remove `tokio-rusqlite`

## Implementation Steps

1. PostgreSQL schema + migrations + Docker Compose with PG + pgvector
2. `src/db/mod.rs` (pool init) + `src/user.rs` (UserContext, UserResolver)
3. Rewrite `MemoryStore` for sqlx + user_id
4. Rewrite `ScheduleStore` for sqlx + user_id
5. Rewrite `TokenStore` (TickTick) + persistent callback server
6. Add `SkillStore` (DB load/save)
7. Update `Agent` struct (remove per-user fields, add pool + user_resolver)
8. Update `process_message` to resolve UserContext first
9. Update `ToolContext` and all tools to accept user_id
10. Update `telegram.rs` (UserResolver instead of allowed_users)
11. Write SQLite-to-PostgreSQL migration tool
12. Remove `tokio-rusqlite`

## Risks

- **RLS misconfiguration**: Integration tests with two users, verify isolation
- **SET LOCAL forgotten**: `with_tenant()` wrapper makes it structurally impossible
- **pgvector performance**: HNSW index + per-user filter, fine for <1000 memories/user
- **Cache staleness**: 5-min TTL + `invalidate()` on edits
- **Migration data loss**: Keep SQLite as backup, test on staging PG first
