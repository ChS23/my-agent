use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use frankenstein::client_reqwest::Bot;
use frankenstein::methods::{
    EditMessageTextParams, GetUpdatesParams, SendChatActionParams, SendMessageParams,
};
use frankenstein::types::{AllowedUpdate, ChatAction};
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
            .allowed_updates(vec![AllowedUpdate::Message])
            .build();

        if let Some(off) = offset {
            params.offset = Some(off);
        }

        let response = self.bot.get_updates(&params).await?;
        Ok(response.result)
    }

    async fn handle_update(&self, agent: &Agent, update: Update) {
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

        // Send placeholder message for streaming edits
        let mut placeholder = SendMessageParams::builder()
            .chat_id(chat_id)
            .text("⏳")
            .build();
        if let Some(tid) = thread_id {
            placeholder.message_thread_id = Some(tid);
        }

        let msg_id = match self.bot.send_message(&placeholder).await {
            Ok(resp) => Some(resp.result.message_id),
            Err(_) => None,
        };

        // Spawn streaming display
        let bot = self.bot.clone();
        let throttle = self.stream_throttle;
        let stream_handle =
            tokio::spawn(Self::stream_to_telegram(bot, chat_id, msg_id, throttle, delta_rx));

        // Process message (drives delta_tx via LLM streaming)
        let result = agent
            .process_message(chat_id, thread_id, &text, &image_urls, delta_tx, &self.bot)
            .await;

        // Wait for streaming task
        let _ = stream_handle.await;

        // Edit with final text (or send new if no placeholder)
        match result {
            Ok(ref final_text) => {
                if let Some(mid) = msg_id {
                    self.edit_message(chat_id, mid, final_text).await;
                } else {
                    self.send_final(chat_id, thread_id, final_text).await;
                }

                // Background tasks: auto-name topic + extract memories
                let text_clone = text.clone();
                let final_clone = final_text.clone();
                agent
                    .maybe_name_topic(
                        chat_id,
                        thread_id,
                        &text_clone,
                        &final_clone,
                        &self.bot,
                    )
                    .await;

                agent.extract_memories(&text, final_text).await;
            }
            Err(e) => {
                tracing::error!(error = %e, chat_id, "agent error");
                let err_text = "Произошла ошибка.";
                if let Some(mid) = msg_id {
                    self.edit_message(chat_id, mid, err_text).await;
                } else {
                    self.send_final(chat_id, thread_id, err_text).await;
                }
            }
        }
    }

    async fn stream_to_telegram(
        bot: Bot,
        chat_id: i64,
        msg_id: Option<i32>,
        throttle: Duration,
        mut rx: mpsc::Receiver<String>,
    ) {
        let msg_id = match msg_id {
            Some(id) => id,
            None => {
                // Drain the channel
                while rx.recv().await.is_some() {}
                return;
            }
        };

        let mut accumulated = String::new();
        let mut last_edit = Instant::now() - throttle; // allow immediate first edit
        let mut pending_edit: Option<tokio::task::JoinHandle<()>> = None;

        while let Some(delta) = rx.recv().await {
            accumulated.push_str(&delta);

            // Check if previous edit is done
            if let Some(ref handle) = pending_edit {
                if !handle.is_finished() {
                    continue; // still editing, skip
                }
            }

            if accumulated.len() > 5 && last_edit.elapsed() >= throttle {
                let display = format!("{}▍", &accumulated);
                let bot_clone = bot.clone();
                pending_edit = Some(tokio::spawn(async move {
                    let params = EditMessageTextParams::builder()
                        .chat_id(chat_id)
                        .message_id(msg_id)
                        .text(&display)
                        .build();
                    if let Err(e) = bot_clone.edit_message_text(&params).await {
                        tracing::debug!(error = %e, "stream edit failed");
                    }
                }));
                last_edit = Instant::now();
            }
        }

        // Wait for last pending edit
        if let Some(handle) = pending_edit {
            let _ = handle.await;
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

    async fn edit_message(&self, chat_id: i64, message_id: i32, text: &str) {
        let params = EditMessageTextParams::builder()
            .chat_id(chat_id)
            .message_id(message_id)
            .text(text)
            .parse_mode(ParseMode::Html)
            .build();

        if let Err(e) = self.bot.edit_message_text(&params).await {
            tracing::error!(error = %e, chat_id, "edit failed");
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
