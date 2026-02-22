//! REPL (Read-Eval-Print Loop) for the OpenPlanter TUI.
//!
//! Port of Python's `RichREPL` from `tui.py`.  Provides a line-oriented REPL
//! that reads user input with crossterm raw-mode key events, dispatches slash
//! commands, and forwards objectives to the engine via a solve callback.
//!
//! Activity display (thinking / streaming / tool execution) is handled by
//! [`ActivityDisplay`](crate::activity::ActivityDisplay).
//!
//! ## Non-TTY fallback
//!
//! When stdin is not a terminal, the REPL falls back to a plain text mode
//! that reads whole lines without escape sequences or raw mode.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;


use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal;
use regex::Regex;
use serde_json::Value;


use op_core::AgentConfig;

use crate::activity::{ActivityDisplay, ActivityMode};
use crate::commands::{self, CommandResult};
use crate::demo::DemoCensor;
use crate::render::{self, StepState, ToolCallRecord};
use crate::splash;

// ---------------------------------------------------------------------------
// Event-parsing regexes (matching Python's _RE_* patterns)
// ---------------------------------------------------------------------------

/// Compiled regex patterns for parsing engine trace events.
struct EventPatterns {
    prefix: Regex,
    calling: Regex,
    subtask: Regex,
    execute: Regex,
    error: Regex,
    tool_start: Regex,
}

impl EventPatterns {
    fn new() -> Self {
        Self {
            prefix: Regex::new(r"^\[d(\d+)(?:/s(\d+))?\]\s*").unwrap(),
            calling: Regex::new(r"calling model").unwrap(),
            subtask: Regex::new(r">> entering subtask").unwrap(),
            execute: Regex::new(r">> executing leaf").unwrap(),
            error: Regex::new(r"(?i)model error:").unwrap(),
            tool_start: Regex::new(r"(\w+)\((.*)?\)$").unwrap(),
        }
    }
}

// ---------------------------------------------------------------------------
// Input history
// ---------------------------------------------------------------------------

/// Simple in-memory input history with optional file persistence (newest at end).
#[derive(Debug)]
pub struct InputHistory {
    entries: Vec<String>,
    /// Current browse position (past the end = new input).
    cursor: usize,
    path: Option<PathBuf>,
    max_entries: usize,
}

impl Default for InputHistory {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            path: None,
            max_entries: 500,
        }
    }
}

impl InputHistory {
    /// Create a new in-memory history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a history backed by a file on disk.
    pub fn with_file(path: PathBuf, max_entries: usize) -> Self {
        let mut entries = Vec::new();
        if let Ok(data) = std::fs::read_to_string(&path) {
            for line in data.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    entries.push(line.to_string());
                }
            }
            if entries.len() > max_entries {
                entries = entries[entries.len() - max_entries..].to_vec();
            }
        }
        let cursor = entries.len();
        Self {
            entries,
            cursor,
            path: Some(path),
            max_entries,
        }
    }

    /// Push a new entry.
    pub fn push(&mut self, entry: String) {
        let trimmed = entry.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        // Dedup: skip if identical to last entry.
        if self.entries.last().map(|s| s.as_str()) == Some(trimmed.as_str()) {
            self.cursor = self.entries.len();
            return;
        }
        self.entries.push(trimmed);
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
        self.cursor = self.entries.len();
        self.save();
    }

    /// Move backward in history (older).  Returns the entry or `None`.
    pub fn prev(&mut self) -> Option<&str> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        Some(&self.entries[self.cursor])
    }

    /// Move forward in history (newer).  Returns the entry or `None`.
    pub fn next(&mut self) -> Option<&str> {
        if self.cursor >= self.entries.len() {
            return None;
        }
        self.cursor += 1;
        if self.cursor >= self.entries.len() {
            None
        } else {
            Some(&self.entries[self.cursor])
        }
    }

    /// Reset browse position to the end.
    pub fn reset_cursor(&mut self) {
        self.cursor = self.entries.len();
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Persist to disk.
    fn save(&self) {
        if let Some(ref p) = self.path {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let data = self.entries.join("\n");
            let _ = std::fs::write(p, data);
        }
    }
}

