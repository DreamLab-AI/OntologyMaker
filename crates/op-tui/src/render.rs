//! Step rendering: format tool calls, observations, token counts, markdown.
//!
//! Port of Python's `_flush_step`, `_present_result`, `_format_session_tokens`,
//! `_extract_key_arg`, and the `_LeftMarkdown` rendering from `tui.py`.

use std::collections::HashMap;
use std::io::{self, Write};

use crate::commands::format_token_count;
use crate::demo::DemoCensor;

/// Build a compact token summary from session token buckets.
///
/// The map structure is `{ model_name: { "input": N, "output": N } }`.
/// Returns an empty string if no tokens have been counted.
pub fn format_session_tokens(session_tokens: &HashMap<String, HashMap<String, u64>>) -> String {
    let total_in: u64 = session_tokens
        .values()
        .filter_map(|v| v.get("input"))
        .sum();
    let total_out: u64 = session_tokens
        .values()
        .filter_map(|v| v.get("output"))
        .sum();
    if total_in == 0 && total_out == 0 {
        return String::new();
    }
    format!(
        "{} in / {} out",
        format_token_count(total_in),
        format_token_count(total_out)
    )
}

// ---------------------------------------------------------------------------
// Key argument extraction
// ---------------------------------------------------------------------------

/// Map tool names to their most informative argument for compact display.
fn key_arg_for_tool(name: &str) -> Option<&'static str> {
    match name {
        "read_file" | "read_image" | "write_file" | "edit_file" | "hashline_edit" => Some("path"),
        "apply_patch" => Some("patch"),
        "run_shell" | "run_shell_bg" => Some("command"),
        "web_search" => Some("query"),
        "fetch_url" => Some("urls"),
        "search_files" => Some("query"),
        "list_files" | "repo_map" => Some("glob"),
        "subtask" | "execute" => Some("objective"),
        "think" => Some("note"),
        "check_shell_bg" | "kill_shell_bg" => Some("job_id"),
        _ => None,
    }
}

/// Extract the most informative argument value for compact display.
///
/// Matches Python's `_extract_key_arg`.
pub fn extract_key_arg(name: &str, arguments: &serde_json::Value) -> String {
    let key = key_arg_for_tool(name);

    if let Some(key) = key {
        if let Some(val) = arguments.get(key) {
            return format_arg_value(val);
        }
    }

    // Fallback: first string-valued argument.
    if let Some(obj) = arguments.as_object() {
        for val in obj.values() {
            if let Some(s) = val.as_str() {
                let s = s.trim();
                if !s.is_empty() {
                    return truncate_str(s, 60);
                }
            }
        }
    }

    String::new()
}

/// Format a JSON value for display.
fn format_arg_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => truncate_str(s.trim(), 60),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr
                .iter()
                .take(3)
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            truncate_str(&items.join(", "), 60)
        }
        other => truncate_str(&other.to_string(), 60),
    }
}

/// Truncate a string to at most `max_len` characters, appending "..." if needed.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let end = max_len.saturating_sub(3);
        format!("{}...", &s[..end])
    }
}

// ---------------------------------------------------------------------------
// Tool call record and step state (matching Python's dataclasses)
// ---------------------------------------------------------------------------

/// A single tool call record within a step.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub name: String,
    pub key_arg: String,
    pub elapsed_sec: f64,
    pub is_error: bool,
}

/// State for a completed model step.
#[derive(Debug, Clone)]
pub struct StepState {
    pub depth: u32,
    pub step: u32,
    pub max_steps: u32,
    pub model_text: String,
    pub model_elapsed_sec: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: Vec<ToolCallRecord>,
}

