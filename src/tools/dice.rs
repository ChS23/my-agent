use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use frankenstein::client_reqwest::Bot;
use frankenstein::methods::SendDiceParams;
use frankenstein::AsyncTelegramApi;

use super::ToolResult;

pub struct SendDiceTool;

impl SendDiceTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "send_dice".into(),
                description: Some(
                    "Send an animated dice/emoji to the chat. Telegram shows a real animation \
                     and returns a random result. Use when the user wants to roll dice, flip a coin, \
                     play darts, basketball, bowling, football, or slot machine.\n\
                     Supported emoji: 🎲 (dice 1-6), 🎯 (darts 1-6), 🏀 (basketball 1-5), \
                     ⚽ (football 1-5), 🎳 (bowling 1-6), 🎰 (slots 1-64)"
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "emoji": {
                            "type": "string",
                            "enum": ["🎲", "🎯", "🏀", "⚽", "🎳", "🎰"],
                            "description": "Which animated emoji to send. Default: 🎲"
                        }
                    }
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(
        arguments: &str,
        bot: &Bot,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let emoji = args["emoji"].as_str().unwrap_or("🎲");

        let mut params = SendDiceParams::builder()
            .chat_id(chat_id)
            .emoji(emoji)
            .build();

        if let Some(tid) = thread_id {
            params.message_thread_id = Some(tid);
        }

        match bot.send_dice(&params).await {
            Ok(resp) => {
                let value = resp
                    .result
                    .dice
                    .map(|d| d.value)
                    .unwrap_or(0);
                tracing::info!(emoji, value, "dice sent");
                Ok(ToolResult {
                    output: format!("Dice sent! Emoji: {emoji}, Result: {value}"),
                })
            }
            Err(e) => Ok(ToolResult {
                output: format!("Failed to send dice: {e}"),
            }),
        }
    }
}
