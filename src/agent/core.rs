use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestToolMessage, CreateChatCompletionRequestArgs, FunctionCall,
};
use frankenstein::client_reqwest::Bot;
use tokio::sync::mpsc;

use crate::config::AgentConfig;
use crate::llm::LlmClient;
use crate::memory::MemoryStore;
use crate::tools::ToolContext;

pub struct Agent {
    llm: LlmClient,
    memory: MemoryStore,
    identity: String,
    config: AgentConfig,
}

impl Agent {
    pub fn new(
        llm: LlmClient,
        memory: MemoryStore,
        identity: String,
        config: AgentConfig,
    ) -> Self {
        Self {
            llm,
            memory,
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

        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %Z");
        prompt.push_str(&format!("\n\nCurrent time: {now}\n"));

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

        let tool_specs = crate::tools::tool_specs();

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
                bot,
                chat_id,
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
}
