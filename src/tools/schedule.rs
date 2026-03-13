use anyhow::Result;
use async_openai::types::chat::ChatCompletionTools;
use serde_json::json;

use crate::scheduler::store::{Job, ScheduleStore};
use crate::scheduler::Schedule;

use super::ToolResult;

pub struct ScheduleAddTool;
pub struct ScheduleListTool;
pub struct ScheduleCancelTool;

impl ScheduleAddTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "schedule_add",
                "description": "Create a scheduled job. The agent will be prompted at the scheduled time. Use for reminders, recurring tasks, periodic checks. Supports cron expressions (with timezone), one-shot 'at' time, or 'every N seconds' interval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Short name for the job (e.g. 'morning briefing', 'check weather')"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "The prompt that will be sent to the agent when the job fires"
                        },
                        "schedule_type": {
                            "type": "string",
                            "enum": ["cron", "at", "every"],
                            "description": "Type of schedule"
                        },
                        "cron_expr": {
                            "type": "string",
                            "description": "Cron expression (6-field with seconds: 'sec min hour day month weekday'). Required for type 'cron'. Example: '0 0 9 * * *' = every day at 9:00"
                        },
                        "timezone": {
                            "type": "string",
                            "description": "IANA timezone for cron (e.g. 'Europe/Moscow'). Optional, defaults to UTC."
                        },
                        "at_time": {
                            "type": "string",
                            "description": "ISO 8601 datetime for one-shot schedule. Required for type 'at'. Example: '2026-03-15T14:00:00Z'"
                        },
                        "every_secs": {
                            "type": "integer",
                            "description": "Interval in seconds. Required for type 'every'. Minimum 60."
                        }
                    },
                    "required": ["name", "prompt", "schedule_type"]
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(
        arguments: &str,
        store: &ScheduleStore,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;

        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'prompt'"))?;
        let schedule_type = args["schedule_type"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'schedule_type'"))?;

        let schedule = match schedule_type {
            "cron" => {
                let expr = args["cron_expr"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing 'cron_expr' for cron schedule"))?;
                // Validate
                expr.parse::<cron::Schedule>()
                    .map_err(|e| anyhow::anyhow!("invalid cron expression '{expr}': {e}"))?;
                let tz = args["timezone"].as_str().map(|s| s.to_string());
                // Validate timezone if provided
                if let Some(ref tz_str) = tz {
                    tz_str
                        .parse::<chrono_tz::Tz>()
                        .map_err(|_| anyhow::anyhow!("invalid timezone: {tz_str}"))?;
                }
                Schedule::Cron {
                    expr: expr.to_string(),
                    tz,
                }
            }
            "at" => {
                let at = args["at_time"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing 'at_time' for at schedule"))?;
                // Validate
                at.parse::<chrono::DateTime<chrono::Utc>>()
                    .map_err(|e| anyhow::anyhow!("invalid datetime '{at}': {e}"))?;
                Schedule::At {
                    at: at.to_string(),
                }
            }
            "every" => {
                let secs = args["every_secs"]
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("missing 'every_secs' for every schedule"))?;
                if secs < 60 {
                    anyhow::bail!("minimum interval is 60 seconds");
                }
                Schedule::Every { every_secs: secs }
            }
            other => anyhow::bail!("unknown schedule_type: {other}"),
        };

        let next = schedule.next_run().unwrap_or_default();
        let id = store
            .add_job(name, &schedule, prompt, chat_id, thread_id)
            .await?;

        Ok(ToolResult {
            output: format!("Scheduled '{name}' (id: {id})\nSchedule: {schedule}\nNext run: {next}"),
        })
    }
}

impl ScheduleListTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "schedule_list",
                "description": "List all scheduled jobs for this chat.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(
        store: &ScheduleStore,
        chat_id: i64,
    ) -> Result<ToolResult> {
        let jobs: Vec<Job> = store.list_jobs(chat_id).await?;

        if jobs.is_empty() {
            return Ok(ToolResult {
                output: "No scheduled jobs.".to_string(),
            });
        }

        let mut output = String::new();
        for job in &jobs {
            let status = if job.enabled { "active" } else { "disabled" };
            output.push_str(&format!(
                "• {} [{}] — {}\n  Schedule: {}\n  Next: {}\n  Prompt: {}\n\n",
                job.name,
                status,
                &job.id[..8],
                job.schedule,
                job.next_run.as_deref().unwrap_or("none"),
                truncate_str(&job.prompt, 80),
            ));
        }

        Ok(ToolResult { output })
    }
}

impl ScheduleCancelTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "schedule_cancel",
                "description": "Cancel (delete) a scheduled job by its ID or name prefix.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Job ID (full or first 8 chars) or job name"
                        }
                    },
                    "required": ["id"]
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(
        arguments: &str,
        store: &ScheduleStore,
        chat_id: i64,
    ) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let search = args["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'id'"))?;

        // Try direct ID match first
        if store.delete_job(search).await? {
            return Ok(ToolResult {
                output: format!("Job {search} deleted."),
            });
        }

        // Search by prefix or name
        let jobs: Vec<Job> = store.list_jobs(chat_id).await?;
        let matched: Vec<&Job> = jobs
            .iter()
            .filter(|j| {
                j.id.starts_with(search)
                    || j.name.to_lowercase().contains(&search.to_lowercase())
            })
            .collect();

        match matched.len() {
            0 => Ok(ToolResult {
                output: format!("No job found matching '{search}'."),
            }),
            1 => {
                store.delete_job(&matched[0].id).await?;
                Ok(ToolResult {
                    output: format!("Job '{}' deleted.", matched[0].name),
                })
            }
            n => {
                let names: Vec<&str> = matched.iter().map(|j| j.name.as_str()).collect();
                Ok(ToolResult {
                    output: format!(
                        "Ambiguous: {n} jobs match '{search}': {}. Use full ID.",
                        names.join(", ")
                    ),
                })
            }
        }
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..s.floor_char_boundary(max)])
    }
}
