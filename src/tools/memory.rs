use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};

use super::ToolResult;
use crate::memory::MemoryStore;

pub struct MemoryStoreTool;

impl MemoryStoreTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "memory_store".into(),
                description: Some(
                    "Store or update a fact about the user. Use this when you learn something \
                     important: preferences, decisions, personal details, habits."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "Short identifier, e.g. 'preferred_language', 'name', 'timezone'"
                        },
                        "content": {
                            "type": "string",
                            "description": "The fact to remember"
                        },
                        "category": {
                            "type": "string",
                            "enum": ["core", "preference", "decision"],
                            "description": "Category of the memory"
                        }
                    },
                    "required": ["key", "content"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, store: &MemoryStore) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let key = args["key"].as_str().unwrap_or("unknown");
        let content = args["content"].as_str().unwrap_or("");
        let category = args["category"].as_str().unwrap_or("core");

        store.store_memory(key, content, category).await?;

        Ok(ToolResult {
            output: format!("Stored: {key} = {content}"),
        })
    }
}

pub struct MemoryForgetTool;

impl MemoryForgetTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "memory_forget".into(),
                description: Some(
                    "Delete a stored fact. Use when the user asks to forget something \
                     or when information is no longer accurate."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "The key of the memory to forget"
                        }
                    },
                    "required": ["key"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, store: &MemoryStore) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let key = args["key"].as_str().unwrap_or("");

        let deleted = store.forget_memory(key).await?;

        let output = if deleted {
            format!("Forgot: {key}")
        } else {
            format!("No memory found: {key}")
        };

        Ok(ToolResult { output })
    }
}
