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
                     ON messages(chat_id, thread_id, timestamp);",
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
