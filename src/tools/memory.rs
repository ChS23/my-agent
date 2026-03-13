use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};

use super::ToolResult;
use crate::llm::EmbeddingClient;
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

    pub async fn execute(
        arguments: &str,
        store: &MemoryStore,
        embeddings: Option<&EmbeddingClient>,
    ) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let key = args["key"].as_str().unwrap_or("unknown");
        let content = args["content"].as_str().unwrap_or("");
        let category = args["category"].as_str().unwrap_or("core");

        store.store_memory(key, content, category).await?;

        // Generate and store embedding in background
        if let Some(emb_client) = embeddings {
            let embed_text = format!("{}: {}", key, content);
            match emb_client.embed(&embed_text).await {
                Ok(vec) => {
                    if let Err(e) = store.save_embedding(key, &vec).await {
                        tracing::warn!(error = %e, key, "failed to save embedding");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, key, "failed to generate embedding");
                }
            }
        }

        Ok(ToolResult {
            output: format!("Stored: {key} = {content}"),
        })
    }
}

pub struct MemorySearchTool;

impl MemorySearchTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "memory_search".into(),
                description: Some(
                    "Search stored memories and past conversations using full-text search. \
                     Use when looking for specific facts, past discussions, or context."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (supports FTS5 syntax: AND, OR, NOT, prefix*)"
                        },
                        "scope": {
                            "type": "string",
                            "enum": ["memories", "messages", "all"],
                            "description": "Where to search: memories (stored facts), messages (chat history), or all"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max results (default 10)"
                        }
                    },
                    "required": ["query"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(
        arguments: &str,
        store: &MemoryStore,
        chat_id: i64,
        embeddings: Option<&EmbeddingClient>,
    ) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let query = args["query"].as_str().unwrap_or("");
        let scope = args["scope"].as_str().unwrap_or("all");
        let limit = args["limit"].as_u64().unwrap_or(10) as usize;

        if query.is_empty() {
            return Ok(ToolResult {
                output: "Error: query is required".into(),
            });
        }

        let mut output = String::new();

        if scope == "memories" || scope == "all" {
            // FTS5 keyword search
            let fts_results = store.search_memories(query, limit).await.unwrap_or_default();

            // Semantic search via embeddings
            let semantic_results = if let Some(emb_client) = embeddings {
                match emb_client.embed(query).await {
                    Ok(query_vec) => store
                        .search_by_embedding(&query_vec, limit)
                        .await
                        .unwrap_or_default(),
                    Err(e) => {
                        tracing::debug!(error = %e, "semantic search failed");
                        vec![]
                    }
                }
            } else {
                vec![]
            };

            // Merge: deduplicate by key
            let mut seen = std::collections::HashSet::new();
            let mut merged = Vec::new();

            for m in &fts_results {
                if seen.insert(m.key.clone()) {
                    merged.push(format!("- [{}] {}: {}", m.category, m.key, m.content));
                }
            }

            for (m, score) in &semantic_results {
                if *score > 0.5 && seen.insert(m.key.clone()) {
                    merged.push(format!(
                        "- [{}] {}: {} (similarity: {:.0}%)",
                        m.category, m.key, m.content, score * 100.0
                    ));
                }
            }

            if merged.is_empty() {
                output.push_str("No memories found.\n");
            } else {
                output.push_str(&format!("Found {} memories:\n", merged.len()));
                for line in &merged {
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }

        if scope == "messages" || scope == "all" {
            let messages = store.search_messages(query, Some(chat_id), limit).await?;
            if messages.is_empty() {
                if scope == "messages" {
                    output.push_str("No messages found.\n");
                }
            } else {
                output.push_str(&format!("\nFound {} messages:\n", messages.len()));
                for m in &messages {
                    let preview = if m.content.len() > 150 {
                        format!("{}...", &m.content[..150])
                    } else {
                        m.content.clone()
                    };
                    output.push_str(&format!("- [{}] {}: {}\n", m.timestamp, m.role, preview));
                }
            }
        }

        Ok(ToolResult { output })
    }
}

pub struct MemoryExportTool;

impl MemoryExportTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "memory_export".into(),
                description: Some(
                    "Export all stored memories as a formatted snapshot. \
                     Use when the user asks to see everything you know about them, \
                     or for backup purposes."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {}
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(store: &MemoryStore) -> Result<ToolResult> {
        let memories = store.load_all_memories().await?;

        if memories.is_empty() {
            return Ok(ToolResult {
                output: "No memories stored yet.".into(),
            });
        }

        let mut output = format!("# Memory Snapshot\n\nTotal: {} memories\n\n", memories.len());

        // Group by category
        let mut by_category: std::collections::BTreeMap<String, Vec<&crate::memory::store::CoreMemory>> =
            std::collections::BTreeMap::new();
        for m in &memories {
            by_category.entry(m.category.clone()).or_default().push(m);
        }

        for (category, items) in &by_category {
            output.push_str(&format!("## {}\n\n", category));
            for m in items {
                output.push_str(&format!("- **{}**: {}\n", m.key, m.content));
            }
            output.push('\n');
        }

        Ok(ToolResult { output })
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
