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
use crate::llm::LlmClient;
use crate::memory::MemoryStore;
use crate::scheduler::store::ScheduleStore;
use crate::ticktick::TickTickClient;
use crate::tools::ToolContext;
use frankenstein::methods::EditForumTopicParams;
use frankenstein::AsyncTelegramApi;

pub struct Agent {
    llm: LlmClient,
    memory: MemoryStore,
    schedule_store: ScheduleStore,
    ticktick: Option<TickTickClient>,
    identity: String,
    config: AgentConfig,
}

impl Agent {
    pub fn new(
        llm: LlmClient,
        memory: MemoryStore,
        schedule_store: ScheduleStore,
        ticktick: Option<TickTickClient>,
        identity: String,
        config: AgentConfig,
    ) -> Self {
        Self {
            llm,
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
        delta_tx: mpsc::Sender<String>,
        bot: &Bot,
    ) -> Result<String> {
        let system_prompt = self.build_system_prompt().await?;
        let history = self
            .memory
            .load_history(chat_id, thread_id, self.config.max_history_messages)
            .await?;

        self.memory
            .save_message(chat_id, thread_id, "user", user_message)
            .await?;

        let mut messages =
            crate::llm::openrouter::build_messages(&system_prompt, &history, user_message);

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
