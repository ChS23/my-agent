use anyhow::Result;
use async_openai::types::chat::{ChatCompletionTool, ChatCompletionTools, FunctionObject};

use super::ToolResult;

pub struct UrlReaderTool;

impl UrlReaderTool {
    pub fn spec() -> ChatCompletionTools {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: "read_url".into(),
                description: Some(
                    "Fetch and read the content of a web page. Returns extracted text content. \
                     Use when the user shares a link and wants to know what's there, \
                     or when you need to look up information from a specific URL."
                        .into(),
                ),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to fetch and read"
                        }
                    },
                    "required": ["url"]
                })),
                strict: None,
            },
        })
    }

    pub async fn execute(arguments: &str) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let url = args["url"].as_str().unwrap_or("");

        if url.is_empty() {
            return Ok(ToolResult {
                output: "Error: url is required".into(),
            });
        }

        let http = frankenstein::reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        let resp = http
            .get(url)
            .header("User-Agent", "Mozilla/5.0 (compatible; PersonalAgent/1.0)")
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Ok(ToolResult {
                output: format!("HTTP error: {status}"),
            });
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = resp.text().await?;

        let text = if content_type.contains("text/html") {
            extract_text_from_html(&body)
        } else {
            body
        };

        // Truncate to reasonable size for LLM context
        let max_chars = 8000;
        let output = if text.len() > max_chars {
            format!(
                "{}\n\n[Truncated — showing first {} of {} chars]",
                &text[..max_chars],
                max_chars,
                text.len()
            )
        } else {
            text
        };

        Ok(ToolResult { output })
    }
}

/// Simple HTML to text extraction using the scraper crate.
fn extract_text_from_html(html: &str) -> String {
    let document = scraper::Html::parse_document(html);

    // Remove script and style elements
    let _skip_tags = ["script", "style", "noscript", "nav", "footer", "header"];

    let mut text = String::new();

    // Try to find main content areas first
    let main_selectors = ["article", "main", "[role=main]", ".content", ".post-content"];
    let mut found_main = false;

    for sel in &main_selectors {
        if let Ok(selector) = scraper::Selector::parse(sel) {
            for element in document.select(&selector) {
                let t = element.text().collect::<Vec<_>>().join(" ");
                if t.len() > 100 {
                    text = t;
                    found_main = true;
                    break;
                }
            }
        }
        if found_main {
            break;
        }
    }

    // Fallback: extract from body
    if !found_main {
        if let Ok(body_sel) = scraper::Selector::parse("body") {
            for element in document.select(&body_sel) {
                text = element.text().collect::<Vec<_>>().join(" ");
            }
        }
    }

    // Clean up whitespace
    let cleaned: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    cleaned
}
