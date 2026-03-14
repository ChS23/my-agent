use std::path::Path;
use anyhow::Result;

/// A loaded skill parsed from a markdown file with YAML frontmatter.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub trigger: SkillTrigger,
    pub enabled: bool,
    /// Full markdown content (after frontmatter).
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkillTrigger {
    /// User must invoke explicitly (e.g. /weekly_review)
    Manual,
    /// LLM can invoke automatically based on context
    Auto,
}

/// Load all skills from a directory.
/// Each .md file is parsed for YAML frontmatter + content.
pub fn load_skills(dir: &Path) -> Result<Vec<Skill>> {
    let mut skills = Vec::new();

    if !dir.is_dir() {
        tracing::debug!(path = %dir.display(), "skills directory not found, skipping");
        return Ok(skills);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        match parse_skill(&path) {
            Ok(skill) => {
                if skill.enabled {
                    tracing::info!(name = %skill.name, trigger = ?skill.trigger, "skill loaded");
                    skills.push(skill);
                } else {
                    tracing::debug!(name = %skill.name, "skill disabled, skipping");
                }
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to parse skill");
            }
        }
    }

    Ok(skills)
}

/// Parse a single skill file.
fn parse_skill(path: &Path) -> Result<Skill> {
    let raw = std::fs::read_to_string(path)?;

    let (frontmatter, content) = split_frontmatter(&raw)
        .ok_or_else(|| anyhow::anyhow!("no YAML frontmatter found"))?;

    let yaml: serde_json::Value = serde_yaml_ng::from_str(frontmatter)?;

    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    let name = yaml["name"]
        .as_str()
        .unwrap_or(file_stem)
        .to_string();

    let description = yaml["description"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let trigger = match yaml["trigger"].as_str() {
        Some("auto") => SkillTrigger::Auto,
        _ => SkillTrigger::Manual,
    };

    let enabled = yaml["enabled"].as_bool().unwrap_or(true);

    Ok(Skill {
        name,
        description,
        trigger,
        enabled,
        content: content.to_string(),
    })
}

/// Split YAML frontmatter from markdown content.
/// Frontmatter is delimited by `---` at the start.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if !text.starts_with("---") {
        return None;
    }

    let after_first = &text[3..];
    let end = after_first.find("\n---")?;
    let frontmatter = &after_first[..end];
    let content = &after_first[end + 4..]; // skip \n---

    Some((frontmatter.trim(), content.trim()))
}

