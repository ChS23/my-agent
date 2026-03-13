use anyhow::Result;
use async_openai::types::chat::ChatCompletionTools;
use serde_json::json;

use super::ToolResult;

pub struct WebSearchTool;

impl WebSearchTool {
    pub fn spec() -> ChatCompletionTools {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web using DuckDuckGo. Returns top results with titles, URLs, and snippets.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results (default 5, max 10)"
                        }
                    },
                    "required": ["query"]
                }
            }
        }))
        .expect("valid tool spec")
    }

    pub async fn execute(arguments: &str) -> Result<ToolResult> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'query'"))?;
        let max_results = args["max_results"]
            .as_u64()
            .unwrap_or(5)
            .clamp(1, 10) as usize;

        let results = search_ddg(query, max_results).await?;

        if results.is_empty() {
            return Ok(ToolResult {
                output: "No results found.".to_string(),
            });
        }

        let mut output = format!("Search results for: {query}\n\n");
        for (i, r) in results.iter().enumerate() {
            output.push_str(&format!(
                "{}. {}\n   {}\n   {}\n\n",
                i + 1,
                r.title,
                r.url,
                r.snippet
            ));
        }

        Ok(ToolResult { output })
    }
}

struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

async fn search_ddg(query: &str, max_results: usize) -> Result<Vec<SearchResult>> {
    let client = frankenstein::reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    // Build form body manually since frankenstein::reqwest may not expose .form()
    let body = format!(
        "q={}&kl=",
        url_encode(query)
    );

    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("DuckDuckGo returned status {}", resp.status());
    }

    let html = resp.text().await?;
    let document = scraper::Html::parse_document(&html);

    let result_selector = scraper::Selector::parse(".result").expect("valid selector");
    let title_selector = scraper::Selector::parse(".result__a").expect("valid selector");
    let snippet_selector = scraper::Selector::parse(".result__snippet").expect("valid selector");

    let mut results = Vec::new();

    for element in document.select(&result_selector).take(max_results) {
        let title = element
            .select(&title_selector)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();

        // Extract href and decode DDG redirect URL
        let url = element
            .select(&title_selector)
            .next()
            .and_then(|e| e.value().attr("href"))
            .map(|href| decode_ddg_url(href))
            .unwrap_or_default();

        let snippet = element
            .select(&snippet_selector)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }

    tracing::debug!(query, count = results.len(), "ddg search");
    Ok(results)
}

/// Decode DuckDuckGo redirect URLs.
/// DDG wraps links as `/l/?uddg=https%3A%2F%2Fexample.com&...`
fn decode_ddg_url(href: &str) -> String {
    if let Some(pos) = href.find("uddg=") {
        let encoded = &href[pos + 5..];
        let encoded = encoded.split('&').next().unwrap_or(encoded);
        url_decode(encoded)
    } else if href.starts_with("http") {
        href.to_string()
    } else {
        String::new()
    }
}

fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push_str(&format!("{b:02X}"));
            }
        }
    }
    result
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            let hex = [hi, lo];
            if let Ok(s) = std::str::from_utf8(&hex) {
                if let Ok(byte) = u8::from_str_radix(s, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push(hi as char);
            result.push(lo as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}
