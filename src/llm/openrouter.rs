use anyhow::Result;
use async_openai::{
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
        ChatCompletionRequestSystemMessage, ChatCompletionRequestUserMessage,
        CreateChatCompletionRequest, FinishReason,
    },
    Client,
};
use futures::StreamExt;
use tokio::sync::mpsc;

/// A tool call assembled from streaming chunks.
#[derive(Debug, Clone)]
pub struct StreamedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Result of a streaming chat request.
pub struct StreamResult {
    pub text: String,
    pub tool_calls: Vec<StreamedToolCall>,
}

struct LlmParams {
    model: String,
    temperature: f32,
    max_tokens: u32,
}

pub struct LlmClient {
    client: Client<OpenAIConfig>,
    params: std::sync::RwLock<LlmParams>,
}

impl LlmClient {
    pub fn new(api_key: &str, api_base: &str, model: &str, temperature: f32, max_tokens: u32) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(api_base);

        Self {
            client: Client::with_config(config),
            params: std::sync::RwLock::new(LlmParams {
                model: model.to_string(),
                temperature,
                max_tokens,
            }),
        }
    }

    pub fn model(&self) -> String {
        self.params.read().unwrap().model.clone()
    }

    pub fn temperature(&self) -> f32 {
        self.params.read().unwrap().temperature
    }

    pub fn max_tokens(&self) -> u32 {
        self.params.read().unwrap().max_tokens
    }

    /// Hot-swap model parameters at runtime.
    pub fn set_model(&self, model: &str, temperature: Option<f32>, max_tokens: Option<u32>) {
        let mut params = self.params.write().unwrap();
        params.model = model.to_string();
        if let Some(t) = temperature {
            params.temperature = t;
        }
        if let Some(m) = max_tokens {
            params.max_tokens = m;
        }
        tracing::info!(model, "LLM model hot-swapped");
    }

    /// Get current settings as a summary string.
    pub fn current_settings(&self) -> String {
        let p = self.params.read().unwrap();
        format!(
            "model={}, temperature={}, max_tokens={}",
            p.model, p.temperature, p.max_tokens
        )
    }

    /// Stream a chat completion. Sends text deltas to `delta_tx`.
    /// Collects tool calls from the stream. Returns text + tool calls.
    pub async fn stream_chat(
        &self,
        request: CreateChatCompletionRequest,
        delta_tx: mpsc::Sender<String>,
    ) -> Result<StreamResult> {
        let mut stream = self.client.chat().create_stream(request).await?;
        let mut text = String::new();
        let mut tool_calls: Vec<StreamedToolCall> = Vec::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(response) => {
                    for choice in &response.choices {
                        // Collect text deltas
                        if let Some(ref delta) = choice.delta.content {
                            text.push_str(delta);
                            let _ = delta_tx.send(delta.clone()).await;
                        }

                        // Collect tool call chunks
                        if let Some(ref tc_chunks) = choice.delta.tool_calls {
                            for chunk in tc_chunks {
                                let idx = chunk.index as usize;

                                // Grow vec if needed
                                while tool_calls.len() <= idx {
                                    tool_calls.push(StreamedToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });
                                }

                                if let Some(ref id) = chunk.id {
                                    tool_calls[idx].id = id.clone();
                                }
                                if let Some(ref func) = chunk.function {
                                    if let Some(ref name) = func.name {
                                        tool_calls[idx].name = name.clone();
                                    }
                                    if let Some(ref args) = func.arguments {
                                        tool_calls[idx].arguments.push_str(args);
                                    }
                                }
                            }
                        }

                        // Log finish reason
                        if let Some(ref reason) = choice.finish_reason {
                            tracing::debug!(?reason, "stream finished");
                            if *reason == FinishReason::ToolCalls {
                                tracing::debug!(count = tool_calls.len(), "tool calls in stream");
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "LLM stream error");
                    return Err(e.into());
                }
            }
        }

        // Filter out empty tool calls
        tool_calls.retain(|tc| !tc.name.is_empty());

        Ok(StreamResult { text, tool_calls })
    }
}

/// Build the messages array for the LLM.
pub fn build_messages(
    system_prompt: &str,
    history: &[crate::memory::store::ChatMessage],
    user_message: &str,
) -> Vec<ChatCompletionRequestMessage> {
    let mut messages = Vec::with_capacity(history.len() + 2);

    messages.push(ChatCompletionRequestMessage::System(
        ChatCompletionRequestSystemMessage::from(system_prompt),
    ));

    for msg in history {
        match msg.role.as_str() {
            "user" => {
                messages.push(ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessage::from(msg.content.as_str()),
                ));
            }
            "assistant" => {
                messages.push(ChatCompletionRequestMessage::Assistant(
                    ChatCompletionRequestAssistantMessage::from(msg.content.as_str()),
                ));
            }
            "system" => {
                // Compressed history summary — inject as user context
                messages.push(ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessage::from(
                        format!("[Previous conversation summary]\n{}", msg.content).as_str(),
                    ),
                ));
            }
            _ => {}
        }
    }

    messages.push(ChatCompletionRequestMessage::User(
        ChatCompletionRequestUserMessage::from(user_message),
    ));

    messages
}