impl Default for StepState {
    fn default() -> Self {
        Self {
            depth: 0,
            step: 0,
            max_steps: 0,
            model_text: String::new(),
            model_elapsed_sec: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            tool_calls: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Step rendering
// ---------------------------------------------------------------------------

/// Maximum characters for event clipping.
const EVENT_MAX_CHARS: usize = 300;

/// Clip a trace event body to a reasonable display length.
pub fn clip_event(text: &str) -> String {
    let (first_line, rest) = match text.split_once('\n') {
        Some((first, rest)) => (first, Some(rest)),
        None => (text, None),
    };
    if first_line.len() > EVENT_MAX_CHARS {
        return format!("{}...", &first_line[..EVENT_MAX_CHARS]);
    }
    if let Some(rest) = rest {
        let extra_lines = rest.lines().count();
        return format!("{}  (+{} lines)", first_line, extra_lines);
    }
    first_line.to_string()
}

/// Render a completed step to stdout (inline).
///
/// Matches Python's `_flush_step`.
pub fn render_step(step: &StepState, context_window: u64, censor: Option<&DemoCensor>) {
    let mut stdout = io::stdout();

    // Timestamp.
    let ts = chrono::Local::now().format("%H:%M:%S");

    // Context usage.
    let ctx_str = format!(
        "{}/{}",
        format_token_count(step.input_tokens),
        format_token_count(context_window)
    );

    // Step header rule.
    let mut right_parts: Vec<String> = Vec::new();
    if step.depth > 0 {
        right_parts.push(format!("depth {}", step.depth));
    }
    if step.max_steps > 0 {
        right_parts.push(format!("{}/{}", step.step, step.max_steps));
    }
    if step.input_tokens > 0 || step.output_tokens > 0 {
        right_parts.push(format!(
            "{}in/{}out",
            format_token_count(step.input_tokens),
            format_token_count(step.output_tokens)
        ));
    }
    right_parts.push(format!("[{}]", ctx_str));
    let right = right_parts.join(" | ");

    let header = format!(
        " {}  Step {} {} ",
        ts, step.step, right
    );

    // Print a cyan rule.
    let _ = writeln!(
        stdout,
        "\x1b[36m{}\x1b[0m",
        format_rule(&header)
    );

    // Model text (dim, truncated).
    if !step.model_text.is_empty() {
        let mut preview = step.model_text.trim().to_string();
        if let Some(ref c) = censor {
            preview = c.censor_text(&preview);
        }
        if preview.len() > 200 {
            preview.truncate(197);
            preview.push_str("...");
        }
        let _ = writeln!(
            stdout,
            "\x1b[2m  ({:.1}s) {}\x1b[0m",
            step.model_elapsed_sec, preview
        );
    }

    // Tool call tree.
    let n = step.tool_calls.len();
    for (i, tc) in step.tool_calls.iter().enumerate() {
        let is_last = i == n - 1;
        let connector = if is_last { "\u{2514}\u{2500}" } else { "\u{251C}\u{2500}" };
        let name_style_start = if tc.is_error { "\x1b[1;31m" } else { "" };
        let name_style_end = if tc.is_error { "\x1b[0m" } else { "" };

        let mut key_arg = tc.key_arg.clone();
        if let Some(ref c) = censor {
            key_arg = c.censor_text(&key_arg);
        }

        let arg_part = if key_arg.is_empty() {
            String::new()
        } else {
            format!("  \x1b[2m\"{}\"\x1b[0m", key_arg)
        };

        let _ = writeln!(
            stdout,
            "\x1b[2m  {} \x1b[0m{}{}{}{}  \x1b[2m{:.1}s\x1b[0m",
            connector,
            name_style_start,
            tc.name,
            name_style_end,
            arg_part,
            tc.elapsed_sec
        );
    }

    let _ = stdout.flush();
}

// ---------------------------------------------------------------------------
// Simple markdown rendering
// ---------------------------------------------------------------------------

/// Render a markdown-ish final answer to stdout.
///
/// This is a simplified markdown renderer that handles:
/// - Headings (`#`, `##`, `###`)
/// - Bold (`**text**`)
/// - Italic (`*text*`)
/// - Code blocks (triple backtick)
/// - Inline code (`\`code\``)
/// - Bullet lists (`-`, `*`)
///
/// Uses ANSI escape codes for inline terminal output.
pub fn render_markdown(text: &str, censor: Option<&DemoCensor>) {
    let mut stdout = io::stdout();
    let text = if let Some(c) = censor {
        c.censor_text(text)
    } else {
        text.to_string()
    };

    let mut in_code_block = false;

    for line in text.lines() {
        // Code block toggle.
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            if in_code_block {
                let _ = writeln!(stdout, "\x1b[33m{}\x1b[0m", line);
            } else {
                let _ = writeln!(stdout, "\x1b[33m{}\x1b[0m", line);
            }
            continue;
        }

        if in_code_block {
            let _ = writeln!(stdout, "\x1b[33m{}\x1b[0m", line);
            continue;
        }

        // Headings.
        if line.starts_with("### ") {
            let _ = writeln!(stdout, "\x1b[1m{}\x1b[0m", &line[4..]);
            continue;
        }
        if line.starts_with("## ") {
            let _ = writeln!(stdout, "\n\x1b[1m{}\x1b[0m", &line[3..]);
            continue;
        }
        if line.starts_with("# ") {
            let _ = writeln!(stdout, "\n\x1b[1;4m{}\x1b[0m", &line[2..]);
            continue;
        }

        // Regular line with inline formatting.
        let formatted = render_inline_markdown(line);
        let _ = writeln!(stdout, "{}", formatted);
    }
    let _ = stdout.flush();
}

/// Apply inline markdown formatting (bold, italic, code) to a line.
fn render_inline_markdown(line: &str) -> String {
    let mut result = String::with_capacity(line.len() + 32);
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Inline code: `code`
        if chars[i] == '`' {
            if let Some(end) = find_closing(&chars, i + 1, '`') {
                result.push_str("\x1b[33m");
                for c in &chars[i + 1..end] {
                    result.push(*c);
                }
                result.push_str("\x1b[0m");
                i = end + 1;
                continue;
            }
        }

        // Bold: **text**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_double_closing(&chars, i + 2, '*') {
                result.push_str("\x1b[1m");
                for c in &chars[i + 2..end] {
                    result.push(*c);
                }
                result.push_str("\x1b[0m");
                i = end + 2;
                continue;
            }
        }

