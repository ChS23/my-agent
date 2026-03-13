use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};
use frankenstein::client_reqwest::Bot;
use frankenstein::methods::SendMessageParams;
use frankenstein::types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use frankenstein::ParseMode;
use frankenstein::AsyncTelegramApi;

use super::ToolResult;

pub struct SendButtonsTool;

impl SendButtonsTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "send_buttons".into(),
                description: Some(
                    "Send a rich message with inline keyboard buttons. Supports:\n\
                     - Action buttons (callback): user clicks → text sent back as new message\n\
                     - URL buttons: opens a link in browser\n\
                     Text supports Telegram HTML formatting (<b>, <i>, <a href=\"...\">).\n\
                     Use for: suggested actions, confirmations, links, navigation, quick replies."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": {
                            "type": "string",
                            "description": "Message text (Telegram HTML). Can include <a href='url'>links</a>, <b>bold</b>, <i>italic</i>"
                        },
                        "buttons": {
                            "type": "array",
                            "description": "Rows of buttons. Each row is an array of button objects.",
                            "items": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Button text shown to user"
                                        },
                                        "data": {
                                            "type": "string",
                                            "description": "For action buttons: text sent back when clicked (defaults to label)"
                                        },
                                        "url": {
                                            "type": "string",
                                            "description": "For URL buttons: opens this link when clicked. If set, 'data' is ignored."
                                        }
                                    },
                                    "required": ["label"]
                                }
                            }
                        }
                    },
                    "required": ["text", "buttons"]
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
        let text = args["text"].as_str().unwrap_or("Choose:");
        let buttons_raw = args["buttons"].as_array();

        let rows: Vec<Vec<InlineKeyboardButton>> = match buttons_raw {
            Some(rows) => rows
                .iter()
                .map(|row| {
                    row.as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|btn| {
                            let label = btn["label"].as_str()?.to_string();

                            if let Some(url) = btn["url"].as_str() {
                                // URL button
                                Some(
                                    InlineKeyboardButton::builder()
                                        .text(label)
                                        .url(url)
                                        .build(),
                                )
                            } else {
                                // Callback button
                                let data = btn["data"]
                                    .as_str()
                                    .unwrap_or(btn["label"].as_str().unwrap_or("?"))
                                    .to_string();
                                // Telegram callback_data max 64 bytes
                                let data = if data.len() > 64 {
                                    data[..64].to_string()
                                } else {
                                    data
                                };
                                Some(
                                    InlineKeyboardButton::builder()
                                        .text(label)
                                        .callback_data(data)
                                        .build(),
                                )
                            }
                        })
                        .collect()
                })
                .collect(),
            None => {
                return Ok(ToolResult {
                    output: "Error: buttons array is required".into(),
                });
            }
        };

        let keyboard = InlineKeyboardMarkup::builder()
            .inline_keyboard(rows)
            .build();

        let mut params = SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::Html)
            .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
            .build();

        if let Some(tid) = thread_id {
            params.message_thread_id = Some(tid);
        }

        match bot.send_message(&params).await {
            Ok(_) => Ok(ToolResult {
                output: "Buttons sent".into(),
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Failed to send buttons: {e}"),
            }),
        }
    }
}
