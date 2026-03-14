use anyhow::Result;
use async_openai::types::chat::ChatCompletionTools;
use serde_json::json;

use crate::ticktick::TickTickClient;
use crate::ticktick::client::Task;

use super::ToolResult;

pub struct TickTickAuthTool;
pub struct TickTickCreateTool;
pub struct TickTickListTool;
pub struct TickTickCompleteTool;
pub struct TickTickDeleteTool;

impl TickTickAuthTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "ticktick_auth",
                "description": "Start TickTick OAuth2 authorization. Returns a URL the user must visit. After authorizing, the browser redirects to localhost — if running locally, the token is saved automatically. If running in Docker, ask the user to paste the redirect URL back into the chat, then use ticktick_callback to exchange the code.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(client: &TickTickClient) -> Result<ToolResult> {
        if client.token_store().is_authorized().await {
            return Ok(ToolResult {
                output: "TickTick already authorized.".to_string(),
            });
        }

        let auth_url = client.token_store().auth_url();

        // Start callback server in background (works for local dev)
        let token_store = client.token_store().clone();
        tokio::spawn(async move {
            match crate::ticktick::TokenStore::wait_for_callback().await {
                Ok(code) => {
                    match token_store.exchange_code(&code).await {
                        Ok(_) => tracing::info!("TickTick OAuth complete via localhost"),
                        Err(e) => tracing::error!(error = %e, "TickTick token exchange failed"),
                    }
                }
                Err(e) => tracing::debug!(error = %e, "TickTick localhost callback not available (expected in Docker)"),
            }
        });

        Ok(ToolResult {
            output: format!(
                "Open this URL to authorize TickTick:\n{}\n\n\
                 If localhost redirect works — done automatically.\n\
                 If not (Docker) — paste the redirect URL from the browser address bar here, \
                 and I'll extract the code with ticktick_callback.",
                auth_url
            ),
        })
    }
}

pub struct TickTickCallbackTool;

impl TickTickCallbackTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "ticktick_callback",
                "description": "Exchange a TickTick OAuth callback URL or authorization code for access tokens. Use when the user pastes a URL like 'http://localhost:8080/callback?code=...' or just the code itself.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url_or_code": {
                            "type": "string",
                            "description": "The full redirect URL or just the authorization code"
                        }
                    },
                    "required": ["url_or_code"]
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(arguments: &str, client: &TickTickClient) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let input = args["url_or_code"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'url_or_code'"))?;

        // Extract code from URL or use as-is
        let code = if input.contains("code=") {
            input
                .split("code=")
                .nth(1)
                .and_then(|s| s.split('&').next())
                .unwrap_or(input)
        } else {
            input.trim()
        };

        client.token_store().exchange_code(code).await?;

        Ok(ToolResult {
            output: "TickTick authorized successfully!".to_string(),
        })
    }
}

impl TickTickCreateTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "ticktick_create",
                "description": "Create a task in TickTick. Omit project_id to add to Inbox.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Task title"
                        },
                        "content": {
                            "type": "string",
                            "description": "Task description/notes (optional)"
                        },
                        "due_date": {
                            "type": "string",
                            "description": "Due date in ISO 8601 format, e.g. '2026-03-15T14:00:00+0000' (optional)"
                        },
                        "priority": {
                            "type": "integer",
                            "description": "Priority: 0=none, 1=low, 3=medium, 5=high (optional)"
                        },
                        "project_id": {
                            "type": "string",
                            "description": "Project ID to add task to (optional, defaults to Inbox)"
                        },
                        "tags": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Tags (optional)"
                        }
                    },
                    "required": ["title"]
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(arguments: &str, client: &TickTickClient) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;

        let title = args["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'title'"))?;

        let task = Task {
            id: None,
            title: title.to_string(),
            content: args["content"].as_str().map(|s| s.to_string()),
            project_id: args["project_id"].as_str().map(|s| s.to_string()),
            due_date: args["due_date"].as_str().map(|s| s.to_string()),
            priority: args["priority"].as_i64().map(|n| n as i32),
            tags: args["tags"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()),
            status: None,
        };

        let created = client.create_task(&task).await?;

        Ok(ToolResult {
            output: format!(
                "Task created: '{}' (id: {})",
                created.title,
                created.id.unwrap_or_default()
            ),
        })
    }
}

impl TickTickListTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "ticktick_list",
                "description": "List tasks from TickTick. Can list all tasks or tasks from a specific project.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "project_id": {
                            "type": "string",
                            "description": "Project ID to list tasks from (optional, lists all if omitted)"
                        }
                    },
                    "required": []
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(arguments: &str, client: &TickTickClient) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;

        let tasks = if let Some(project_id) = args["project_id"].as_str() {
            let data = client.get_project_data(project_id).await?;
            data.tasks
                .into_iter()
                .map(|t| (data.project.name.clone(), t))
                .collect()
        } else {
            client.list_all_tasks().await?
        };

        if tasks.is_empty() {
            return Ok(ToolResult {
                output: "No tasks found.".to_string(),
            });
        }

        let mut output = String::new();
        for (project_name, task) in &tasks {
            let priority = match task.priority {
                Some(5) => " [!!!]",
                Some(3) => " [!!]",
                Some(1) => " [!]",
                _ => "",
            };
            let due = task
                .due_date
                .as_deref()
                .map(|d| format!(" due:{d}"))
                .unwrap_or_default();

            output.push_str(&format!(
                "• {}{} — {}{} (id: {}, project: {})\n",
                task.title,
                priority,
                project_name,
                due,
                task.id.as_deref().unwrap_or("?"),
                task.project_id.as_deref().unwrap_or("?"),
            ));
        }

        Ok(ToolResult { output })
    }
}

impl TickTickCompleteTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "ticktick_complete",
                "description": "Mark a task as complete in TickTick. Requires both project_id and task_id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "string",
                            "description": "Task ID"
                        },
                        "project_id": {
                            "type": "string",
                            "description": "Project ID the task belongs to"
                        }
                    },
                    "required": ["task_id", "project_id"]
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(arguments: &str, client: &TickTickClient) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let task_id = args["task_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'task_id'"))?;
        let project_id = args["project_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'project_id'"))?;

        client.complete_task(project_id, task_id).await?;

        Ok(ToolResult {
            output: format!("Task {task_id} marked as complete."),
        })
    }
}

impl TickTickDeleteTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "ticktick_delete",
                "description": "Delete a task from TickTick. Requires both project_id and task_id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "string",
                            "description": "Task ID"
                        },
                        "project_id": {
                            "type": "string",
                            "description": "Project ID the task belongs to"
                        }
                    },
                    "required": ["task_id", "project_id"]
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(arguments: &str, client: &TickTickClient) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let task_id = args["task_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'task_id'"))?;
        let project_id = args["project_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'project_id'"))?;

        client.delete_task(project_id, task_id).await?;

        Ok(ToolResult {
            output: format!("Task {task_id} deleted."),
        })
    }
}