        // Italic: *text*
        if chars[i] == '*' {
            if let Some(end) = find_closing(&chars, i + 1, '*') {
                result.push_str("\x1b[3m");
                for c in &chars[i + 1..end] {
                    result.push(*c);
                }
                result.push_str("\x1b[0m");
                i = end + 1;
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

/// Find the index of the closing single character.
fn find_closing(chars: &[char], start: usize, delimiter: char) -> Option<usize> {
    for i in start..chars.len() {
        if chars[i] == delimiter {
            return Some(i);
        }
    }
    None
}

/// Find the index of a closing double character (e.g. `**`).
fn find_double_closing(chars: &[char], start: usize, delimiter: char) -> Option<usize> {
    if chars.len() < 2 {
        return None;
    }
    for i in start..chars.len() - 1 {
        if chars[i] == delimiter && chars[i + 1] == delimiter {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Rule formatting
// ---------------------------------------------------------------------------

/// Format a horizontal rule with centered text.
///
/// Produces something like `──── Step 3 | 1.2kin/500out [3.2k/200k] ────`.
fn format_rule(text: &str) -> String {
    let width = terminal_width();
    let text_len = text.len();
    if text_len + 4 >= width {
        return text.to_string();
    }
    let remaining = width - text_len;
    let left = remaining / 2;
    let right = remaining - left;
    format!(
        "{}{}{}",
        "\u{2500}".repeat(left),
        text,
        "\u{2500}".repeat(right)
    )
}

/// Get the terminal width, defaulting to 80.
fn terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
}

// ---------------------------------------------------------------------------
// Ratatui full-screen widget types and rendering functions
// ---------------------------------------------------------------------------

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{DisplayMessage, MessageRole, StatusInfo};
use crate::theme;

/// Build the four-area vertical layout:
///   [0] header   (3 lines)
///   [1] conversation (fill)
///   [2] input    (3 lines)
///   [3] status   (1 line)
pub fn build_layout(area: Rect) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // header
            Constraint::Min(1),      // conversation
            Constraint::Length(3),   // input
            Constraint::Length(1),   // status bar
        ])
        .split(area)
        .to_vec()
}

/// Render the header bar with application title.
pub fn render_header(f: &mut Frame, area: Rect) {
    let title = Line::from(vec![
        Span::styled(" OpenPlanter ", theme::style_thinking()),
        Span::styled(" TUI ", theme::style_dim()),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(theme::style_dim());

    let header = Paragraph::new(title).block(block);
    f.render_widget(header, area);
}

/// Render the conversation messages in the main area.
///
/// Scrolls to keep the most recent message visible.
pub fn render_conversation(
    f: &mut Frame,
    area: Rect,
    messages: &[DisplayMessage],
    scroll_offset: u16,
) {
    let mut lines: Vec<Line<'_>> = Vec::new();

    for msg in messages {
        let (prefix, prefix_style) = match msg.role {
            MessageRole::User => ("you> ", theme::style_prompt()),
            MessageRole::Assistant => ("assistant> ", theme::style_streaming()),
            MessageRole::System => ("system> ", theme::style_info()),
            MessageRole::Tool => {
                let name = msg.tool_name.as_deref().unwrap_or("tool");
                let elapsed_str = msg
                    .elapsed_sec
                    .map(|e| format!(" ({:.1}s)", e))
                    .unwrap_or_default();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  \u{2502} {}{}", name, elapsed_str),
                        theme::style_tool(),
                    ),
                ]));
                for text_line in msg.content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  \u{2502} {}", text_line),
                        theme::style_dim(),
                    )));
                }
                lines.push(Line::from(""));
                continue;
            }
        };

        let content_lines: Vec<&str> = msg.content.lines().collect();
        if content_lines.is_empty() {
            lines.push(Line::from(Span::styled(
                prefix.to_string(),
                prefix_style,
            )));
        } else {
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), prefix_style),
                Span::raw(content_lines[0]),
            ]));
            for &text_line in &content_lines[1..] {
                lines.push(Line::from(Span::raw(format!(
                    "{}{}",
                    " ".repeat(prefix.len()),
                    text_line,
                ))));
            }
        }
        lines.push(Line::from(""));
    }

    let block = Block::default().borders(Borders::NONE);
    let text = ratatui::text::Text::from(lines);
    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));

    f.render_widget(paragraph, area);
}

