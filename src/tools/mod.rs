mod buttons;
mod memory;
mod model;
mod schedule;
mod ticktick;
mod topics;
mod url_reader;
mod web_search;

pub use buttons::SendButtonsTool;
pub use memory::MemoryExportTool;
pub use memory::MemoryForgetTool;
pub use memory::MemorySearchTool;
pub use memory::MemoryStoreTool;
pub use schedule::{ScheduleAddTool, ScheduleCancelTool, ScheduleListTool};
pub use ticktick::{TickTickAuthTool, TickTickCompleteTool, TickTickCreateTool, TickTickDeleteTool, TickTickListTool};
pub use topics::{CloseTopicTool, CreateTopicTool, DeleteTopicTool, RenameTopicTool, ReopenTopicTool};
pub use model::{GetModelTool, SetModelTool};
pub use url_reader::UrlReaderTool;
pub use web_search::WebSearchTool;

use anyhow::Result;
use async_openai::types::chat::ChatCompletionTools;
use frankenstein::client_reqwest::Bot;

use crate::llm::{EmbeddingClient, LlmClient};
use crate::memory::MemoryStore;
use crate::scheduler::store::ScheduleStore;
use crate::ticktick::TickTickClient;

pub struct ToolResult {
    pub output: String,
}

/// Context passed to all tool executions.
pub struct ToolContext<'a> {
    pub store: &'a MemoryStore,
    pub schedule_store: &'a ScheduleStore,
    pub bot: &'a Bot,
    pub chat_id: i64,
    pub thread_id: Option<i32>,
    pub llm: &'a LlmClient,
    pub embeddings: Option<&'a EmbeddingClient>,
    pub ticktick: Option<&'a TickTickClient>,
}

/// Return OpenAI-format tool specs for all available tools.
pub fn tool_specs(has_ticktick: bool) -> Vec<ChatCompletionTools> {
    let mut specs = vec![
        MemoryStoreTool::spec(),
        MemoryForgetTool::spec(),
        MemorySearchTool::spec(),
        MemoryExportTool::spec(),
        WebSearchTool::spec(),
        ScheduleAddTool::spec(),
        ScheduleListTool::spec(),
        ScheduleCancelTool::spec(),
        CreateTopicTool::spec(),
        RenameTopicTool::spec(),
        CloseTopicTool::spec(),
        ReopenTopicTool::spec(),
        DeleteTopicTool::spec(),
        SetModelTool::spec(),
        GetModelTool::spec(),
        UrlReaderTool::spec(),
        SendButtonsTool::spec(),
    ];

    if has_ticktick {
        specs.push(TickTickAuthTool::spec());
        specs.push(TickTickCreateTool::spec());
        specs.push(TickTickListTool::spec());
        specs.push(TickTickCompleteTool::spec());
        specs.push(TickTickDeleteTool::spec());
    }

    specs
}

/// Execute a tool by name with JSON arguments.
pub async fn execute_tool(
    name: &str,
    arguments: &str,
    ctx: &ToolContext<'_>,
) -> Result<ToolResult> {
    match name {
        "memory_store" => MemoryStoreTool::execute(arguments, ctx.store, ctx.embeddings).await,
        "memory_forget" => MemoryForgetTool::execute(arguments, ctx.store).await,
        "memory_search" => {
            MemorySearchTool::execute(arguments, ctx.store, ctx.chat_id, ctx.embeddings).await
        }
        "memory_export" => MemoryExportTool::execute(ctx.store).await,
        "web_search" => WebSearchTool::execute(arguments).await,
        "schedule_add" => {
            ScheduleAddTool::execute(arguments, ctx.schedule_store, ctx.chat_id, ctx.thread_id)
                .await
        }
        "schedule_list" => ScheduleListTool::execute(ctx.schedule_store, ctx.chat_id).await,
        "schedule_cancel" => {
            ScheduleCancelTool::execute(arguments, ctx.schedule_store, ctx.chat_id).await
        }
        "ticktick_auth" => {
            let client = ctx.ticktick.ok_or_else(|| anyhow::anyhow!("TickTick not configured"))?;
            TickTickAuthTool::execute(client).await
        }
        "ticktick_create" => {
            let client = ctx.ticktick.ok_or_else(|| anyhow::anyhow!("TickTick not configured"))?;
            TickTickCreateTool::execute(arguments, client).await
        }
        "ticktick_list" => {
            let client = ctx.ticktick.ok_or_else(|| anyhow::anyhow!("TickTick not configured"))?;
            TickTickListTool::execute(arguments, client).await
        }
        "ticktick_complete" => {
            let client = ctx.ticktick.ok_or_else(|| anyhow::anyhow!("TickTick not configured"))?;
            TickTickCompleteTool::execute(arguments, client).await
        }
        "ticktick_delete" => {
            let client = ctx.ticktick.ok_or_else(|| anyhow::anyhow!("TickTick not configured"))?;
            TickTickDeleteTool::execute(arguments, client).await
        }
        "create_topic" => CreateTopicTool::execute(arguments, ctx).await,
        "rename_topic" => RenameTopicTool::execute(arguments, ctx).await,
        "close_topic" => CloseTopicTool::execute(arguments, ctx).await,
        "reopen_topic" => ReopenTopicTool::execute(arguments, ctx).await,
        "delete_topic" => DeleteTopicTool::execute(arguments, ctx).await,
        "set_model" => SetModelTool::execute(arguments, ctx.llm).await,
        "get_model" => GetModelTool::execute(ctx.llm).await,
        "read_url" => UrlReaderTool::execute(arguments).await,
        "send_buttons" => {
            SendButtonsTool::execute(arguments, ctx.bot, ctx.chat_id, ctx.thread_id).await
        }
        _ => Ok(ToolResult {
            output: format!("Unknown tool: {name}"),
        }),
    }
}
