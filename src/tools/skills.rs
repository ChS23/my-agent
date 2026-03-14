use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};

use crate::skills::{Skill, SkillTrigger};

use super::ToolResult;

pub struct UseSkillTool;

impl UseSkillTool {
    /// Generate tool spec dynamically from loaded skills.
    pub fn spec(skills: &[Skill]) -> Option<ChatCompletionTools> {
        if skills.is_empty() {
            return None;
        }

        let names: Vec<serde_json::Value> = skills
            .iter()
            .map(|s| serde_json::Value::String(s.name.clone()))
            .collect();

        let mut desc = String::from(
            "Invoke a skill to get detailed instructions for a specific task. Available skills:\n",
        );
        for skill in skills {
            let trigger = match skill.trigger {
                SkillTrigger::Auto => " [auto]",
                SkillTrigger::Manual => "",
            };
            desc.push_str(&format!("- {}{}: {}\n", skill.name, trigger, skill.description));
        }
        desc.push_str(
            "\nSkills marked [auto] should be invoked proactively when the context matches. \
             Other skills are invoked when the user requests them (e.g. /skill_name).\n\
             After receiving skill instructions, follow them to complete the task.",
        );

        Some(ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "use_skill".into(),
                description: Some(desc),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "enum": names,
                            "description": "Name of the skill to invoke"
                        }
                    },
                    "required": ["name"]
                })),
                strict: None,
            },
        }))
    }

    pub fn execute(arguments: &str, skills: &[Skill]) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing skill name"))?;

        match skills.iter().find(|s| s.name == name) {
            Some(skill) => {
                tracing::info!(skill = %skill.name, "skill content loaded");
                Ok(ToolResult {
                    output: format!(
                        "## Skill: {}\n\n{}\n\n---\nFollow these instructions now.",
                        skill.name, skill.content
                    ),
                })
            }
            None => Ok(ToolResult {
                output: format!("Skill '{}' not found.", name),
            }),
        }
    }
}

pub struct WriteSkillTool;

impl WriteSkillTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "write_skill".into(),
                description: Some(
                    "Save a new skill file to the skills/ directory. The file will be available \
                     after agent restart. Use this when the user wants to create a new skill/command."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Skill filename (without .md extension). Only latin letters, digits, underscores."
                        },
                        "content": {
                            "type": "string",
                            "description": "Full skill file content including YAML frontmatter (---\\nname: ...\\n---) and markdown instructions"
                        }
                    },
                    "required": ["name", "content"]
                })),
                strict: None,
            },
        })
    }

    pub fn execute(arguments: &str) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let name = args["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing skill name"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing skill content"))?;

        // Validate name (only safe chars)
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Ok(ToolResult {
                output: "Error: skill name must contain only latin letters, digits, underscores"
                    .into(),
            });
        }

        let path = std::path::Path::new("skills").join(format!("{}.md", name));

        // Don't overwrite existing skills without explicit intent
        if path.exists() {
            return Ok(ToolResult {
                output: format!(
                    "Skill '{}' already exists at {}. Delete it first or choose a different name.",
                    name,
                    path.display()
                ),
            });
        }

        std::fs::write(&path, content)?;
        tracing::info!(name, path = %path.display(), "skill file written");

        Ok(ToolResult {
            output: format!(
                "Skill '{}' saved to {}. Use restart_agent to activate it.",
                name,
                path.display()
            ),
        })
    }
}

pub struct RestartAgentTool;

impl RestartAgentTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "restart_agent".into(),
                description: Some(
                    "Restart the agent process. Use after writing a new skill to activate it. \
                     The process exits gracefully and the orchestrator (Docker/systemd) restarts it."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "reason": {
                            "type": "string",
                            "description": "Why the restart is needed (logged before exit)"
                        }
                    },
                    "required": ["reason"]
                })),
                strict: None,
            },
        })
    }

    pub fn execute(arguments: &str, bot: &frankenstein::client_reqwest::Bot, chat_id: i64, thread_id: Option<i32>) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let reason = args["reason"].as_str().unwrap_or("no reason");

        tracing::info!(reason, "agent restart requested");

        // Send confirmation before exit
        let bot = bot.clone();
        let msg = format!("🔄 Перезапускаюсь: {reason}");
        tokio::spawn(async move {
            use frankenstein::AsyncTelegramApi;
            let mut params = frankenstein::methods::SendMessageParams::builder()
                .chat_id(chat_id)
                .text(&msg)
                .build();
            if let Some(tid) = thread_id {
                params.message_thread_id = Some(tid);
            }
            let _ = bot.send_message(&params).await;

            // Give message time to send, then exit
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            std::process::exit(0);
        });

        Ok(ToolResult {
            output: "Restarting...".into(),
        })
    }
}
