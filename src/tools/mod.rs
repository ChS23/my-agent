mod memory;
mod schedule;
mod topics;
mod web_search;

pub use memory::MemoryForgetTool;
pub use memory::MemoryStoreTool;
pub use schedule::{ScheduleAddTool, ScheduleCancelTool, ScheduleListTool};
pub use topics::{CloseTopicTool, CreateTopicTool, DeleteTopicTool, RenameTopicTool, ReopenTopicTool};
pub use web_search::WebSearchTool;

use anyhow::Result;
use async_openai::types::chat::ChatCompletionTools;
use frankenstein::client_reqwest::Bot;

use crate::memory::MemoryStore;
use crate::scheduler::store::ScheduleStore;

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
}

/// Return OpenAI-format tool specs for all available tools.
pub fn tool_specs() -> Vec<ChatCompletionTools> {
    vec![
        MemoryStoreTool::spec(),
        MemoryForgetTool::spec(),
        WebSearchTool::spec(),
        ScheduleAddTool::spec(),
        ScheduleListTool::spec(),
        ScheduleCancelTool::spec(),
        CreateTopicTool::spec(),
        RenameTopicTool::spec(),
        CloseTopicTool::spec(),
        ReopenTopicTool::spec(),
        DeleteTopicTool::spec(),
    ]
}

/// Execute a tool by name with JSON arguments.
pub async fn execute_tool(
    name: &str,
    arguments: &str,
    ctx: &ToolContext<'_>,
) -> Result<ToolResult> {
    match name {
        "memory_store" => MemoryStoreTool::execute(arguments, ctx.store).await,
        "memory_forget" => MemoryForgetTool::execute(arguments, ctx.store).await,
        "web_search" => WebSearchTool::execute(arguments).await,
        "schedule_add" => {
            ScheduleAddTool::execute(arguments, ctx.schedule_store, ctx.chat_id, ctx.thread_id)
                .await
        }
        "schedule_list" => ScheduleListTool::execute(ctx.schedule_store, ctx.chat_id).await,
        "schedule_cancel" => {
            ScheduleCancelTool::execute(arguments, ctx.schedule_store, ctx.chat_id).await
        }
        "create_topic" => CreateTopicTool::execute(arguments, ctx).await,
        "rename_topic" => RenameTopicTool::execute(arguments, ctx).await,
        "close_topic" => CloseTopicTool::execute(arguments, ctx).await,
        "reopen_topic" => ReopenTopicTool::execute(arguments, ctx).await,
        "delete_topic" => DeleteTopicTool::execute(arguments, ctx).await,
        _ => Ok(ToolResult {
            output: format!("Unknown tool: {name}"),
        }),
    }
}