// ---------------------------------------------------------------------------
// Inline line editor
// ---------------------------------------------------------------------------

/// Minimal single-line editor with cursor movement.
#[derive(Debug)]
struct LineEditor {
    buf: Vec<char>,
    cursor: usize,
}

impl LineEditor {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
        }
    }

    fn insert(&mut self, ch: char) {
        self.buf.insert(self.cursor, ch);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buf.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.buf.len() {
            self.cursor += 1;
        }
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.buf.len();
    }

    fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
    }

    fn set(&mut self, text: &str) {
        self.buf = text.chars().collect();
        self.cursor = self.buf.len();
    }

    fn kill_to_end(&mut self) {
        self.buf.truncate(self.cursor);
    }

    fn kill_to_start(&mut self) {
        self.buf.drain(..self.cursor);
        self.cursor = 0;
    }

    fn as_string(&self) -> String {
        self.buf.iter().collect()
    }

    fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Prompt rendering
// ---------------------------------------------------------------------------

const PROMPT_ANSI: &str = "\x1b[1;35myou> \x1b[0m";

/// Redraw the prompt line with the current editor state.
fn redraw_prompt(editor: &LineEditor) -> io::Result<()> {
    let mut stdout = io::stdout();
    // Move to start of line, clear it, print prompt + buffer.
    write!(stdout, "\r\x1b[K{}{}", PROMPT_ANSI, editor.as_string())?;
    // Position cursor correctly.
    let offset = editor.buf.len() - editor.cursor;
    if offset > 0 {
        write!(stdout, "\x1b[{}D", offset)?;
    }
    stdout.flush()
}

// ---------------------------------------------------------------------------
// Raw-mode line reading result
// ---------------------------------------------------------------------------

enum ReadLineResult {
    Line(String),
    Eof,
    Interrupted,
}

