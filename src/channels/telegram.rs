use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{
    GetUpdatesParams, SendChatActionParams, SendMessageDraftParams, SendMessageParams,
};
use frankenstein::methods::AnswerCallbackQueryParams;
use frankenstein::types::{AllowedUpdate, ChatAction, InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use frankenstein::ParseMode;
use frankenstein::updates::{Update, UpdateContent};
use frankenstein::AsyncTelegramApi;
use tokio::sync::{broadcast, mpsc};

use crate::agent::Agent;
use crate::config::TelegramConfig;
use crate::llm::SttClient;

pub struct TelegramBot {
    bot: Bot,
    token: String,
    allowed_users: HashSet<String>,
    stream_throttle: Duration,
    stt: Option<SttClient>,
}

impl TelegramBot {
    pub fn new(token: &str, config: &TelegramConfig, stt: Option<SttClient>) -> Self {
        Self {
            bot: Bot::new(token),
            token: token.to_string(),
            allowed_users: config.allowed_users.iter().cloned().collect(),
            stream_throttle: Duration::from_millis(config.stream_throttle_ms),
            stt,
        }
    }

    /// Main polling loop. Runs until shutdown signal.
    pub async fn run(
        self: Arc<Self>,
        agent: Arc<Agent>,
        mut shutdown: broadcast::Receiver<()>,
    ) -> Result<()> {
        let mut offset: Option<i64> = None;

        tracing::info!(
            allowed_users = ?self.allowed_users,
            "telegram bot started polling"
        );

        loop {
            tokio::select! {
                biased;
                _ = shutdown.recv() => {
                    tracing::info!("telegram: shutdown received");
                    break;
                }
                result = self.poll(offset) => {
                    match result {
                        Ok(updates) => {
                            for update in updates {
                                let new_offset = update.update_id as i64 + 1;
                                offset = Some(match offset {
                                    Some(cur) => cur.max(new_offset),
                                    None => new_offset,
                                });

                                let bot = Arc::clone(&self);
                                let agent = Arc::clone(&agent);
                                tokio::spawn(async move {
                                    bot.handle_update(&agent, update).await;
                                });
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "polling error");
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn poll(&self, offset: Option<i64>) -> Result<Vec<Update>> {
        let mut params = GetUpdatesParams::builder()
            .timeout(30_u32)
            .allowed_updates(vec![AllowedUpdate::Message, AllowedUpdate::CallbackQuery])
            .build();

        if let Some(off) = offset {
            params.offset = Some(off);
        }

        let response = self.bot.get_updates(&params).await?;
        Ok(response.result)
    }

    async fn handle_update(&self, agent: &Agent, update: Update) {
        // Handle callback queries (inline button clicks)
        if let UpdateContent::CallbackQuery(callback) = &update.content {
            self.handle_callback(agent, callback).await;
            return;
        }

        let message = match update.content {
            UpdateContent::Message(msg) => msg,
            _ => return,
        };

        let chat_id = message.chat.id;
        let thread_id = message.message_thread_id;

        let username = message
            .from
            .as_ref()
            .and_then(|u| u.username.as_deref())
            .unwrap_or("");

        if !self.allowed_users.contains("*") && !self.allowed_users.contains(username) {
            tracing::warn!(username, chat_id, "unauthorized");
            return;
        }

        // Extract text and images from message
        let mut image_urls: Vec<String> = Vec::new();

        // Handle photos
        if let Some(ref photos) = message.photo {
            // Take the largest photo (last in array)
            if let Some(largest) = photos.last() {
                match self.download_file_as_base64(&largest.file_id).await {
                    Ok(b64) => {
                        image_urls.push(format!("data:image/jpeg;base64,{}", b64));
                        tracing::info!(chat_id, "photo received");
                    }
                    Err(e) => {
                        tracing::error!(error = %e, chat_id, "photo download failed");
                    }
                }
            }
        }

        let text = if let Some(ref t) = message.text {
            if t.is_empty() && image_urls.is_empty() {
                return;
            }
            t.clone()
        } else if let Some(ref caption) = message.caption {
            caption.clone()
        } else if let Some(ref voice) = message.voice {
            match self.transcribe_voice(&voice.file_id).await {
                Ok(t) => {
                    tracing::info!(chat_id, len = t.len(), "voice transcribed");
                    t
                }
                Err(e) => {
                    tracing::error!(error = %e, chat_id, "voice transcription failed");
                    self.send_final(chat_id, thread_id, "Не удалось распознать голосовое.")
                        .await;
                    return;
                }
            }
        } else if !image_urls.is_empty() {
            // Photo without caption — ask to describe
            "Что на этом изображении?".to_string()
        } else {
            return;
        };

        tracing::info!(username, chat_id, len = text.len(), "message");

        // Send typing indicator
        let mut action_params = SendChatActionParams::builder()
            .chat_id(chat_id)
            .action(ChatAction::Typing)
            .build();
        if let Some(tid) = thread_id {
            action_params.message_thread_id = Some(tid);
        }
        let _ = self.bot.send_chat_action(&action_params).await;

        let (delta_tx, delta_rx) = mpsc::channel::<String>(128);

        // Spawn draft-based streaming display (no placeholder needed)
        let bot = self.bot.clone();
        let throttle = self.stream_throttle;
        let stream_handle =
            tokio::spawn(Self::stream_to_telegram(bot, chat_id, thread_id, throttle, delta_rx));

        // Process message (drives delta_tx via LLM streaming)
        let result = agent
            .process_message(chat_id, thread_id, username, &text, &image_urls, delta_tx, &self.bot)
            .await;

        // Wait for streaming task
        let _ = stream_handle.await;

        // Send final message (replaces draft automatically)
        match result {
            Ok(ref final_text) => {
                let (clean_text, buttons) = Self::extract_buttons(final_text);
                self.send_final_with_buttons(chat_id, thread_id, &clean_text, buttons)
                    .await;

                // Background tasks: auto-name topic + extract memories
                agent
                    .maybe_name_topic(chat_id, thread_id, &text, final_text, &self.bot)
                    .await;

                agent.extract_memories(&text, final_text).await;
            }
            Err(e) => {
                tracing::error!(error = %e, chat_id, "agent error");
                self.send_final(chat_id, thread_id, "Произошла ошибка.")
                    .await;
            }
        }
    }

    /// Stream LLM output to Telegram using sendMessageDraft (Bot API 9.5).
    /// Shows text as a typing draft that the bot is composing — no placeholder message needed.
    async fn stream_to_telegram(
        bot: Bot,
        chat_id: i64,
        thread_id: Option<i32>,
        throttle: Duration,
        mut rx: mpsc::Receiver<String>,
    ) {
        // Generate a unique draft_id from current timestamp nanos
        let draft_id = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as i32)
            .wrapping_abs();

        let mut accumulated = String::new();
        let mut last_send = Instant::now() - throttle;
        let mut pending: Option<tokio::task::JoinHandle<()>> = None;

        while let Some(delta) = rx.recv().await {
            accumulated.push_str(&delta);

            if let Some(ref handle) = pending {
                if !handle.is_finished() {
                    continue;
                }
            }

            if accumulated.len() > 5 && last_send.elapsed() >= throttle {
                // Strip ```buttons block so user doesn't see raw JSON in draft
                let text = match accumulated.find("```buttons") {
                    Some(pos) => accumulated[..pos].trim_end().to_string(),
                    None => accumulated.clone(),
                };
                let bot_clone = bot.clone();
                pending = Some(tokio::spawn(async move {
                    let mut params = SendMessageDraftParams::builder()
                        .chat_id(chat_id)
                        .text(&text)
                        .draft_id(draft_id)
                        .build();
                    if let Some(tid) = thread_id {
                        params.message_thread_id = Some(tid);
                    }
                    if let Err(e) = bot_clone.send_message_draft(&params).await {
                        tracing::debug!(error = %e, "draft send failed");
                    }
                }));
                last_send = Instant::now();
            }
        }

        if let Some(handle) = pending {
            let _ = handle.await;
        }
    }

    async fn handle_callback(&self, agent: &Agent, callback: &frankenstein::types::CallbackQuery) {
        // Answer callback to remove loading spinner
        let answer = AnswerCallbackQueryParams::builder()
            .callback_query_id(&callback.id)
            .build();
        let _ = self.bot.answer_callback_query(&answer).await;

        let data = match callback.data.as_deref() {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => return,
        };

        let message = match &callback.message {
            Some(msg) => msg,
            None => return,
        };

        // Extract chat_id and thread_id from the original message
        let (chat_id, thread_id) = match message {
            frankenstein::types::MaybeInaccessibleMessage::Message(msg) => {
                (msg.chat.id, msg.message_thread_id)
            }
            frankenstein::types::MaybeInaccessibleMessage::InaccessibleMessage(msg) => {
                (msg.chat.id, None)
            }
        };

        let username = callback
            .from
            .username
            .as_deref()
            .unwrap_or("");

        if !self.allowed_users.contains("*") && !self.allowed_users.contains(username) {
            return;
        }

        tracing::info!(username, chat_id, data = %data, "button clicked");

        let (delta_tx, delta_rx) = mpsc::channel::<String>(128);

        let bot = self.bot.clone();
        let throttle = self.stream_throttle;
        let stream_handle =
            tokio::spawn(Self::stream_to_telegram(bot, chat_id, thread_id, throttle, delta_rx));

        let result = agent
            .process_message(chat_id, thread_id, username, &data, &[], delta_tx, &self.bot)
            .await;

        let _ = stream_handle.await;

        match result {
            Ok(ref final_text) => {
                let (clean_text, buttons) = Self::extract_buttons(final_text);
                self.send_final_with_buttons(chat_id, thread_id, &clean_text, buttons)
                    .await;
            }
            Err(e) => {
                tracing::error!(error = %e, chat_id, "agent error (callback)");
                self.send_final(chat_id, thread_id, "Произошла ошибка.")
                    .await;
            }
        }
    }

    async fn download_file_as_base64(&self, file_id: &str) -> Result<String> {
        use base64::Engine;

        let params = frankenstein::methods::GetFileParams::builder()
            .file_id(file_id)
            .build();
        let file_resp = self.bot.get_file(&params).await?;
        let file_path = file_resp
            .result
            .file_path
            .ok_or_else(|| anyhow::anyhow!("no file_path in response"))?;

        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        );
        let bytes = frankenstein::reqwest::get(&url).await?.bytes().await?;

        tracing::debug!(size = bytes.len(), "file downloaded for vision");
        Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
    }

    async fn transcribe_voice(&self, file_id: &str) -> Result<String> {
        let stt = self
            .stt
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("STT not configured (set GROQ_API_KEY)"))?;

        // Get file path from Telegram
        let params = frankenstein::methods::GetFileParams::builder()
            .file_id(file_id)
            .build();
        let file_resp = self.bot.get_file(&params).await?;
        let file_path = file_resp
            .result
            .file_path
            .ok_or_else(|| anyhow::anyhow!("no file_path in response"))?;

        // Download file
        let url = format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        );
        let bytes = frankenstein::reqwest::get(&url).await?.bytes().await?;

        tracing::debug!(size = bytes.len(), "voice file downloaded");

        // Transcribe
        stt.transcribe(bytes.to_vec(), "voice.ogg").await
    }

    /// Extract ```buttons JSON block from response text.
    /// Returns (clean_text, optional button rows).
    fn extract_buttons(text: &str) -> (String, Option<Vec<Vec<InlineKeyboardButton>>>) {
        // Find ```buttons ... ``` block
        let marker_start = "```buttons";
        let marker_end = "```";

        let start = match text.find(marker_start) {
            Some(pos) => pos,
            None => return (text.to_string(), None),
        };

        let json_start = start + marker_start.len();
        let end = match text[json_start..].find(marker_end) {
            Some(pos) => json_start + pos,
            None => return (text.to_string(), None),
        };

        let json_str = text[json_start..end].trim();
        let clean = format!("{}{}", text[..start].trim_end(), text[end + marker_end.len()..].trim_start());
        let clean = clean.trim().to_string();

        // Parse JSON: array of rows, each row is array of {label, data?, url?}
        let parsed: Vec<Vec<serde_json::Value>> = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => {
                // Try as flat array of buttons → single row
                match serde_json::from_str::<Vec<serde_json::Value>>(json_str) {
                    Ok(flat) => vec![flat],
                    Err(_) => return (text.to_string(), None),
                }
            }
        };

        let rows: Vec<Vec<InlineKeyboardButton>> = parsed
            .iter()
            .map(|row| {
                row.iter()
                    .filter_map(|btn| {
                        let label = btn["label"].as_str()?.to_string();
                        if let Some(url) = btn["url"].as_str() {
                            Some(InlineKeyboardButton::builder().text(label).url(url).build())
                        } else {
                            let data = btn["data"]
                                .as_str()
                                .unwrap_or(btn["label"].as_str().unwrap_or("?"));
                            let data = if data.len() > 64 { &data[..64] } else { data };
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
            .filter(|row: &Vec<InlineKeyboardButton>| !row.is_empty())
            .collect();

        if rows.is_empty() {
            (clean, None)
        } else {
            (clean, Some(rows))
        }
    }

    async fn send_final_with_buttons(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        text: &str,
        buttons: Option<Vec<Vec<InlineKeyboardButton>>>,
    ) {
        let mut params = SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::Html)
            .build();

        if let Some(tid) = thread_id {
            params.message_thread_id = Some(tid);
        }

        if let Some(rows) = buttons {
            let keyboard = InlineKeyboardMarkup::builder()
                .inline_keyboard(rows)
                .build();
            params.reply_markup = Some(ReplyMarkup::InlineKeyboardMarkup(keyboard));
        }

        if let Err(e) = self.bot.send_message(&params).await {
            tracing::error!(error = %e, chat_id, "send with buttons failed");
        }
    }

    async fn send_final(&self, chat_id: i64, thread_id: Option<i32>, text: &str) {
        let mut params = SendMessageParams::builder()
            .chat_id(chat_id)
            .text(text)
            .parse_mode(ParseMode::Html)
            .build();

        if let Some(tid) = thread_id {
            params.message_thread_id = Some(tid);
        }

        if let Err(e) = self.bot.send_message(&params).await {
            tracing::error!(error = %e, chat_id, "send failed");
        }
    }
}
