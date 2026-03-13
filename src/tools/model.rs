use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};

use super::ToolResult;
use crate::llm::LlmClient;

pub struct SetModelTool;

impl SetModelTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "set_model".into(),
                description: Some(
                    "Change the LLM model at runtime without restarting. \
                     Can also adjust temperature and max_tokens."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "model": {
                            "type": "string",
                            "description": "Model ID, e.g. 'google/gemini-2.5-flash', 'openai/gpt-4o'"
                        },
                        "temperature": {
                            "type": "number",
                            "description": "Temperature (0.0-2.0)"
                        },
                        "max_tokens": {
                            "type": "integer",
                            "description": "Max output tokens"
                        }
                    },
                    "required": ["model"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str, llm: &LlmClient) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let model = args["model"].as_str().unwrap_or("");

        if model.is_empty() {
            return Ok(ToolResult {
                output: "Error: model is required".into(),
            });
        }

        let temperature = args["temperature"].as_f64().map(|t| t as f32);
        let max_tokens = args["max_tokens"].as_u64().map(|m| m as u32);

        llm.set_model(model, temperature, max_tokens);

        Ok(ToolResult {
            output: format!("Model switched. Current settings: {}", llm.current_settings()),
        })
    }
}

pub struct GetModelTool;

impl GetModelTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "get_model".into(),
                description: Some("Get current LLM model settings.".into()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {}
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(llm: &LlmClient) -> Result<ToolResult> {
        Ok(ToolResult {
            output: llm.current_settings(),
        })
    }
}
