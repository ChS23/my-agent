use anyhow::Result;
use chrono::Utc;
use tokio_rusqlite::Connection;
use tokio_rusqlite::rusqlite;

#[derive(Debug, Clone)]
pub struct CoreMemory {
    pub key: String,
    pub content: String,
    pub category: String,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
}

#[derive(Clone)]
pub struct MemoryStore {
    conn: Connection,
}

impl MemoryStore {
    pub async fn new(db_path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path).await?;

        conn.call(|db| {
            db.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;

                 CREATE TABLE IF NOT EXISTS core_memories (
                     id TEXT PRIMARY KEY,
                     key TEXT UNIQUE NOT NULL,
                     content TEXT NOT NULL,
                     category TEXT NOT NULL DEFAULT 'core',
                     created_at TEXT NOT NULL,
                     updated_at TEXT NOT NULL
                 );

                 CREATE TABLE IF NOT EXISTS messages (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     chat_id INTEGER NOT NULL,
                     thread_id INTEGER,
                     role TEXT NOT NULL,
                     content TEXT NOT NULL,
                     timestamp TEXT NOT NULL
                 );

                 CREATE INDEX IF NOT EXISTS idx_messages_session
                     ON messages(chat_id, thread_id, timestamp);

                 -- Embeddings for semantic search
                 CREATE TABLE IF NOT EXISTS memory_embeddings (
                     key TEXT PRIMARY KEY REFERENCES core_memories(key) ON DELETE CASCADE,
                     embedding BLOB NOT NULL,
                     updated_at TEXT NOT NULL
                 );

                 -- FTS5 for core_memories search
                 CREATE VIRTUAL TABLE IF NOT EXISTS core_memories_fts USING fts5(
                     key, content, category,
                     content=core_memories,
                     content_rowid=rowid
                 );

                 -- Triggers to keep FTS in sync
                 CREATE TRIGGER IF NOT EXISTS core_memories_ai AFTER INSERT ON core_memories BEGIN
                     INSERT INTO core_memories_fts(rowid, key, content, category)
                     VALUES (new.rowid, new.key, new.content, new.category);
                 END;

                 CREATE TRIGGER IF NOT EXISTS core_memories_ad AFTER DELETE ON core_memories BEGIN
                     INSERT INTO core_memories_fts(core_memories_fts, rowid, key, content, category)
                     VALUES ('delete', old.rowid, old.key, old.content, old.category);
                 END;

                 CREATE TRIGGER IF NOT EXISTS core_memories_au AFTER UPDATE ON core_memories BEGIN
                     INSERT INTO core_memories_fts(core_memories_fts, rowid, key, content, category)
                     VALUES ('delete', old.rowid, old.key, old.content, old.category);
                     INSERT INTO core_memories_fts(rowid, key, content, category)
                     VALUES (new.rowid, new.key, new.content, new.category);
                 END;

                 -- FTS5 for messages search
                 CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                     role, content,
                     content=messages,
                     content_rowid=id
                 );

                 CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
                     INSERT INTO messages_fts(rowid, role, content)
                     VALUES (new.id, new.role, new.content);
                 END;

                 CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
                     INSERT INTO messages_fts(messages_fts, rowid, role, content)
                     VALUES ('delete', old.id, old.role, old.content);
                 END;",
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        // Rebuild FTS indexes for any pre-existing data
        conn.call(|db| {
            db.execute_batch(
                "INSERT OR IGNORE INTO core_memories_fts(core_memories_fts) VALUES('rebuild');
                 INSERT OR IGNORE INTO messages_fts(messages_fts) VALUES('rebuild');",
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        Ok(Self { conn })
    }

    pub async fn store_memory(&self, key: &str, content: &str, category: &str) -> Result<()> {
        let key = key.to_string();
        let content = content.to_string();
        let category = category.to_string();
        let now = Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();

        let key_log = key.clone();
        self.conn
            .call(move |db| {
                db.execute(
                    "INSERT INTO core_memories (id, key, content, category, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                     ON CONFLICT(key) DO UPDATE SET
                         content = excluded.content,
                         category = excluded.category,
                         updated_at = excluded.updated_at",
                    rusqlite::params![id, key, content, category, now],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;

        tracing::debug!(key = %key_log, "memory stored");
        Ok(())
    }

    pub async fn forget_memory(&self, key: &str) -> Result<bool> {
        let key = key.to_string();

        let deleted = self
            .conn
            .call(move |db| {
                let count = db.execute(
                    "DELETE FROM core_memories WHERE key = ?1",
                    rusqlite::params![key],
                )?;
                Ok::<_, rusqlite::Error>(count > 0)
            })
            .await?;

        Ok(deleted)
    }

    pub async fn load_all_memories(&self) -> Result<Vec<CoreMemory>> {
        let rows = self
            .conn
            .call(|db| {
                let mut stmt =
                    db.prepare("SELECT key, content, category FROM core_memories ORDER BY key")?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok(CoreMemory {
                            key: row.get(0)?,
                            content: row.get(1)?,
                            category: row.get(2)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await?;

        Ok(rows)
    }

    /// FTS5 search across core_memories. Returns ranked results.
    pub async fn search_memories(&self, query: &str, limit: usize) -> Result<Vec<CoreMemory>> {
        let query = query.to_string();
        let rows = self
            .conn
            .call(move |db| {
                let mut stmt = db.prepare(
                    "SELECT cm.key, cm.content, cm.category,
                            rank
                     FROM core_memories_fts fts
                     JOIN core_memories cm ON cm.rowid = fts.rowid
                     WHERE core_memories_fts MATCH ?1
                     ORDER BY rank
                     LIMIT ?2",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![query, limit], |row| {
                        Ok(CoreMemory {
                            key: row.get(0)?,
                            content: row.get(1)?,
                            category: row.get(2)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await?;

        Ok(rows)
    }

    /// FTS5 search across chat messages. Returns matching messages with context.
    pub async fn search_messages(
        &self,
        query: &str,
        chat_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<ChatMessage>> {
        let query = query.to_string();
        let rows = self
            .conn
            .call(move |db| {
                let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(cid) = chat_id {
                    (
                        "SELECT m.role, m.content, m.timestamp
                         FROM messages_fts fts
                         JOIN messages m ON m.id = fts.rowid
                         WHERE messages_fts MATCH ?1 AND m.chat_id = ?2
                         ORDER BY rank
                         LIMIT ?3",
                        vec![
                            Box::new(query) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(cid),
                            Box::new(limit as i64),
                        ],
                    )
                } else {
                    (
                        "SELECT m.role, m.content, m.timestamp
                         FROM messages_fts fts
                         JOIN messages m ON m.id = fts.rowid
                         WHERE messages_fts MATCH ?1
                         ORDER BY rank
                         LIMIT ?2",
                        vec![
                            Box::new(query) as Box<dyn rusqlite::types::ToSql>,
                            Box::new(limit as i64),
                        ],
                    )
                };

                let mut stmt = db.prepare(sql)?;
                let rows = stmt
                    .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                        Ok(ChatMessage {
                            role: row.get(0)?,
                            content: row.get(1)?,
                            timestamp: row.get(2)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await?;

        Ok(rows)
    }

    /// Save embedding vector for a memory key.
    pub async fn save_embedding(&self, key: &str, embedding: &[f32]) -> Result<()> {
        let key = key.to_string();
        let blob = embedding_to_blob(embedding);
        let now = Utc::now().to_rfc3339();

        self.conn
            .call(move |db| {
                db.execute(
                    "INSERT INTO memory_embeddings (key, embedding, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET
                         embedding = excluded.embedding,
                         updated_at = excluded.updated_at",
                    rusqlite::params![key, blob, now],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;

        Ok(())
    }

    /// Load all embeddings for semantic search.
    pub async fn load_all_embeddings(&self) -> Result<Vec<(String, Vec<f32>)>> {
        let rows = self
            .conn
            .call(|db| {
                let mut stmt = db.prepare(
                    "SELECT key, embedding FROM memory_embeddings",
                )?;
                let rows = stmt
                    .query_map([], |row| {
                        let key: String = row.get(0)?;
                        let blob: Vec<u8> = row.get(1)?;
                        Ok((key, blob))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await?;

        Ok(rows
            .into_iter()
            .map(|(key, blob)| (key, blob_to_embedding(&blob)))
            .collect())
    }

    /// Semantic search: find memories most similar to query embedding.
    pub async fn search_by_embedding(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(CoreMemory, f32)>> {
        let all_embeddings = self.load_all_embeddings().await?;
        let all_memories = self.load_all_memories().await?;

        // Build key->memory map
        let memory_map: std::collections::HashMap<&str, &CoreMemory> =
            all_memories.iter().map(|m| (m.key.as_str(), m)).collect();

        // Compute similarities
        let mut scored: Vec<(CoreMemory, f32)> = all_embeddings
            .iter()
            .filter_map(|(key, emb)| {
                let score = crate::llm::embeddings::cosine_similarity(query_embedding, emb);
                memory_map.get(key.as_str()).map(|m| ((*m).clone(), score))
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored)
    }

    /// Replace the oldest `count` messages with a summary message.
    pub async fn compress_messages(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        count: usize,
        summary: &str,
    ) -> Result<()> {
        let summary = summary.to_string();
        self.conn
            .call(move |db| {
                let tx = db.transaction()?;

                // Get IDs of oldest messages to delete
                let ids: Vec<i64> = if let Some(tid) = thread_id {
                    let mut stmt = tx.prepare(
                        "SELECT id FROM messages WHERE chat_id = ?1 AND thread_id = ?2 ORDER BY id ASC LIMIT ?3",
                    )?;
                    let rows: Vec<i64> = stmt
                        .query_map(rusqlite::params![chat_id, tid, count], |row| row.get(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                } else {
                    let mut stmt = tx.prepare(
                        "SELECT id FROM messages WHERE chat_id = ?1 AND thread_id IS NULL ORDER BY id ASC LIMIT ?2",
                    )?;
                    let rows: Vec<i64> = stmt
                        .query_map(rusqlite::params![chat_id, count], |row| row.get(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                };

                if ids.is_empty() {
                    return Ok::<_, rusqlite::Error>(());
                }

                // Delete old messages
                let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
                let sql = format!(
                    "DELETE FROM messages WHERE id IN ({})",
                    placeholders.join(",")
                );
                let params: Vec<Box<dyn rusqlite::types::ToSql>> =
                    ids.iter().map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>).collect();
                tx.execute(&sql, rusqlite::params_from_iter(params.iter()))?;

                // Insert summary as a system message with earliest possible timestamp
                tx.execute(
                    "INSERT INTO messages (chat_id, thread_id, role, content, timestamp)
                     VALUES (?1, ?2, 'system', ?3, '1970-01-01T00:00:00Z')",
                    rusqlite::params![chat_id, thread_id, summary],
                )?;

                tx.commit()?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;

        Ok(())
    }

    pub async fn save_message(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        role: &str,
        content: &str,
    ) -> Result<()> {
        let role = role.to_string();
        let content = content.to_string();
        let now = Utc::now().to_rfc3339();

        self.conn
            .call(move |db| {
                db.execute(
                    "INSERT INTO messages (chat_id, thread_id, role, content, timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![chat_id, thread_id, role, content, now],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;

        Ok(())
    }

    pub async fn load_history(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        limit: usize,
    ) -> Result<Vec<ChatMessage>> {
        let rows = self
            .conn
            .call(move |db| {
                let mut rows = if let Some(tid) = thread_id {
                    let mut stmt = db.prepare(
                        "SELECT role, content, timestamp FROM messages
                         WHERE chat_id = ?1 AND thread_id = ?2
                         ORDER BY id DESC LIMIT ?3",
                    )?;
                    let r = stmt
                        .query_map(rusqlite::params![chat_id, tid, limit], |row| {
                            Ok(ChatMessage {
                                role: row.get(0)?,
                                content: row.get(1)?,
                                timestamp: row.get(2)?,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    r
                } else {
                    let mut stmt = db.prepare(
                        "SELECT role, content, timestamp FROM messages
                         WHERE chat_id = ?1 AND thread_id IS NULL
                         ORDER BY id DESC LIMIT ?2",
                    )?;
                    let r = stmt
                        .query_map(rusqlite::params![chat_id, limit], |row| {
                            Ok(ChatMessage {
                                role: row.get(0)?,
                                content: row.get(1)?,
                                timestamp: row.get(2)?,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    r
                };
                rows.reverse();
                Ok::<_, rusqlite::Error>(rows)
            })
            .await?;

        Ok(rows)
    }
}

/// Encode f32 vector as little-endian bytes for SQLite blob storage.
fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Decode little-endian bytes back to f32 vector.
fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}
