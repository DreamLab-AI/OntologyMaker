//! TuiApp -- top-level application entry point for the OpenPlanter TUI.
//!
//! Port of Python's `RichREPL` and `run_rich_repl()` into an inline (non-
//! full-screen) REPL application.  The [`TuiApp`] struct owns the config,
//! settings store, and an optional demo censor.
//!
//! The REPL intentionally does NOT use ratatui's alternate-screen / full-screen
//! mode.  Instead it writes inline to stdout using ANSI escape codes, matching
//! the Rich `Console.print` / `Live` pattern from the Python codebase.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use op_core::AgentConfig;

use crate::commands::{self, CommandResult};
use crate::repl::{InputHistory, Repl, ReplCallbacks};

// ---------------------------------------------------------------------------
// TuiAppConfig
// ---------------------------------------------------------------------------

/// Configuration for the TUI application.
#[derive(Debug, Clone)]
pub struct TuiAppConfig {
    pub workspace_root: PathBuf,
    pub agent_config: AgentConfig,
    pub demo_mode: bool,
    pub model_name: String,
    pub provider: String,
    /// When true, skip terminal UI and run headless (for testing).
    pub headless: bool,
}

// ---------------------------------------------------------------------------
// TuiApp
// ---------------------------------------------------------------------------

/// The main TUI application.
///
/// Creates and runs the interactive inline REPL, managing terminal lifecycle
/// and engine integration.
///
/// The `input`, `cursor_pos`, `history`, and `should_quit` fields are used
/// by the headless / testing input pipeline (see `handle_key`).
#[allow(dead_code)]
pub struct TuiApp {
    config: TuiAppConfig,
    /// Conversation messages (kept for history / potential replay).
    messages: Vec<DisplayMessage>,
    /// Current user input buffer.
    input: String,
    /// Cursor position within `input`.
    cursor_pos: usize,
    /// Input history.
    history: InputHistory,
    /// Status information.
    status: StatusInfo,
    /// Whether the app should quit on the next iteration.
    should_quit: bool,
}

/// A message in the conversation display.
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: MessageRole,
    pub content: String,
    pub tool_name: Option<String>,
    pub elapsed_sec: Option<f64>,
}

/// The role of a message in the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    /// User input.
    User,
    /// Agent / model response.
    Assistant,
    /// System message (help, status, splash).
    System,
    /// Tool call / observation.
    Tool,
}

/// Status bar information.
#[derive(Debug, Clone, Default)]
pub struct StatusInfo {
    pub provider: String,
    pub model_name: String,
    pub mode: String,
    pub token_summary: String,
    pub activity: Option<String>,
}

impl TuiApp {
    /// Create a new TuiApp with the given configuration.
    pub fn new(config: TuiAppConfig) -> Self {
        let status = StatusInfo {
            provider: config.provider.clone(),
            model_name: config.model_name.clone(),
            mode: if config.agent_config.recursive {
                "recursive".to_string()
            } else {
                "flat".to_string()
            },
            token_summary: String::new(),
            activity: None,
        };

        Self {
            config,
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            history: InputHistory::new(),
            status,
            should_quit: false,
        }
    }

    // =====================================================================
    // Inline REPL mode (the primary mode)
    // =====================================================================

    /// Run the inline REPL (non-full-screen).
    ///
    /// This is the primary entry point.  It creates a [`Repl`] and runs its
    /// interactive loop with a placeholder solve function.
    ///
    /// When `op-engine` and `op-runtime` are fully wired, callers should use
    /// [`run_with_solve()`](TuiApp::run_with_solve) to provide a real engine
    /// callback instead.
    pub fn run_inline(self) {
        let mut startup_info = HashMap::new();
        startup_info.insert("model".to_string(), self.config.model_name.clone());
        startup_info.insert("provider".to_string(), self.config.provider.clone());
        startup_info.insert(
            "workspace".to_string(),
            self.config.workspace_root.display().to_string(),
        );
        if self.config.agent_config.recursive {
            startup_info.insert("mode".to_string(), "recursive".to_string());
        }
        if self.config.demo_mode {
            startup_info.insert("demo".to_string(), "enabled".to_string());
        }

        let mut repl = Repl::new(self.config.agent_config, startup_info);
        repl.run(|objective, _callbacks| {
            // Placeholder: engine not yet wired.
            let mut stdout = io::stdout();
            let _ = writeln!(
                stdout,
                "\x1b[2m(Engine not connected in standalone REPL mode. \
                 Use TuiApp::run_with_solve() for full functionality.)\x1b[0m"
            );
            let _ = writeln!(stdout, "\x1b[2mObjective: {}\x1b[0m", objective);
            let _ = stdout.flush();
            Ok(())
        });
    }

