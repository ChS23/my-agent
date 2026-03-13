/// Convert markdown to Telegram-compatible HTML.
///
/// Supports: **bold**, *italic*, `code`, ```pre```, [links](url),
/// ~~strikethrough~~, > blockquotes, headers (as bold).
pub fn md_to_telegram_html(md: &str) -> String {
    let mut out = String::with_capacity(md.len() * 2);
    let mut chars = md.chars().peekable();
    let mut in_code_block = false;
    let mut code_block_buf = String::new();
    let mut line_start = true;

    while let Some(ch) = chars.next() {
        // Code blocks: ```
        if ch == '`' && chars.peek() == Some(&'`') {
            chars.next(); // second `
            if chars.peek() == Some(&'`') {
                chars.next(); // third `
                if !in_code_block {
                    in_code_block = true;
                    code_block_buf.clear();
                    // Skip optional language tag
                    while let Some(&c) = chars.peek() {
                        if c == '\n' {
                            chars.next();
                            break;
                        }
                        chars.next();
                    }
                    out.push_str("<pre>");
                } else {
                    in_code_block = false;
                    out.push_str(&escape_html(&code_block_buf));
                    out.push_str("</pre>");
                }
                continue;
            }
            // Was just `` — treat as text
            out.push('`');
            out.push('`');
            continue;
        }

        if in_code_block {
            code_block_buf.push(ch);
            continue;
        }

        // Inline code: `text`
        if ch == '`' {
            let code: String = chars.by_ref().take_while(|&c| c != '`').collect();
            out.push_str("<code>");
            out.push_str(&escape_html(&code));
            out.push_str("</code>");
            continue;
        }

        // Bold: **text**
        if ch == '*' && chars.peek() == Some(&'*') {
            chars.next();
            let inner = take_until_marker(&mut chars, "**");
            out.push_str("<b>");
            out.push_str(&escape_html(&inner));
            out.push_str("</b>");
            continue;
        }

        // Italic: *text*  (single asterisk)
        if ch == '*' {
            let inner = take_until_marker(&mut chars, "*");
            out.push_str("<i>");
            out.push_str(&escape_html(&inner));
            out.push_str("</i>");
            continue;
        }

        // Strikethrough: ~~text~~
        if ch == '~' && chars.peek() == Some(&'~') {
            chars.next();
            let inner = take_until_marker(&mut chars, "~~");
            out.push_str("<s>");
            out.push_str(&escape_html(&inner));
            out.push_str("</s>");
            continue;
        }

        // Links: [text](url)
        if ch == '[' {
            let text: String = chars.by_ref().take_while(|&c| c != ']').collect();
            if chars.peek() == Some(&'(') {
                chars.next();
                let url: String = chars.by_ref().take_while(|&c| c != ')').collect();
                out.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    escape_html(&url),
                    escape_html(&text)
                ));
            } else {
                out.push('[');
                out.push_str(&escape_html(&text));
                out.push(']');
            }
            continue;
        }

        // Headers: # text → bold
        if line_start && ch == '#' {
            // Skip # and spaces
            while chars.peek() == Some(&'#') || chars.peek() == Some(&' ') {
                chars.next();
            }
            let line: String = chars.by_ref().take_while(|&c| c != '\n').collect();
            out.push_str("<b>");
            out.push_str(&escape_html(&line));
            out.push_str("</b>\n");
            line_start = true;
            continue;
        }

        // Blockquote: > text
        if line_start && ch == '>' {
            if chars.peek() == Some(&' ') {
                chars.next();
            }
            let line: String = chars.by_ref().take_while(|&c| c != '\n').collect();
            out.push_str("<blockquote>");
            out.push_str(&escape_html(&line));
            out.push_str("</blockquote>\n");
            line_start = true;
            continue;
        }

        // Newline tracking
        if ch == '\n' {
            out.push('\n');
            line_start = true;
            continue;
        }

        line_start = false;

        // Escape HTML entities for plain text
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(ch),
        }
    }

    // Close unclosed code block
    if in_code_block {
        out.push_str(&escape_html(&code_block_buf));
        out.push_str("</pre>");
    }

    out
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn take_until_marker(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    marker: &str,
) -> String {
    let mut buf = String::new();
    let marker_chars: Vec<char> = marker.chars().collect();

    loop {
        match chars.peek() {
            None => break,
            Some(&c) => {
                if c == marker_chars[0] {
                    // Check if full marker matches
                    let mut matched = true;
                    let mut temp = Vec::new();
                    for &mc in &marker_chars {
                        match chars.next() {
                            Some(ch) if ch == mc => temp.push(ch),
                            Some(ch) => {
                                temp.push(ch);
                                matched = false;
                                break;
                            }
                            None => {
                                matched = false;
                                break;
                            }
                        }
                    }
                    if matched {
                        break;
                    }
                    // Not a match — push consumed chars
                    for ch in temp {
                        buf.push(ch);
                    }
                } else {
                    buf.push(c);
                    chars.next();
                }
            }
        }
    }

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold() {
        assert_eq!(md_to_telegram_html("**bold**"), "<b>bold</b>");
    }

    #[test]
    fn test_italic() {
        assert_eq!(md_to_telegram_html("*italic*"), "<i>italic</i>");
    }

    #[test]
    fn test_code() {
        assert_eq!(md_to_telegram_html("`code`"), "<code>code</code>");
    }

    #[test]
    fn test_code_block() {
        assert_eq!(
            md_to_telegram_html("```rust\nlet x = 1;\n```"),
            "<pre>let x = 1;\n</pre>"
        );
    }

    #[test]
    fn test_link() {
        assert_eq!(
            md_to_telegram_html("[click](https://example.com)"),
            "<a href=\"https://example.com\">click</a>"
        );
    }

    #[test]
    fn test_header() {
        assert_eq!(md_to_telegram_html("## Title"), "<b>Title</b>\n");
    }

    #[test]
    fn test_plain_html_escaped() {
        assert_eq!(md_to_telegram_html("<script>"), "&lt;script&gt;");
    }
}
