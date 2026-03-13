use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestToolMessage, ChatCompletionRequestUserMessage,
    CreateChatCompletionRequestArgs, FunctionCall,
};
use frankenstein::client_reqwest::Bot;
use tokio::sync::mpsc;

use crate::config::AgentConfig;
use crate::llm::{EmbeddingClient, LlmClient};
use crate::memory::MemoryStore;
use crate::scheduler::store::ScheduleStore;
use crate::ticktick::TickTickClient;
use crate::tools::ToolContext;
use frankenstein::methods::EditForumTopicParams;
use frankenstein::AsyncTelegramApi;

pub struct Agent {
    llm: LlmClient,
    embeddings: Option<EmbeddingClient>,
    memory: MemoryStore,
    schedule_store: ScheduleStore,
    ticktick: Option<TickTickClient>,
    identity: String,
    config: AgentConfig,
}

impl Agent {
    pub fn new(
        llm: LlmClient,
        embeddings: Option<EmbeddingClient>,
        memory: MemoryStore,
        schedule_store: ScheduleStore,
        ticktick: Option<TickTickClient>,
        identity: String,
        config: AgentConfig,
    ) -> Self {
        Self {
            llm,
            embeddings,
            memory,
            schedule_store,
            ticktick,
            identity,
            config,
        }
    }

    async fn build_system_prompt(&self) -> Result<String> {
        let memories = self.memory.load_all_memories().await?;
        let mut prompt = self.identity.clone();

        if !memories.is_empty() {
            prompt.push_str("\n\n## What you know about the user\n\n");
            for m in &memories {
                prompt.push_str(&format!("- **{}** ({}): {}\n", m.key, m.category, m.content));
            }
        }

        // Use configured timezone
        let now = chrono::Utc::now();
        let tz: chrono_tz::Tz = self.config.timezone.parse().unwrap_or(chrono_tz::Tz::UTC);
        let local = now.with_timezone(&tz);
        prompt.push_str(&format!(
            "\n\nCurrent time: {}\n",
            local.format("%Y-%m-%d %H:%M:%S %Z")
        ));

        Ok(prompt)
    }

    /// Process a user message. Streams text deltas through `delta_tx`.
    /// Handles tool calls in a loop. Returns final assistant text.
    pub async fn process_message(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        user_message: &str,
        image_urls: &[String],
        delta_tx: mpsc::Sender<String>,
        bot: &Bot,
    ) -> Result<String> {
        let system_prompt = self.build_system_prompt().await?;

        // Compress old messages if history is too long
        if let Err(e) = self.maybe_compress_history(chat_id, thread_id).await {
            tracing::warn!(error = %e, "history compression error");
        }

        let history = self
            .memory
            .load_history(chat_id, thread_id, self.config.max_history_messages)
            .await?;

        self.memory
            .save_message(chat_id, thread_id, "user", user_message)
            .await?;

        let mut messages =
            crate::llm::openrouter::build_messages(&system_prompt, &history, user_message, image_urls);

        let tool_specs = crate::tools::tool_specs(self.ticktick.is_some());

        for iteration in 0..self.config.max_tool_iterations {
            let mut builder = CreateChatCompletionRequestArgs::default();
            builder
                .model(self.llm.model())
                .temperature(self.llm.temperature())
                .max_tokens(self.llm.max_tokens())
                .messages(messages.clone());

            if !tool_specs.is_empty() {
                builder.tools(tool_specs.clone());
            }

            let request = builder.build()?;

            // Always stream — collect text + tool calls from stream
            let result = self.llm.stream_chat(request, delta_tx.clone()).await?;

            if result.tool_calls.is_empty() {
                // No tool calls — we're done, text was already streamed
                self.memory
                    .save_message(chat_id, thread_id, "assistant", &result.text)
                    .await?;

                tracing::info!(chat_id, iterations = iteration + 1, len = result.text.len(), "done");
                return Ok(result.text);
            }

            // Has tool calls — process them
            tracing::info!(
                iteration,
                tools = ?result.tool_calls.iter().map(|c| &c.name).collect::<Vec<_>>(),
                "tool calls"
            );

            // Build tool_calls vec for assistant message
            let tc_for_msg: Vec<ChatCompletionMessageToolCalls> = result
                .tool_calls
                .iter()
                .map(|tc| {
                    ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                        id: tc.id.clone(),
                        function: FunctionCall {
                            name: tc.name.clone(),
                            arguments: tc.arguments.clone(),
                        },
                    })
                })
                .collect();

