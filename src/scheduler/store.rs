use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio_rusqlite::Connection;
use tokio_rusqlite::rusqlite;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub schedule: Schedule,
    pub prompt: String,
    pub chat_id: i64,
    pub thread_id: Option<i32>,
    pub enabled: bool,
    pub next_run: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Schedule {
    #[serde(rename = "cron")]
    Cron {
        expr: String,
        #[serde(default)]
        tz: Option<String>,
    },
    #[serde(rename = "at")]
    At { at: String },
    #[serde(rename = "every")]
    Every { every_secs: u64 },
}

impl Schedule {
    /// Calculate the next run time from now.
    pub fn next_run(&self) -> Option<String> {
        match self {
            Schedule::Cron { expr, tz } => {
                let schedule: cron::Schedule = expr.parse().ok()?;
                if let Some(tz_name) = tz {
                    let tz: chrono_tz::Tz = tz_name.parse().ok()?;
                    let now_tz = Utc::now().with_timezone(&tz);
                    schedule
                        .after(&now_tz)
                        .next()
                        .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
                } else {
                    schedule
                        .after(&Utc::now())
                        .next()
                        .map(|dt| dt.to_rfc3339())
                }
            }
            Schedule::At { at } => {
                let dt: DateTime<Utc> = at.parse().ok()?;
                if dt > Utc::now() {
                    Some(dt.to_rfc3339())
                } else {
                    None // Already past
                }
            }
            Schedule::Every { every_secs } => {
                let next = Utc::now() + chrono::Duration::seconds(*every_secs as i64);
                Some(next.to_rfc3339())
            }
        }
    }
}

