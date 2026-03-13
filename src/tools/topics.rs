use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use frankenstein::methods::{
    CloseForumTopicParams, CreateForumTopicParams, DeleteForumTopicParams,
    EditForumTopicParams, ReopenForumTopicParams,
};
use frankenstein::AsyncTelegramApi;

use super::{ToolContext, ToolResult};

pub struct CreateTopicTool;

impl CreateTopicTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "create_topic".into(),
                description: Some(
                    "Create a new forum topic (thread) in the current chat. \
                     Use when the user asks to create a new topic or conversation thread."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Name of the new topic"
                        },
                        "icon_color": {
                            "type": "integer",
                            "description": "Color of the topic icon in RGB (optional). Valid values: 7322096, 16766590, 13338331, 9367192, 16749490, 16478047"
                        }
                    },
                    "required": ["name"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, ctx: &ToolContext<'_>) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let name = args["name"].as_str().unwrap_or("New Topic");

        let mut params = CreateForumTopicParams::builder()
            .chat_id(ctx.chat_id)
            .name(name)
            .build();

        if let Some(color) = args["icon_color"].as_u64() {
            params.icon_color = Some(color as u32);
        }

        match ctx.bot.create_forum_topic(&params).await {
            Ok(resp) => Ok(ToolResult {
                output: format!(
                    "Topic '{}' created (thread_id: {})",
                    resp.result.name, resp.result.message_thread_id
                ),
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to create topic: {e}"),
            }),
        }
    }
}

pub struct RenameTopicTool;

impl RenameTopicTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "rename_topic".into(),
                description: Some(
                    "Rename a forum topic in the current chat. \
                     Use when the user asks to rename the current or a specific topic."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "thread_id": {
                            "type": "integer",
                            "description": "ID of the thread/topic to rename"
                        },
                        "name": {
                            "type": "string",
                            "description": "New name for the topic"
                        }
                    },
                    "required": ["thread_id", "name"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, ctx: &ToolContext<'_>) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let thread_id = args["thread_id"].as_i64().unwrap_or(0) as i32;
        let name = args["name"].as_str().unwrap_or("");

        let params = EditForumTopicParams::builder()
            .chat_id(ctx.chat_id)
            .message_thread_id(thread_id)
            .name(name)
            .build();

        match ctx.bot.edit_forum_topic(&params).await {
            Ok(_) => Ok(ToolResult {
                output: format!("Topic {thread_id} renamed to '{name}'"),
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to rename topic: {e}"),
            }),
        }
    }
}

pub struct CloseTopicTool;

impl CloseTopicTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "close_topic".into(),
                description: Some(
                    "Close a forum topic. Use when the user asks to close/archive a topic.".into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "thread_id": {
                            "type": "integer",
                            "description": "ID of the thread/topic to close"
                        }
                    },
                    "required": ["thread_id"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, ctx: &ToolContext<'_>) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let thread_id = args["thread_id"].as_i64().unwrap_or(0) as i32;

        let params = CloseForumTopicParams::builder()
            .chat_id(ctx.chat_id)
            .message_thread_id(thread_id)
            .build();

        match ctx.bot.close_forum_topic(&params).await {
            Ok(_) => Ok(ToolResult {
                output: format!("Topic {thread_id} closed"),
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to close topic: {e}"),
            }),
        }
    }
}

pub struct ReopenTopicTool;

impl ReopenTopicTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "reopen_topic".into(),
                description: Some(
                    "Reopen a closed forum topic. Use when the user asks to reopen a topic.".into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "thread_id": {
                            "type": "integer",
                            "description": "ID of the thread/topic to reopen"
                        }
                    },
                    "required": ["thread_id"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, ctx: &ToolContext<'_>) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let thread_id = args["thread_id"].as_i64().unwrap_or(0) as i32;

        let params = ReopenForumTopicParams::builder()
            .chat_id(ctx.chat_id)
            .message_thread_id(thread_id)
            .build();

        match ctx.bot.reopen_forum_topic(&params).await {
            Ok(_) => Ok(ToolResult {
                output: format!("Topic {thread_id} reopened"),
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to reopen topic: {e}"),
            }),
        }
    }
}

pub struct DeleteTopicTool;

impl DeleteTopicTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "delete_topic".into(),
                description: Some(
                    "Delete a forum topic and all its messages. This is irreversible. \
                     Use when the user explicitly asks to delete a topic."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "thread_id": {
                            "type": "integer",
                            "description": "ID of the thread/topic to delete"
                        }
                    },
                    "required": ["thread_id"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, ctx: &ToolContext<'_>) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let thread_id = args["thread_id"].as_i64().unwrap_or(0) as i32;

        let params = DeleteForumTopicParams::builder()
            .chat_id(ctx.chat_id)
            .message_thread_id(thread_id)
            .build();

        match ctx.bot.delete_forum_topic(&params).await {
            Ok(_) => Ok(ToolResult {
                output: format!("Topic {thread_id} deleted"),
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to delete topic: {e}"),
            }),
        }
    }
}