/// Render the input box with the current user input and cursor.
pub fn render_input(
    f: &mut Frame,
    area: Rect,
    input: &str,
    cursor_pos: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::style_prompt())
        .title(Span::styled(" Input ", theme::style_prompt()));

    let input_text = Paragraph::new(input.to_string())
        .block(block)
        .wrap(Wrap { trim: false });

    f.render_widget(input_text, area);

    let cursor_x = area.x + 1 + cursor_pos as u16;
    let cursor_y = area.y + 1;
    f.set_cursor_position((cursor_x, cursor_y));
}

/// Render the bottom status bar.
pub fn render_status_bar(f: &mut Frame, area: Rect, status: &StatusInfo) {
    let mut spans = vec![
        Span::styled(
            format!(" {} ", status.provider),
            Style::default()
                .fg(theme::CYAN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("| {} ", status.model_name),
            theme::style_dim(),
        ),
    ];

    if !status.mode.is_empty() {
        spans.push(Span::styled(
            format!("| {} ", status.mode),
            theme::style_dim(),
        ));
    }

    if !status.token_summary.is_empty() {
        spans.push(Span::styled(
            format!("| {} ", status.token_summary),
            theme::style_dim(),
        ));
    }

    if let Some(ref activity) = status.activity {
        spans.push(Span::styled(
            format!("| {} ", activity),
            theme::style_streaming(),
        ));
    }

    let line = Line::from(spans);
    let bar = Paragraph::new(line)
        .style(Style::default().bg(ratatui::style::Color::DarkGray));

    f.render_widget(bar, area);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- token formatting ---------------------------------------------------

    #[test]
    fn test_format_token_count_below_1k() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1), "1");
    }

    #[test]
    fn test_format_token_count_1k_to_10k() {
        assert_eq!(format_token_count(1_000), "1.0k");
        assert_eq!(format_token_count(1_234), "1.2k");
        assert_eq!(format_token_count(9_999), "10.0k");
    }

    #[test]
    fn test_format_token_count_10k_to_1m() {
        assert_eq!(format_token_count(10_000), "10k");
        assert_eq!(format_token_count(15_678), "16k");
        assert_eq!(format_token_count(999_999), "1000k");
    }

    #[test]
    fn test_format_token_count_above_1m() {
        assert_eq!(format_token_count(1_000_000), "1.0M");
        assert_eq!(format_token_count(1_234_567), "1.2M");
    }

    // -- session tokens -----------------------------------------------------

    #[test]
    fn test_format_session_tokens_empty() {
        let tokens = HashMap::new();
        assert_eq!(format_session_tokens(&tokens), "");
    }

    #[test]
    fn test_format_session_tokens_zero() {
        let mut tokens = HashMap::new();
        let mut bucket = HashMap::new();
        bucket.insert("input".to_string(), 0u64);
        bucket.insert("output".to_string(), 0u64);
        tokens.insert("model".to_string(), bucket);
        assert_eq!(format_session_tokens(&tokens), "");
    }

    #[test]
    fn test_format_session_tokens_normal() {
        let mut tokens = HashMap::new();
        let mut bucket = HashMap::new();
        bucket.insert("input".to_string(), 5_000u64);
        bucket.insert("output".to_string(), 1_200u64);
        tokens.insert("claude".to_string(), bucket);
        assert_eq!(format_session_tokens(&tokens), "5.0k in / 1.2k out");
    }

    // -- key arg extraction -------------------------------------------------

    #[test]
    fn test_extract_key_arg_known_tool() {
        let args = serde_json::json!({"path": "/tmp/foo.rs", "content": "hello"});
        assert_eq!(extract_key_arg("read_file", &args), "/tmp/foo.rs");
    }

    #[test]
    fn test_extract_key_arg_fallback() {
        let args = serde_json::json!({"custom_field": "some value"});
        assert_eq!(extract_key_arg("unknown_tool", &args), "some value");
    }

    #[test]
    fn test_extract_key_arg_truncation() {
        let long_val = "a".repeat(100);
        let args = serde_json::json!({"path": long_val});
        let result = extract_key_arg("read_file", &args);
        assert!(result.len() <= 63); // 60 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_extract_key_arg_empty_args() {
        let args = serde_json::json!({});
        assert_eq!(extract_key_arg("read_file", &args), "");
    }

    // -- clip event ---------------------------------------------------------

    #[test]
    fn test_clip_event_short() {
        assert_eq!(clip_event("hello world"), "hello world");
    }

    #[test]
    fn test_clip_event_multiline() {
        let text = "first line\nsecond\nthird";
        let clipped = clip_event(text);
        assert!(clipped.contains("first line"));
        assert!(clipped.contains("+2 lines"));
    }

    #[test]
    fn test_clip_event_long_first_line() {
        let long = "x".repeat(500);
        let clipped = clip_event(&long);
        assert!(clipped.len() < 500);
        assert!(clipped.ends_with("..."));
    }

    // -- inline markdown ----------------------------------------------------

    #[test]
    fn test_render_inline_bold() {
        let result = render_inline_markdown("hello **world**");
        assert!(result.contains("\x1b[1m"));
        assert!(result.contains("world"));
    }

    #[test]
    fn test_render_inline_italic() {
        let result = render_inline_markdown("hello *world*");
        assert!(result.contains("\x1b[3m"));
        assert!(result.contains("world"));
    }

    #[test]
    fn test_render_inline_code() {
        let result = render_inline_markdown("use `fn main()`");
        assert!(result.contains("\x1b[33m"));
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn test_render_inline_plain() {
        let result = render_inline_markdown("plain text");
        assert_eq!(result, "plain text");
    }

    // -- format_rule --------------------------------------------------------

    #[test]
    fn test_format_rule_contains_text() {
        let rule = format_rule(" Step 1 ");
        assert!(rule.contains("Step 1"));
        assert!(rule.contains('\u{2500}'));
    }

    // -- truncate_str -------------------------------------------------------

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        let result = truncate_str("hello world", 8);
        assert_eq!(result, "hello...");
    }
}