/// Read a single line using crossterm raw-mode key events.
///
/// Supports cursor movement, backspace, delete, Home/End, Up/Down history,
/// Ctrl+A/E/K/U, and Tab completion for slash commands.
fn read_line_raw(editor: &mut LineEditor, history: &mut InputHistory) -> ReadLineResult {
    history.reset_cursor();

    loop {
        // Poll with a short timeout so the REPL stays responsive.
        if !event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            continue;
        }

        let evt = match event::read() {
            Ok(e) => e,
            Err(_) => return ReadLineResult::Eof,
        };

        match evt {
            Event::Key(KeyEvent {
                code, modifiers, ..
            }) => {
                // Ctrl+C -> interrupt.
                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                    return ReadLineResult::Interrupted;
                }
                // Ctrl+D -> EOF (only when buffer is empty).
                if code == KeyCode::Char('d') && modifiers.contains(KeyModifiers::CONTROL) {
                    if editor.is_empty() {
                        return ReadLineResult::Eof;
                    }
                    editor.delete();
                    let _ = redraw_prompt(editor);
                    continue;
                }
                // Ctrl+A -> Home.
                if code == KeyCode::Char('a') && modifiers.contains(KeyModifiers::CONTROL) {
                    editor.move_home();
                    let _ = redraw_prompt(editor);
                    continue;
                }
                // Ctrl+E -> End.
                if code == KeyCode::Char('e') && modifiers.contains(KeyModifiers::CONTROL) {
                    editor.move_end();
                    let _ = redraw_prompt(editor);
                    continue;
                }
                // Ctrl+K -> Kill to end of line.
                if code == KeyCode::Char('k') && modifiers.contains(KeyModifiers::CONTROL) {
                    editor.kill_to_end();
                    let _ = redraw_prompt(editor);
                    continue;
                }
                // Ctrl+U -> Kill to start of line.
                if code == KeyCode::Char('u') && modifiers.contains(KeyModifiers::CONTROL) {
                    editor.kill_to_start();
                    let _ = redraw_prompt(editor);
                    continue;
                }

                match code {
                    KeyCode::Enter => {
                        return ReadLineResult::Line(editor.as_string());
                    }
                    KeyCode::Backspace => {
                        editor.backspace();
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::Delete => {
                        editor.delete();
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::Left => {
                        editor.move_left();
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::Right => {
                        editor.move_right();
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::Home => {
                        editor.move_home();
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::End => {
                        editor.move_end();
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::Up => {
                        if let Some(entry) = history.prev() {
                            editor.set(entry);
                        }
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::Down => {
                        match history.next() {
                            Some(entry) => editor.set(entry),
                            None => editor.clear(),
                        }
                        let _ = redraw_prompt(editor);
                    }
                    KeyCode::Tab => {
                        // Tab completion for slash commands.
                        let current = editor.as_string();
                        let suggestions = commands::compute_suggestions(&current);
                        if suggestions.len() == 1 {
                            editor.set(&format!("{} ", suggestions[0]));
                            let _ = redraw_prompt(editor);
                        }
                        // If multiple suggestions, do nothing (could show them).
                    }
                    KeyCode::Char(c) => {
                        editor.insert(c);
                        let _ = redraw_prompt(editor);
                    }
                    _ => {}
                }
            }
            Event::Resize(_, _) => {
                let _ = redraw_prompt(editor);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Repl
// ---------------------------------------------------------------------------

/// The interactive REPL that drives the OpenPlanter TUI session.
///
/// Reads user input with crossterm raw-mode key events (or plain stdin for
/// non-TTY), dispatches slash commands via [`commands::dispatch`], and
/// forwards objectives to the engine via callbacks.
pub struct Repl {
    config: AgentConfig,
    history: InputHistory,
    activity: ActivityDisplay,
    current_step: Option<StepState>,
    patterns: EventPatterns,
    censor: Option<DemoCensor>,
    /// Startup info key/value pairs to display after splash art.
    startup_info: HashMap<String, String>,
}

impl Repl {
    /// Create a new REPL for the given config.
    pub fn new(config: AgentConfig, startup_info: HashMap<String, String>) -> Self {
        let censor = if config.demo {
            Some(DemoCensor::new(&config.workspace))
        } else {
            None
        };
        let activity = ActivityDisplay::new(censor.clone());

        // Set up file-backed history.
        let history = {
            let history_dir = dirs_home().join(".openplanter");
            let history_path = history_dir.join("repl_history");
            InputHistory::with_file(history_path, 500)
        };

        Self {
            config,
            history,
            activity,
            current_step: None,
            patterns: EventPatterns::new(),
            censor,
            startup_info,
        }
    }

    // -- splash & banner ----------------------------------------------------

    /// Print the startup banner (splash art, info, help hint).
    fn print_banner(&self) {
        let mut stdout = io::stdout();

        // Clear screen.
        let _ = write!(stdout, "\x1b[2J\x1b[H");

        // Splash art in bold cyan.
        let _ = writeln!(stdout, "\x1b[1;36m{}\x1b[0m", splash::splash_art());
        let _ = writeln!(stdout);

        // Startup info.
        for (key, val) in &self.startup_info {
            let _ = writeln!(stdout, "  \x1b[2m{:>10}  {}\x1b[0m", key, val);
        }
        if !self.startup_info.is_empty() {
            let _ = writeln!(stdout);
        }

        let _ = writeln!(
            stdout,
            "\x1b[2mType /help for commands, Ctrl+D to exit.  Ctrl+C to cancel a running task.\x1b[0m"
        );
        let _ = writeln!(stdout);
        let _ = stdout.flush();
    }

    // -- event callbacks (matching Python's _on_event / _on_step) ----------

    /// Handle a trace event string from the engine/runtime.
    ///
    /// This is the `on_event` callback.
    pub fn on_event(&mut self, msg: &str) {
        let m = self.patterns.prefix.find(msg);
        let body = if let Some(mat) = m {
            &msg[mat.end()..]
        } else {
            msg
        };

        // Extract step label from prefix.
        let step_label = if let Some(caps) = self.patterns.prefix.captures(msg) {
            if let Some(s) = caps.get(2) {
                format!("Step {}/{}", s.as_str(), self.config.max_steps_per_call)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Calling model -> start thinking display.
        if self.patterns.calling.is_match(body) {
            self.flush_step();
            self.activity.start(ActivityMode::Thinking, &step_label);
            return;
        }

        // Subtask / execute entry.
        if self.patterns.subtask.is_match(body) || self.patterns.execute.is_match(body) {
            self.flush_step();
            self.activity.stop();
            let label = self
                .patterns
                .subtask
                .replace(body, "")
                .to_string();
            let label = self
                .patterns
                .execute
                .replace(&label, "")
                .trim()
                .to_string();
            let mut stdout = io::stdout();
            let _ = writeln!(stdout, "\x1b[2m{}\x1b[0m", render::clip_event(&label));
            let _ = stdout.flush();
            return;
        }

        // Error.
        if self.patterns.error.is_match(body) {
            self.activity.stop();
            let first_line = msg.split('\n').next().unwrap_or(msg);
            let clipped = render::clip_event(first_line);
            let mut stdout = io::stdout();
            let _ = writeln!(stdout, "\x1b[1;31m{}\x1b[0m", clipped);
            let _ = stdout.flush();
            return;
        }

        // Tool start (e.g. "read_file(path=foo.py)").
        if let Some(caps) = self.patterns.tool_start.captures(body) {
            let tool_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let tool_arg = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            self.activity
                .set_tool(tool_name, tool_arg, &step_label);
        }
    }

    /// Handle a structured step event from the engine.
    ///
    /// This is the `on_step` callback.
    pub fn on_step(&mut self, step_event: &Value) {
        let action = match step_event.get("action") {
            Some(a) if a.is_object() => a,
            _ => return,
        };
        let name = action
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if name == "_model_turn" {
            // Model turn completed.
            self.activity.stop();
            self.current_step = Some(StepState {
                depth: step_event
                    .get("depth")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                step: step_event
                    .get("step")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                max_steps: self.config.max_steps_per_call,
                model_text: step_event
                    .get("model_text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                model_elapsed_sec: step_event
                    .get("elapsed_sec")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                input_tokens: step_event
                    .get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                output_tokens: step_event
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                tool_calls: Vec::new(),
            });
            return;
        }

        if name == "final" {
            self.flush_step();
            return;
        }

        // Tool call -- append to current step.
        if let Some(ref mut step) = self.current_step {
            let arguments = action
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));
            let key_arg = render::extract_key_arg(name, &arguments);
            let elapsed = step_event
                .get("elapsed_sec")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let observation = step_event
                .get("observation")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_error =
                observation.starts_with("Tool ") && observation.contains("crashed");

            step.tool_calls.push(ToolCallRecord {
                name: name.to_string(),
                key_arg,
                elapsed_sec: elapsed,
                is_error,
            });
        }
    }

    /// Handle a content delta (thinking or text) from the engine.
    pub fn on_content_delta(&mut self, delta_type: &str, text: &str) {
        self.activity.feed(delta_type, text);
    }

    /// Flush and render the current completed step.
    fn flush_step(&mut self) {
        if let Some(step) = self.current_step.take() {
            // Default context window (200k for claude-opus-4-6).
            let context_window = 200_000;
            render::render_step(&step, context_window, self.censor.as_ref());
        }
    }

    /// Render the final answer.
    pub fn present_result(&mut self, answer: &str) {
        self.activity.stop();
        self.flush_step();

        let mut stdout = io::stdout();
        let _ = writeln!(stdout);
        render::render_markdown(answer, self.censor.as_ref());
        let _ = writeln!(stdout);
        let _ = stdout.flush();
    }

    // -- main loop (TTY raw-mode) -------------------------------------------

    /// Run the interactive REPL loop.
    ///
    /// Uses crossterm raw mode for key-by-key input when stdin is a TTY,
    /// or falls back to line-buffered stdin otherwise.
    ///
    /// The `solve_fn` closure is called for each non-command objective.
    /// It receives `(objective, &mut Repl)` and should:
    /// 1. Call `repl.on_event()`, `repl.on_step()`, `repl.on_content_delta()`
    ///    as engine callbacks fire.
    /// 2. Call `repl.present_result(&answer)` when done.
    /// 3. Return `Ok(())` or `Err(error_message)`.
    pub fn run<F>(&mut self, mut solve_fn: F)
    where
        F: FnMut(&str, &mut ReplCallbacks) -> Result<(), String>,
    {
        if !is_tty() {
            self.run_plain(solve_fn);
            return;
        }

        self.print_banner();

        let mut stdout = io::stdout();
        let mut editor = LineEditor::new();

        // Enable raw mode.
        let was_raw = terminal::is_raw_mode_enabled().unwrap_or(false);
        if !was_raw {
            let _ = terminal::enable_raw_mode();
        }

        loop {
            // Print prompt.
            editor.clear();
            let _ = redraw_prompt(&editor);

            // Read a line via raw-mode key events.
            let line = match read_line_raw(&mut editor, &mut self.history) {
                ReadLineResult::Line(line) => line,
                ReadLineResult::Eof => break,
                ReadLineResult::Interrupted => {
                    let _ = writeln!(stdout);
                    let _ = stdout.flush();
                    continue;
                }
            };

            // Echo newline after Enter.
            let _ = writeln!(stdout);
            let _ = stdout.flush();

            let input = line.trim().to_string();
            if input.is_empty() {
                continue;
            }

            self.history.push(input.clone());

            // Dispatch slash commands.
            let result = commands::dispatch(&input, &self.config);
            match result {
                CommandResult::Quit => break,
                CommandResult::Clear => {
                    // Leave raw mode briefly for ANSI clear.
                    let _ = terminal::disable_raw_mode();
                    let _ = write!(stdout, "\x1b[2J\x1b[H");
                    let _ = stdout.flush();
                    let _ = terminal::enable_raw_mode();
                    continue;
                }
                CommandResult::Output(lines) => {
                    let _ = terminal::disable_raw_mode();
                    for ln in &lines {
                        let _ = writeln!(stdout, "\x1b[36m{}\x1b[0m", ln);
                    }
                    let _ = stdout.flush();
                    let _ = terminal::enable_raw_mode();
                    continue;
                }
                CommandResult::NotACommand => {}
            }

            // Objective for the engine -- leave raw mode for output.
            let _ = terminal::disable_raw_mode();
            let _ = writeln!(stdout);
            let _ = stdout.flush();

            // Build a callbacks struct that the solve_fn can use.
            let mut callbacks = ReplCallbacks {
                activity: &self.activity,
                current_step: &mut self.current_step,
                config: &self.config,
                patterns: &self.patterns,
                censor: self.censor.as_ref(),
            };

            match solve_fn(&input, &mut callbacks) {
                Ok(()) => {}
                Err(err) => {
                    self.activity.stop();
                    let _ = writeln!(stdout, "\x1b[1;31mError: {}\x1b[0m", err);
                    let _ = writeln!(stdout);
                    let _ = stdout.flush();
                }
            }

            // Re-enable raw mode for next prompt.
            let _ = terminal::enable_raw_mode();
        }

        // Restore terminal state.
        if !was_raw {
            let _ = terminal::disable_raw_mode();
        }
        let _ = writeln!(stdout, "\x1b[2mGoodbye.\x1b[0m");
        let _ = stdout.flush();
    }

    // -- plain-text REPL (non-TTY fallback) ---------------------------------

    /// Non-interactive REPL for piped input.
    fn run_plain<F>(&mut self, mut solve_fn: F)
    where
        F: FnMut(&str, &mut ReplCallbacks) -> Result<(), String>,
    {
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line_result in stdin.lock().lines() {
            let line = match line_result {
                Ok(l) => l.trim().to_string(),
                Err(_) => break,
            };
            if line.is_empty() {
                continue;
            }

            let result = commands::dispatch(&line, &self.config);
            match result {
                CommandResult::Quit => break,
                CommandResult::Clear => continue,
                CommandResult::Output(lines) => {
                    for l in &lines {
                        let _ = writeln!(stdout, "{}", l);
                    }
                    continue;
                }
                CommandResult::NotACommand => {}
            }

            let mut callbacks = ReplCallbacks {
                activity: &self.activity,
                current_step: &mut self.current_step,
                config: &self.config,
                patterns: &self.patterns,
                censor: self.censor.as_ref(),
            };

            if let Err(err) = solve_fn(&line, &mut callbacks) {
                let _ = writeln!(stdout, "Error: {}", err);
            }
            self.activity.stop();
        }
    }

    // -- accessors ----------------------------------------------------------

    /// Reference to the activity display.
    pub fn activity(&self) -> &ActivityDisplay {
        &self.activity
    }

    /// Mutable reference to the config.
    pub fn config_mut(&mut self) -> &mut AgentConfig {
        &mut self.config
    }

    /// Reference to the config.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// ReplCallbacks -- passed to solve_fn so it can drive the display
// ---------------------------------------------------------------------------

/// A bundle of references that the solve function can use to drive the REPL
/// display while the engine is running.
pub struct ReplCallbacks<'a> {
    pub activity: &'a ActivityDisplay,
    current_step: &'a mut Option<StepState>,
    config: &'a AgentConfig,
    patterns: &'a EventPatterns,
    censor: Option<&'a DemoCensor>,
}

impl<'a> ReplCallbacks<'a> {
    /// Handle a trace event string from the engine.
    pub fn on_event(&mut self, msg: &str) {
        let m = self.patterns.prefix.find(msg);
        let body = if let Some(mat) = m {
            &msg[mat.end()..]
        } else {
            msg
        };

        let step_label = if let Some(caps) = self.patterns.prefix.captures(msg) {
            if let Some(s) = caps.get(2) {
                format!("Step {}/{}", s.as_str(), self.config.max_steps_per_call)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if self.patterns.calling.is_match(body) {
            self.flush_step();
            self.activity.start(ActivityMode::Thinking, &step_label);
            return;
        }

        if self.patterns.subtask.is_match(body) || self.patterns.execute.is_match(body) {
            self.flush_step();
            self.activity.stop();
            let label = self.patterns.subtask.replace(body, "").to_string();
            let label = self.patterns.execute.replace(&label, "").trim().to_string();
            let mut stdout = io::stdout();
            let _ = writeln!(stdout, "\x1b[2m{}\x1b[0m", render::clip_event(&label));
            let _ = stdout.flush();
            return;
        }

        if self.patterns.error.is_match(body) {
            self.activity.stop();
            let first_line = msg.split('\n').next().unwrap_or(msg);
            let clipped = render::clip_event(first_line);
            let mut stdout = io::stdout();
            let _ = writeln!(stdout, "\x1b[1;31m{}\x1b[0m", clipped);
            let _ = stdout.flush();
            return;
        }

        if let Some(caps) = self.patterns.tool_start.captures(body) {
            let tool_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let tool_arg = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            self.activity.set_tool(tool_name, tool_arg, &step_label);
        }
    }

    /// Handle a structured step event from the engine.
    pub fn on_step(&mut self, step_event: &Value) {
        let action = match step_event.get("action") {
            Some(a) if a.is_object() => a,
            _ => return,
        };
        let name = action
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if name == "_model_turn" {
            self.activity.stop();
            *self.current_step = Some(StepState {
                depth: step_event.get("depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                step: step_event.get("step").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                max_steps: self.config.max_steps_per_call,
                model_text: step_event.get("model_text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                model_elapsed_sec: step_event.get("elapsed_sec").and_then(|v| v.as_f64()).unwrap_or(0.0),
                input_tokens: step_event.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output_tokens: step_event.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                tool_calls: Vec::new(),
            });
            return;
        }

        if name == "final" {
            self.flush_step();
            return;
        }

        if let Some(ref mut step) = self.current_step {
            let arguments = action.get("arguments").cloned().unwrap_or(Value::Object(serde_json::Map::new()));
            let key_arg = render::extract_key_arg(name, &arguments);
            let elapsed = step_event.get("elapsed_sec").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let observation = step_event.get("observation").and_then(|v| v.as_str()).unwrap_or("");
            let is_error = observation.starts_with("Tool ") && observation.contains("crashed");

            step.tool_calls.push(ToolCallRecord {
                name: name.to_string(),
                key_arg,
                elapsed_sec: elapsed,
                is_error,
            });
        }
    }

    /// Handle a content delta (thinking or text).
    pub fn on_content_delta(&self, delta_type: &str, text: &str) {
        self.activity.feed(delta_type, text);
    }

    /// Render the final answer.
    pub fn present_result(&mut self, answer: &str) {
        self.activity.stop();
        self.flush_step();

        let mut stdout = io::stdout();
        let _ = writeln!(stdout);
        render::render_markdown(answer, self.censor);
        let _ = writeln!(stdout);
        let _ = stdout.flush();
    }

    /// Flush and render the current step.
    fn flush_step(&mut self) {
        if let Some(step) = self.current_step.take() {
            let context_window = 200_000;
            render::render_step(&step, context_window, self.censor);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if stdin is a TTY.
fn is_tty() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = io::stdin().as_raw_fd();
        // SAFETY: isatty is a safe POSIX function.
        unsafe { libc_isatty(fd) != 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(unix)]
extern "C" {
    fn isatty(fd: i32) -> i32;
}

#[cfg(unix)]
unsafe fn libc_isatty(fd: i32) -> i32 {
    unsafe { isatty(fd) }
}

/// Get the user's home directory.
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
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

    // -- InputHistory -------------------------------------------------------

    #[test]
    fn test_input_history_push_and_browse() {
        let mut h = InputHistory::new();
        h.push("first".to_string());
        h.push("second".to_string());
        h.push("third".to_string());

        assert_eq!(h.len(), 3);

        // Browse backward.
        assert_eq!(h.prev(), Some("third"));
        assert_eq!(h.prev(), Some("second"));
        assert_eq!(h.prev(), Some("first"));
        assert_eq!(h.prev(), None);

        // Browse forward.
        assert_eq!(h.next(), Some("second"));
        assert_eq!(h.next(), Some("third"));
        assert_eq!(h.next(), None);
    }

    #[test]
    fn test_input_history_empty() {
        let mut h = InputHistory::new();
        assert!(h.is_empty());
        assert_eq!(h.prev(), None);
        assert_eq!(h.next(), None);
    }

    #[test]
    fn test_input_history_ignores_whitespace_only() {
        let mut h = InputHistory::new();
        h.push("  ".to_string());
        assert!(h.is_empty());
    }

    #[test]
    fn test_input_history_dedup() {
        let mut h = InputHistory::new();
        h.push("same".to_string());
        h.push("same".to_string());
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn test_input_history_reset_cursor() {
        let mut h = InputHistory::new();
        h.push("a".to_string());
        h.push("b".to_string());
        let _ = h.prev();
        let _ = h.prev();
        h.reset_cursor();
        assert_eq!(h.prev(), Some("b"));
    }

    #[test]
    fn test_input_history_max_entries() {
        let mut h = InputHistory {
            entries: Vec::new(),
            cursor: 0,
            path: None,
            max_entries: 3,
        };
        h.push("a".to_string());
        h.push("b".to_string());
        h.push("c".to_string());
        h.push("d".to_string());
        assert_eq!(h.len(), 3);
        h.reset_cursor();
        assert_eq!(h.prev(), Some("d"));
        assert_eq!(h.prev(), Some("c"));
        assert_eq!(h.prev(), Some("b"));
        assert_eq!(h.prev(), None);
    }

    // -- LineEditor ---------------------------------------------------------

    #[test]
    fn test_line_editor_insert() {
        let mut ed = LineEditor::new();
        ed.insert('h');
        ed.insert('i');
        assert_eq!(ed.as_string(), "hi");
        assert_eq!(ed.cursor, 2);
    }

    #[test]
    fn test_line_editor_backspace() {
        let mut ed = LineEditor::new();
        ed.insert('a');
        ed.insert('b');
        ed.insert('c');
        ed.backspace();
        assert_eq!(ed.as_string(), "ab");
    }

    #[test]
    fn test_line_editor_move_and_insert() {
        let mut ed = LineEditor::new();
        ed.insert('a');
        ed.insert('b');
        ed.insert('c');
        ed.move_left();
        ed.move_left();
        ed.insert('X');
        assert_eq!(ed.as_string(), "aXbc");
    }

    #[test]
    fn test_line_editor_home_end() {
        let mut ed = LineEditor::new();
        ed.insert('a');
        ed.insert('b');
        ed.move_home();
        assert_eq!(ed.cursor, 0);
        ed.move_end();
        assert_eq!(ed.cursor, 2);
    }

    #[test]
    fn test_line_editor_kill_to_end() {
        let mut ed = LineEditor::new();
        ed.set("hello world");
        ed.cursor = 5;
        ed.kill_to_end();
        assert_eq!(ed.as_string(), "hello");
    }

    #[test]
    fn test_line_editor_kill_to_start() {
        let mut ed = LineEditor::new();
        ed.set("hello world");
        ed.cursor = 5;
        ed.kill_to_start();
        assert_eq!(ed.as_string(), " world");
    }

    #[test]
    fn test_line_editor_delete() {
        let mut ed = LineEditor::new();
        ed.set("abc");
        ed.cursor = 1;
        ed.delete();
        assert_eq!(ed.as_string(), "ac");
    }

    #[test]
    fn test_line_editor_set_and_clear() {
        let mut ed = LineEditor::new();
        ed.set("hello");
        assert_eq!(ed.as_string(), "hello");
        assert_eq!(ed.cursor, 5);
        ed.clear();
        assert!(ed.is_empty());
        assert_eq!(ed.cursor, 0);
    }

    // -- Repl construction --------------------------------------------------

    #[test]
    fn test_repl_creation() {
        let cfg = test_config();
        let repl = Repl::new(cfg, HashMap::new());
        assert!(repl.current_step.is_none());
    }

    #[test]
    fn test_event_patterns_compile() {
        let _ = EventPatterns::new();
    }

    // -- on_step ------------------------------------------------------------

    #[test]
    fn test_on_step_model_turn() {
        let cfg = test_config();
        let mut repl = Repl::new(cfg, HashMap::new());

        let step_event = serde_json::json!({
            "depth": 0,
            "step": 1,
            "action": {"name": "_model_turn"},
            "model_text": "thinking about the problem",
            "elapsed_sec": 1.5,
            "input_tokens": 5000,
            "output_tokens": 1200,
        });

        repl.on_step(&step_event);
        assert!(repl.current_step.is_some());
        let step = repl.current_step.as_ref().unwrap();
        assert_eq!(step.step, 1);
        assert_eq!(step.input_tokens, 5000);
    }

    #[test]
    fn test_on_step_tool_call() {
        let cfg = test_config();
        let mut repl = Repl::new(cfg, HashMap::new());

        let model_event = serde_json::json!({
            "depth": 0,
            "step": 1,
            "action": {"name": "_model_turn"},
            "model_text": "",
            "elapsed_sec": 1.0,
            "input_tokens": 1000,
            "output_tokens": 500,
        });
        repl.on_step(&model_event);

        let tool_event = serde_json::json!({
            "depth": 0,
            "step": 1,
            "action": {"name": "read_file", "arguments": {"path": "/tmp/foo.rs"}},
            "observation": "file content here",
            "elapsed_sec": 0.3,
        });
        repl.on_step(&tool_event);

        let step = repl.current_step.as_ref().unwrap();
        assert_eq!(step.tool_calls.len(), 1);
        assert_eq!(step.tool_calls[0].name, "read_file");
        assert_eq!(step.tool_calls[0].key_arg, "/tmp/foo.rs");
    }

    #[test]
    fn test_on_step_final_flushes() {
        let cfg = test_config();
        let mut repl = Repl::new(cfg, HashMap::new());

        let model_event = serde_json::json!({
            "depth": 0,
            "step": 1,
            "action": {"name": "_model_turn"},
            "model_text": "done",
            "elapsed_sec": 0.5,
            "input_tokens": 100,
            "output_tokens": 50,
        });
        repl.on_step(&model_event);
        assert!(repl.current_step.is_some());

        let final_event = serde_json::json!({
            "action": {"name": "final", "arguments": {"text": "The answer"}},
            "observation": "The answer",
            "is_final": true,
        });
        repl.on_step(&final_event);
        assert!(repl.current_step.is_none());
    }
}