impl std::fmt::Display for Schedule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Schedule::Cron { expr, tz } => {
                write!(f, "cron: {expr}")?;
                if let Some(tz) = tz {
                    write!(f, " ({tz})")?;
                }
                Ok(())
            }
            Schedule::At { at } => write!(f, "at: {at}"),
            Schedule::Every { every_secs } => {
                if *every_secs >= 3600 {
                    write!(f, "every {}h", every_secs / 3600)
                } else if *every_secs >= 60 {
                    write!(f, "every {}m", every_secs / 60)
                } else {
                    write!(f, "every {every_secs}s")
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ScheduleStore {
    conn: Connection,
}

impl ScheduleStore {
    pub async fn new(db_path: &str) -> Result<Self> {
        if let Some(parent) = std::path::Path::new(db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path).await?;

        conn.call(|db| {
            db.execute_batch(
                "PRAGMA journal_mode = WAL;

                 CREATE TABLE IF NOT EXISTS scheduled_jobs (
                     id TEXT PRIMARY KEY,
                     name TEXT NOT NULL,
                     schedule TEXT NOT NULL,
                     prompt TEXT NOT NULL,
                     chat_id INTEGER NOT NULL,
                     thread_id INTEGER,
                     enabled INTEGER NOT NULL DEFAULT 1,
                     next_run TEXT,
                     last_run TEXT,
                     last_status TEXT,
                     last_output TEXT,
                     created_at TEXT NOT NULL
                 );

                 CREATE INDEX IF NOT EXISTS idx_jobs_next_run
                     ON scheduled_jobs(next_run) WHERE enabled = 1;",
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        Ok(Self { conn })
    }

    pub async fn add_job(
        &self,
        name: &str,
        schedule: &Schedule,
        prompt: &str,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let schedule_json = serde_json::to_string(schedule)?;
        let next_run = schedule.next_run();
        let now = Utc::now().to_rfc3339();

        let id_clone = id.clone();
        let name = name.to_string();
        let name_log = name.clone();
        let prompt = prompt.to_string();

        self.conn
            .call(move |db| {
                db.execute(
                    "INSERT INTO scheduled_jobs (id, name, schedule, prompt, chat_id, thread_id, next_run, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![id_clone, name, schedule_json, prompt, chat_id, thread_id, next_run, now],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;

        tracing::info!(id = %id, name = %name_log, "job created");
        Ok(id)
    }

    pub async fn get_due_jobs(&self) -> Result<Vec<Job>> {
        let now = Utc::now().to_rfc3339();

        let jobs = self
            .conn
            .call(move |db| {
                let mut stmt = db.prepare(
                    "SELECT id, name, schedule, prompt, chat_id, thread_id, enabled, next_run
                     FROM scheduled_jobs
                     WHERE enabled = 1 AND next_run IS NOT NULL AND next_run <= ?1",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![now], |row| {
                        let schedule_json: String = row.get(2)?;
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            schedule_json,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, Option<i32>>(5)?,
                            row.get::<_, bool>(6)?,
                            row.get::<_, Option<String>>(7)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await?;

        let mut result = Vec::new();
        for (id, name, schedule_json, prompt, chat_id, thread_id, enabled, next_run) in jobs {
            if let Ok(schedule) = serde_json::from_str::<Schedule>(&schedule_json) {
                result.push(Job {
                    id,
                    name,
                    schedule,
                    prompt,
                    chat_id,
                    thread_id,
                    enabled,
                    next_run,
                });
            }
        }

        Ok(result)
    }

    pub async fn list_jobs(&self, chat_id: i64) -> Result<Vec<Job>> {
        let jobs = self
            .conn
            .call(move |db| {
                let mut stmt = db.prepare(
                    "SELECT id, name, schedule, prompt, chat_id, thread_id, enabled, next_run
                     FROM scheduled_jobs
                     WHERE chat_id = ?1
                     ORDER BY created_at DESC",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![chat_id], |row| {
                        let schedule_json: String = row.get(2)?;
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            schedule_json,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, Option<i32>>(5)?,
                            row.get::<_, bool>(6)?,
                            row.get::<_, Option<String>>(7)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok::<_, rusqlite::Error>(rows)
            })
            .await?;

        let mut result = Vec::new();
        for (id, name, schedule_json, prompt, chat_id, thread_id, enabled, next_run) in jobs {
            if let Ok(schedule) = serde_json::from_str::<Schedule>(&schedule_json) {
                result.push(Job {
                    id,
                    name,
                    schedule,
                    prompt,
                    chat_id,
                    thread_id,
                    enabled,
                    next_run,
                });
            }
        }

        Ok(result)
    }

    pub async fn delete_job(&self, id: &str) -> Result<bool> {
        let id = id.to_string();
        let deleted = self
            .conn
            .call(move |db| {
                let count = db.execute(
                    "DELETE FROM scheduled_jobs WHERE id = ?1",
                    rusqlite::params![id],
                )?;
                Ok::<_, rusqlite::Error>(count > 0)
            })
            .await?;
        Ok(deleted)
    }

    pub async fn set_next_run(&self, id: &str, next_run: &str) -> Result<()> {
        let id = id.to_string();
        let next_run = next_run.to_string();
        self.conn
            .call(move |db| {
                db.execute(
                    "UPDATE scheduled_jobs SET next_run = ?1 WHERE id = ?2",
                    rusqlite::params![next_run, id],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let id = id.to_string();
        self.conn
            .call(move |db| {
                db.execute(
                    "UPDATE scheduled_jobs SET enabled = ?1 WHERE id = ?2",
                    rusqlite::params![enabled, id],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }

    pub async fn mark_completed(&self, id: &str, status: &str, output: &str) -> Result<()> {
        let id = id.to_string();
        let status = status.to_string();
        let output = output.to_string();
        let now = Utc::now().to_rfc3339();
        self.conn
            .call(move |db| {
                db.execute(
                    "UPDATE scheduled_jobs SET last_run = ?1, last_status = ?2, last_output = ?3 WHERE id = ?4",
                    rusqlite::params![now, status, output, id],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .await?;
        Ok(())
    }
}