    /// Run the inline REPL with a custom solve function.
    ///
    /// The `solve_fn` closure receives:
    /// - `objective: &str` -- the user's input.
    /// - `callbacks: &mut ReplCallbacks` -- to drive the display (activity,
    ///   step rendering, result presentation).
    ///
    /// It should call `callbacks.on_event()`, `callbacks.on_step()`,
    /// `callbacks.on_content_delta()`, and `callbacks.present_result()`
    /// as appropriate, then return `Ok(())` or `Err(error_message)`.
    pub fn run_with_solve<F>(self, solve_fn: F)
    where
        F: FnMut(&str, &mut ReplCallbacks) -> Result<(), String>,
    {
        let mut startup_info = HashMap::new();
        startup_info.insert("model".to_string(), self.config.model_name.clone());
        startup_info.insert("provider".to_string(), self.config.provider.clone());
        startup_info.insert(
            "workspace".to_string(),
            self.config.workspace_root.display().to_string(),
        );
        if self.config.agent_config.recursive {
            startup_info.insert("mode".to_string(), "recursive".to_string());
        }
        if self.config.demo_mode {
            startup_info.insert("demo".to_string(), "enabled".to_string());
        }

        let mut repl = Repl::new(self.config.agent_config, startup_info);
        repl.run(solve_fn);
    }

    // =====================================================================
    // Input handling (for testing / headless mode)
    // =====================================================================

    /// Handle a key event (used in headless/testing mode).
    #[allow(dead_code)]
    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Enter => {
                self.submit_input();
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
            }
            KeyCode::Up => {
                if let Some(entry) = self.history.prev() {
                    self.input = entry.to_string();
                    self.cursor_pos = self.input.len();
                }
            }
            KeyCode::Down => {
                if let Some(entry) = self.history.next() {
                    self.input = entry.to_string();
                    self.cursor_pos = self.input.len();
                } else {
                    self.input.clear();
                    self.cursor_pos = 0;
                }
            }
            KeyCode::Esc => {
                self.input.clear();
                self.cursor_pos = 0;
            }
            _ => {}
        }
    }

    #[allow(dead_code)]
    fn submit_input(&mut self) {
        let input = self.input.trim().to_string();
        self.input.clear();
        self.cursor_pos = 0;
        self.history.reset_cursor();

        if input.is_empty() {
            return;
        }

        self.history.push(input.clone());

        self.messages.push(DisplayMessage {
            role: MessageRole::User,
            content: input.clone(),
            tool_name: None,
            elapsed_sec: None,
        });

        let result = commands::dispatch(&input, &self.config.agent_config);
        match result {
            CommandResult::Quit => {
                self.should_quit = true;
            }
            CommandResult::Clear => {
                self.messages.clear();
            }
            CommandResult::Output(lines) => {
                let content = lines.join("\n");
                self.messages.push(DisplayMessage {
                    role: MessageRole::System,
                    content,
                    tool_name: None,
                    elapsed_sec: None,
                });
            }
            CommandResult::NotACommand => {
                self.messages.push(DisplayMessage {
                    role: MessageRole::Assistant,
                    content: format!(
                        "(Engine integration pending. Objective received: \"{}\")",
                        input
                    ),
                    tool_name: None,
                    elapsed_sec: None,
                });
            }
        }
    }

    // -- public accessors ---------------------------------------------------

    /// Get the workspace root path.
    pub fn workspace_root(&self) -> &PathBuf {
        &self.config.workspace_root
    }

    /// Check if demo mode is enabled.
    pub fn is_demo_mode(&self) -> bool {
        self.config.demo_mode
    }

    /// Reference to the current agent config.
    pub fn agent_config(&self) -> &AgentConfig {
        &self.config.agent_config
    }

    /// The conversation messages.
    pub fn messages(&self) -> &[DisplayMessage] {
        &self.messages
    }

    /// Push an assistant message into the conversation.
    pub fn push_assistant_message(&mut self, content: String) {
        self.messages.push(DisplayMessage {
            role: MessageRole::Assistant,
            content,
            tool_name: None,
            elapsed_sec: None,
        });
    }

    /// Push a tool-call message into the conversation.
    pub fn push_tool_message(
        &mut self,
        tool_name: &str,
        content: String,
        elapsed_sec: f64,
    ) {
        self.messages.push(DisplayMessage {
            role: MessageRole::Tool,
            content,
            tool_name: Some(tool_name.to_string()),
            elapsed_sec: Some(elapsed_sec),
        });
    }

    /// Update the status bar information.
    pub fn set_status(&mut self, status: StatusInfo) {
        self.status = status;
    }

    /// Set the activity string shown in the status bar.
    pub fn set_activity(&mut self, activity: Option<String>) {
        self.status.activity = activity;
    }
}

// ---------------------------------------------------------------------------
// Convenience entry points
// ---------------------------------------------------------------------------

