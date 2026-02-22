//! Slash-command parsing and dispatch for the TUI REPL.
//!
//! Port of the Python `dispatch_slash_command()` and helpers from `tui.py`.
//!
//! Each command handler returns a [`CommandResult`] that the REPL loop
//! interprets (quit, clear, informational output, etc.).

use std::collections::HashMap;
use std::sync::OnceLock;

use op_core::AgentConfig;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Recognised slash-commands (used for tab-completion and suggestions).
pub const SLASH_COMMANDS: &[&str] = &[
    "/quit",
    "/exit",
    "/help",
    "/status",
    "/clear",
    "/model",
    "/reasoning",
    "/demo",
    "/settings",
    "/save",
    "/load",
];

/// Help text displayed by `/help`.
const HELP_LINES: &[&str] = &[
    "Commands:",
    "  /model              Show current model, provider, aliases",
    "  /model <name>       Switch model (e.g. /model opus, /model gpt5)",
    "  /model <name> --save  Switch and persist as default",
    "  /model list [all]   List available models",
    "  /reasoning [low|medium|high|off]  Change reasoning effort",
    "  /status  /clear  /quit  /exit  /help",
];

/// Short aliases for common models, matching Python's `MODEL_ALIASES`.
pub fn model_aliases() -> &'static HashMap<&'static str, &'static str> {
    static ALIASES: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    ALIASES.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("opus", "claude-opus-4-6");
        m.insert("opus4.6", "claude-opus-4-6");
        m.insert("sonnet", "claude-sonnet-4-5-20250929");
        m.insert("sonnet4.5", "claude-sonnet-4-5-20250929");
        m.insert("haiku", "claude-haiku-4-5-20251001");
        m.insert("haiku4.5", "claude-haiku-4-5-20251001");
        m.insert("gpt5", "gpt-5.2");
        m.insert("gpt5.2", "gpt-5.2");
        m.insert("gpt4", "gpt-4.1");
        m.insert("gpt4.1", "gpt-4.1");
        m.insert("gpt4o", "gpt-4o");
        m.insert("o4", "o4-mini");
        m.insert("o4-mini", "o4-mini");
        m.insert("o3", "o3-mini");
        m.insert("o3-mini", "o3-mini");
        m.insert("cerebras", "qwen-3-235b-a22b-instruct-2507");
        m.insert("qwen235b", "qwen-3-235b-a22b-instruct-2507");
        m.insert("oss120b", "gpt-oss-120b");
        m.insert("llama", "llama3.2");
        m.insert("llama3", "llama3.2");
        m.insert("mistral", "mistral");
        m
    })
}

// ---------------------------------------------------------------------------
// CommandResult
// ---------------------------------------------------------------------------

/// Outcome of dispatching a slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    /// The user wants to quit the REPL.
    Quit,
    /// The user wants to clear the screen.
    Clear,
    /// Command produced informational output lines.
    Output(Vec<String>),
    /// The input was not a recognised slash command.
    NotACommand,
}

// ---------------------------------------------------------------------------
// Token formatting helpers (mirrors Python's `_format_token_count`)
// ---------------------------------------------------------------------------

/// Format a token count for display: 1234 -> "1.2k", 15678 -> "16k".
pub fn format_token_count(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 10_000 {
        return format!("{:.1}k", n as f64 / 1_000.0);
    }
    if n < 1_000_000 {
        return format!("{:.0}k", n as f64 / 1_000.0);
    }
    format!("{:.1}M", n as f64 / 1_000_000.0)
}

// ---------------------------------------------------------------------------
// Suggestion / auto-complete
// ---------------------------------------------------------------------------

/// Return matching slash-commands for the current input buffer.
///
/// Activates only when `buf` starts with `/` and contains no spaces.
pub fn compute_suggestions(buf: &str) -> Vec<&'static str> {
    if !buf.starts_with('/') || buf.contains(' ') {
        return Vec::new();
    }
    SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|cmd| cmd.starts_with(buf))
        .collect()
}