            // Add assistant message with tool calls
            messages.push(ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessage {
                    content: if result.text.is_empty() {
                        None
                    } else {
                        Some(result.text.into())
                    },
                    tool_calls: Some(tc_for_msg),
                    ..Default::default()
                },
            ));

            // Execute tools and add results
            let tool_ctx = ToolContext {
                store: &self.memory,
                schedule_store: &self.schedule_store,
                bot,
                chat_id,
                thread_id,
                llm: &self.llm,
                embeddings: self.embeddings.as_ref(),
                ticktick: self.ticktick.as_ref(),
            };
            for tc in &result.tool_calls {
                let tool_result = crate::tools::execute_tool(
                    &tc.name,
                    &tc.arguments,
                    &tool_ctx,
                )
                .await;

                let output = match tool_result {
                    Ok(r) => r.output,
                    Err(e) => {
                        tracing::warn!(tool = %tc.name, error = %e, "tool failed");
                        format!("Error: {e}")
                    }
                };

                messages.push(ChatCompletionRequestMessage::Tool(
                    ChatCompletionRequestToolMessage {
                        tool_call_id: tc.id.clone(),
                        content: output.into(),
                    },
                ));
            }
        }

        anyhow::bail!(
            "Exceeded max tool iterations ({})",
            self.config.max_tool_iterations
        )
    }

    /// Compress old messages into a summary when history exceeds threshold.
    /// Keeps the last `keep` messages intact, summarizes the rest.
    async fn maybe_compress_history(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
    ) -> Result<()> {
        let threshold = self.config.max_history_messages;
        let keep = threshold / 2; // keep recent half, compress older half

        let all = self
            .memory
            .load_history(chat_id, thread_id, threshold + 10)
            .await?;

        if all.len() <= threshold {
            return Ok(());
        }

        // Check if first message is already a summary
        if all.first().map(|m| m.role.as_str()) == Some("system") {
            // Already has a summary, check if we need to re-compress
            if all.len() <= threshold {
                return Ok(());
            }
        }

        let to_compress = &all[..all.len() - keep];

        // Build conversation text for summarization
        let mut conversation = String::new();
        for msg in to_compress {
            let role = match msg.role.as_str() {
                "system" => "Previous summary",
                "user" => "User",
                "assistant" => "Assistant",
                _ => &msg.role,
            };
            conversation.push_str(&format!("{}: {}\n\n", role, msg.content));
        }

        let prompt = format!(
            "Summarize this conversation concisely, preserving all important context, \
             decisions, and facts. Write in the same language as the conversation.\n\n\
             {}\n\n\
             Reply with ONLY the summary, no preamble.",
            conversation
        );

        let messages = vec![ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage::from(prompt.as_str()),
        )];

        let request = match CreateChatCompletionRequestArgs::default()
            .model(self.llm.model())
            .temperature(0.3_f32)
            .max_tokens(500_u32)
            .messages(messages)
            .build()
        {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };

        let (tx, mut rx) = mpsc::channel::<String>(32);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let summary = match self.llm.stream_chat(request, tx).await {
            Ok(r) => r.text,
            Err(e) => {
                tracing::warn!(error = %e, "history compression failed");
                return Ok(());
            }
        };

        if summary.is_empty() {
            return Ok(());
        }

        // Delete old messages and insert summary
        let compressed_count = to_compress.len();
        self.memory
            .compress_messages(chat_id, thread_id, compressed_count, &summary)
            .await?;

        tracing::info!(
            chat_id,
            compressed = compressed_count,
            summary_len = summary.len(),
            "history compressed"
        );

        Ok(())
    }

    /// Background memory extraction (Mem0 pattern).
    /// Analyzes the exchange and auto-stores important facts.
    pub async fn extract_memories(
        &self,
        user_message: &str,
        assistant_response: &str,
    ) {
        let memories = match self.memory.load_all_memories().await {
            Ok(m) => m,
            Err(_) => return,
        };

        let existing = if memories.is_empty() {
            "No existing memories.".to_string()
        } else {
            memories
                .iter()
                .map(|m| format!("- {} ({}): {}", m.key, m.category, m.content))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let prompt = format!(
            "Analyze this conversation exchange and extract important facts about the user that should be remembered.\n\n\
             Existing memories:\n{existing}\n\n\
             User: {user}\n\
             Assistant: {assistant}\n\n\
             Return a JSON array of actions. Each action is one of:\n\
             - {{\"action\": \"store\", \"key\": \"...\", \"content\": \"...\", \"category\": \"core|preference|decision\"}}\n\
             - {{\"action\": \"delete\", \"key\": \"...\"}}\n\n\
             Rules:\n\
             - Only extract genuinely important, long-term facts (name, preferences, habits, decisions)\n\
             - Do NOT store transient info (current question, temporary context)\n\
             - Update existing memories if new info refines them\n\
             - Delete memories that are contradicted by new info\n\
             - Return [] if nothing worth remembering\n\
             - Reply with ONLY the JSON array, nothing else",
            user = truncate_str(user_message, 500),
            assistant = truncate_str(assistant_response, 500),
        );

        let messages = vec![
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage::from(prompt.as_str()),
            ),
        ];

        let request = match CreateChatCompletionRequestArgs::default()
            .model(self.llm.model())
            .temperature(0.3_f32)
            .max_tokens(500_u32)
            .messages(messages)
            .build()
        {
            Ok(r) => r,
            Err(_) => return,
        };

        let (tx, mut rx) = mpsc::channel::<String>(32);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let result = match self.llm.stream_chat(request, tx).await {
            Ok(r) => r.text,
            Err(e) => {
                tracing::debug!(error = %e, "memory extraction failed");
                return;
            }
        };

        // Parse JSON array from response
        let text = result.trim().trim_start_matches("```json").trim_end_matches("```").trim();
        let actions: Vec<serde_json::Value> = match serde_json::from_str(text) {
            Ok(a) => a,
            Err(_) => return,
        };

        for action in &actions {
            match action["action"].as_str() {
                Some("store") => {
                    let key = action["key"].as_str().unwrap_or_default();
                    let content = action["content"].as_str().unwrap_or_default();
                    let category = action["category"].as_str().unwrap_or("core");
                    if !key.is_empty() && !content.is_empty() {
                        if let Err(e) = self.memory.store_memory(key, content, category).await {
                            tracing::debug!(error = %e, key, "auto-store failed");
                        } else {
                            tracing::info!(key, "memory auto-extracted");
                        }
                    }
                }
                Some("delete") => {
                    let key = action["key"].as_str().unwrap_or_default();
                    if !key.is_empty() {
                        let _ = self.memory.forget_memory(key).await;
                        tracing::info!(key, "memory auto-deleted");
                    }
                }
                _ => {}
            }
        }
    }

    /// Auto-name a forum topic after the first exchange.
    /// Generates a short title with emoji based on the conversation.
    pub async fn maybe_name_topic(
        &self,
        chat_id: i64,
        thread_id: Option<i32>,
        user_message: &str,
        assistant_response: &str,
        bot: &Bot,
    ) {
        let tid = match thread_id {
            Some(tid) => tid,
            None => return, // Not a thread
        };

        // Check if this is the first message in the thread
        let history_count = self
            .memory
            .load_history(chat_id, thread_id, 5)
            .await
            .map(|h| h.len())
            .unwrap_or(0);

        // Only name on first exchange (user + assistant = 2 messages)
        if history_count > 2 {
            return;
        }

        // Generate topic name via LLM
        let prompt = format!(
            "Generate a very short forum topic name (max 4 words) with one fitting emoji at the start, based on this conversation.\n\
             User: {}\n\
             Assistant: {}\n\n\
             Reply with ONLY the topic name, nothing else. Example: \"🛒 Список покупок\" or \"🦀 Rust async вопрос\"",
            truncate_str(user_message, 200),
            truncate_str(assistant_response, 200),
        );

        let messages = vec![
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage::from(prompt.as_str()),
            ),
        ];

        let request = match CreateChatCompletionRequestArgs::default()
            .model(self.llm.model())
            .temperature(0.7_f32)
            .max_tokens(50_u32)
            .messages(messages)
            .build()
        {
            Ok(r) => r,
            Err(_) => return,
        };

        // Use a throwaway channel
        let (tx, mut rx) = mpsc::channel::<String>(32);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let result = self.llm.stream_chat(request, tx).await;
        let topic_name = match result {
            Ok(r) => r.text.trim().trim_matches('"').to_string(),
            Err(e) => {
                tracing::debug!(error = %e, "topic naming failed");
                return;
            }
        };

        if topic_name.is_empty() || topic_name.len() > 128 {
            return;
        }

        // Extract emoji (first char if it's emoji) for icon
        let icon = topic_name.chars().next().filter(|c| !c.is_ascii());

        let params = EditForumTopicParams::builder()
            .chat_id(chat_id)
            .message_thread_id(tid)
            .name(&topic_name)
            .build();

        match bot.edit_forum_topic(&params).await {
            Ok(_) => tracing::info!(chat_id, thread_id = tid, name = %topic_name, "topic named"),
            Err(e) => tracing::debug!(error = %e, "topic rename failed"),
        }

        // Set icon if we extracted one
        if let Some(emoji) = icon {
            let icon_params = EditForumTopicParams::builder()
                .chat_id(chat_id)
                .message_thread_id(tid)
                .icon_custom_emoji_id(&emoji.to_string())
                .build();
            let _ = bot.edit_forum_topic(&icon_params).await;
        }
    }
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}