/// One-shot entry point: build a `TuiApp` from the workspace path and run it.
///
/// This matches the Python `run_rich_repl(ctx, startup_info)` function.
pub fn run_tui(workspace: &Path) {
    let config = AgentConfig::from_env(workspace);
    let app_config = TuiAppConfig {
        workspace_root: workspace.to_path_buf(),
        agent_config: config.clone(),
        demo_mode: config.demo,
        model_name: config.model.clone(),
        provider: config.provider.clone(),
        headless: false,
    };
    let app = TuiApp::new(app_config);
    app.run_inline();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app_config() -> TuiAppConfig {
        TuiAppConfig {
            workspace_root: PathBuf::from("/tmp/test"),
            agent_config: AgentConfig::from_env(Path::new("/tmp/test")),
            demo_mode: false,
            model_name: "test-model".to_string(),
            provider: "test-provider".to_string(),
            headless: true,
        }
    }

    #[test]
    fn test_tui_app_new() {
        let app = TuiApp::new(test_app_config());
        assert!(!app.should_quit);
        assert!(app.messages.is_empty());
        assert!(app.input.is_empty());
        assert_eq!(app.cursor_pos, 0);
    }

    #[test]
    fn test_tui_app_accessors() {
        let config = test_app_config();
        let app = TuiApp::new(config);
        assert!(!app.is_demo_mode());
        assert_eq!(app.workspace_root(), &PathBuf::from("/tmp/test"));
    }

    #[test]
    fn test_handle_key_char() {
        let mut app = TuiApp::new(test_app_config());

        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

        assert_eq!(app.input, "hi");
        assert_eq!(app.cursor_pos, 2);
    }

    #[test]
    fn test_handle_key_backspace() {
        let mut app = TuiApp::new(test_app_config());

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        assert_eq!(app.input, "a");
        assert_eq!(app.cursor_pos, 1);
    }

    #[test]
    fn test_handle_key_cursor_movement() {
        let mut app = TuiApp::new(test_app_config());

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.cursor_pos, 2);

        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.cursor_pos, 0);

        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(app.cursor_pos, 3);
    }

    #[test]
    fn test_handle_key_escape_clears() {
        let mut app = TuiApp::new(test_app_config());

        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(app.input.is_empty());
        assert_eq!(app.cursor_pos, 0);
    }

    #[test]
    fn test_handle_key_ctrl_c_quits() {
        let mut app = TuiApp::new(test_app_config());
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn test_submit_empty_input() {
        let mut app = TuiApp::new(test_app_config());
        app.input = "  ".to_string();
        app.submit_input();
        assert!(app.messages.is_empty());
    }

    #[test]
    fn test_submit_slash_help() {
        let mut app = TuiApp::new(test_app_config());
        app.input = "/help".to_string();
        app.cursor_pos = 5;
        app.submit_input();
        // Should have user message + system output.
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[0].role, MessageRole::User);
        assert_eq!(app.messages[1].role, MessageRole::System);
    }

    #[test]
    fn test_submit_quit() {
        let mut app = TuiApp::new(test_app_config());
        app.input = "/quit".to_string();
        app.cursor_pos = 5;
        app.submit_input();
        assert!(app.should_quit);
    }

    #[test]
    fn test_submit_clear() {
        let mut app = TuiApp::new(test_app_config());
        app.messages.push(DisplayMessage {
            role: MessageRole::System,
            content: "old".to_string(),
            tool_name: None,
            elapsed_sec: None,
        });
        app.input = "/clear".to_string();
        app.cursor_pos = 6;
        app.submit_input();
        assert!(app.messages.is_empty());
    }

    #[test]
    fn test_submit_regular_objective() {
        let mut app = TuiApp::new(test_app_config());
        app.input = "fix the bug".to_string();
        app.cursor_pos = 11;
        app.submit_input();
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.messages[0].role, MessageRole::User);
        assert_eq!(app.messages[1].role, MessageRole::Assistant);
    }

    #[test]
    fn test_push_assistant_message() {
        let mut app = TuiApp::new(test_app_config());
        app.push_assistant_message("Hello from the model".to_string());
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].role, MessageRole::Assistant);
    }

    #[test]
    fn test_push_tool_message() {
        let mut app = TuiApp::new(test_app_config());
        app.push_tool_message("read_file", "file content".to_string(), 0.5);
        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].role, MessageRole::Tool);
        assert_eq!(app.messages[0].tool_name.as_deref(), Some("read_file"));
    }

    #[test]
    fn test_set_activity() {
        let mut app = TuiApp::new(test_app_config());
        app.set_activity(Some("Thinking...".to_string()));
        assert_eq!(app.status.activity.as_deref(), Some("Thinking..."));
        app.set_activity(None);
        assert!(app.status.activity.is_none());
    }

    #[test]
    fn test_status_initialized_from_config() {
        let mut config = test_app_config();
        config.provider = "anthropic".to_string();
        config.model_name = "claude-opus-4-6".to_string();
        config.agent_config.recursive = true;
        let app = TuiApp::new(config);
        assert_eq!(app.status.provider, "anthropic");
        assert_eq!(app.status.model_name, "claude-opus-4-6");
        assert_eq!(app.status.mode, "recursive");
    }
}