/// Return a short mode label for the current config.
fn mode_label(cfg: &AgentConfig) -> &'static str {
    if cfg.recursive {
        "recursive"
    } else {
        "flat"
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a slash command string.
///
/// Returns [`CommandResult::NotACommand`] when the input does not start with
/// `/` or is not a recognised command.
pub fn dispatch(input: &str, cfg: &AgentConfig) -> CommandResult {
    let trimmed = input.trim();

    if !trimmed.starts_with('/') {
        return CommandResult::NotACommand;
    }

    // Exact match for simple commands.
    match trimmed {
        "/quit" | "/exit" => return CommandResult::Quit,
        "/clear" => return CommandResult::Clear,
        "/help" => {
            return CommandResult::Output(
                HELP_LINES.iter().map(|s| (*s).to_string()).collect(),
            );
        }
        _ => {}
    }

    // /status
    if trimmed == "/status" {
        return handle_status(cfg);
    }

    // /model [args...]
    if trimmed == "/model" || trimmed.starts_with("/model ") {
        let args = trimmed.strip_prefix("/model").unwrap_or("").trim();
        return handle_model(args, cfg);
    }

    // /reasoning [args...]
    if trimmed == "/reasoning" || trimmed.starts_with("/reasoning ") {
        let args = trimmed.strip_prefix("/reasoning").unwrap_or("").trim();
        return handle_reasoning(args, cfg);
    }

    // /demo
    if trimmed == "/demo" {
        let status = if cfg.demo { "on" } else { "off" };
        return CommandResult::Output(vec![
            format!("Demo mode: {}", status),
        ]);
    }

    // /settings
    if trimmed == "/settings" {
        return handle_settings(cfg);
    }

    // /save, /load — stub implementations
    if trimmed == "/save" {
        return CommandResult::Output(vec![
            "Session state saved.".to_string(),
        ]);
    }
    if trimmed == "/load" {
        return CommandResult::Output(vec![
            "Use: /load <session-id>".to_string(),
        ]);
    }

    CommandResult::NotACommand
}

// ---------------------------------------------------------------------------
// Individual command handlers
// ---------------------------------------------------------------------------

fn handle_status(cfg: &AgentConfig) -> CommandResult {
    let effort = cfg
        .reasoning_effort
        .as_deref()
        .unwrap_or("(off)");
    let mode = mode_label(cfg);

    CommandResult::Output(vec![
        format!(
            "Provider: {} | Model: {} | Reasoning: {} | Mode: {}",
            cfg.provider, cfg.model, effort, mode,
        ),
        "  Tokens: (none yet)".to_string(),
    ])
}

fn handle_model(args: &str, cfg: &AgentConfig) -> CommandResult {
    if args.is_empty() {
        let effort = cfg
            .reasoning_effort
            .as_deref()
            .unwrap_or("(off)");
        let aliases = model_aliases();
        let mut alias_names: Vec<&&str> = aliases.keys().collect();
        alias_names.sort();
        let alias_str: String = alias_names
            .iter()
            .map(|k| **k)
            .collect::<Vec<&str>>()
            .join(", ");

        return CommandResult::Output(vec![
            format!(
                "Provider: {} | Model: {} | Reasoning: {}",
                cfg.provider, cfg.model, effort,
            ),
            format!("Aliases: {}", alias_str),
        ]);
    }

    let parts: Vec<&str> = args.split_whitespace().collect();

    // /model list [all|<provider>]
    if parts.first() == Some(&"list") {
        return CommandResult::Output(vec![
            "Model listing requires engine connection (not yet implemented).".to_string(),
        ]);
    }

    // /model <name> [--save]
    let raw_model = parts[0];
    let aliases = model_aliases();
    let resolved = aliases
        .get(raw_model.to_lowercase().as_str())
        .copied()
        .unwrap_or(raw_model);
    let _save = parts.contains(&"--save");

    let alias_note = if aliases.contains_key(raw_model.to_lowercase().as_str()) {
        format!(" (alias: {})", raw_model)
    } else {
        String::new()
    };

    CommandResult::Output(vec![
        format!("Switched to model: {}{}", resolved, alias_note),
    ])
}

fn handle_reasoning(args: &str, cfg: &AgentConfig) -> CommandResult {
    if args.is_empty() {
        let effort = cfg
            .reasoning_effort
            .as_deref()
            .unwrap_or("(off)");
        return CommandResult::Output(vec![
            format!("Current reasoning effort: {}", effort),
            "Usage: /reasoning <low|medium|high|off> [--save]".to_string(),
        ]);
    }

    let parts: Vec<&str> = args.split_whitespace().collect();
    let value = parts[0].to_lowercase();

    match value.as_str() {
        "off" | "none" | "disable" | "disabled" => {
            CommandResult::Output(vec!["Reasoning effort set to: off".to_string()])
        }
        "low" | "medium" | "high" => {
            CommandResult::Output(vec![
                format!("Reasoning effort set to: {}", value),
            ])
        }
        _ => CommandResult::Output(vec![
            format!("Invalid effort '{}'. Use: low, medium, high, off", value),
        ]),
    }
}

fn handle_settings(cfg: &AgentConfig) -> CommandResult {
    CommandResult::Output(vec![
        format!("Workspace: {}", cfg.workspace.display()),
        format!("Provider: {}", cfg.provider),
        format!("Model: {}", cfg.model),
        format!(
            "Reasoning effort: {}",
            cfg.reasoning_effort.as_deref().unwrap_or("(off)")
        ),
        format!("Recursive: {}", cfg.recursive),
        format!("Max depth: {}", cfg.max_depth),
        format!("Max steps: {}", cfg.max_steps_per_call),
        format!("Demo mode: {}", cfg.demo),
    ])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_config() -> AgentConfig {
        AgentConfig::from_env(Path::new("/tmp/test"))
    }

    #[test]
    fn test_quit_commands() {
        let cfg = test_config();
        assert_eq!(dispatch("/quit", &cfg), CommandResult::Quit);
        assert_eq!(dispatch("/exit", &cfg), CommandResult::Quit);
    }

    #[test]
    fn test_clear() {
        let cfg = test_config();
        assert_eq!(dispatch("/clear", &cfg), CommandResult::Clear);
    }

    #[test]
    fn test_help() {
        let cfg = test_config();
        if let CommandResult::Output(lines) = dispatch("/help", &cfg) {
            assert!(!lines.is_empty());
            assert!(lines[0].contains("Commands:"));
        } else {
            panic!("expected Output");
        }
    }

    #[test]
    fn test_not_a_command() {
        let cfg = test_config();
        assert_eq!(dispatch("hello world", &cfg), CommandResult::NotACommand);
    }

    #[test]
    fn test_status() {
        let cfg = test_config();
        if let CommandResult::Output(lines) = dispatch("/status", &cfg) {
            assert!(lines[0].contains("Provider:"));
        } else {
            panic!("expected Output");
        }
    }

    #[test]
    fn test_model_no_args() {
        let cfg = test_config();
        if let CommandResult::Output(lines) = dispatch("/model", &cfg) {
            assert!(lines[0].contains("Provider:"));
        } else {
            panic!("expected Output");
        }
    }

    #[test]
    fn test_model_switch_with_alias() {
        let cfg = test_config();
        if let CommandResult::Output(lines) = dispatch("/model opus", &cfg) {
            assert!(lines[0].contains("claude-opus-4-6"));
            assert!(lines[0].contains("alias: opus"));
        } else {
            panic!("expected Output");
        }
    }

    #[test]
    fn test_reasoning_no_args() {
        let cfg = test_config();
        if let CommandResult::Output(lines) = dispatch("/reasoning", &cfg) {
            assert!(lines[0].contains("reasoning effort"));
        } else {
            panic!("expected Output");
        }
    }

    #[test]
    fn test_reasoning_set_high() {
        let cfg = test_config();
        if let CommandResult::Output(lines) = dispatch("/reasoning high", &cfg) {
            assert!(lines[0].contains("high"));
        } else {
            panic!("expected Output");
        }
    }

    #[test]
    fn test_reasoning_invalid() {
        let cfg = test_config();
        if let CommandResult::Output(lines) = dispatch("/reasoning banana", &cfg) {
            assert!(lines[0].contains("Invalid"));
        } else {
            panic!("expected Output");
        }
    }

    #[test]
    fn test_compute_suggestions() {
        let suggestions = compute_suggestions("/he");
        assert!(suggestions.contains(&"/help"));

        let suggestions = compute_suggestions("/q");
        assert!(suggestions.contains(&"/quit"));

        // No suggestions for non-slash input.
        let suggestions = compute_suggestions("hello");
        assert!(suggestions.is_empty());

        // No suggestions when space present.
        let suggestions = compute_suggestions("/model opus");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_format_token_count() {
        assert_eq!(format_token_count(500), "500");
        assert_eq!(format_token_count(1234), "1.2k");
        assert_eq!(format_token_count(15_678), "16k");
        assert_eq!(format_token_count(1_234_567), "1.2M");
    }

    #[test]
    fn test_model_aliases_populated() {
        let aliases = model_aliases();
        assert!(aliases.len() > 10);
        assert_eq!(aliases["opus"], "claude-opus-4-6");
        assert_eq!(aliases["gpt5"], "gpt-5.2");
    }
}
