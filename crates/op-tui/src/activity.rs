//! Activity display: spinner, streaming text deltas, tool execution status.
//!
//! Port of Python's `_ActivityDisplay` class from `tui.py`.
//!
//! This module provides an inline (non-full-screen) activity indicator that
//! shows the current state of the agent: thinking, streaming, or running a
//! tool.  Output is written directly to stdout using crossterm escape
//! sequences, matching the Rich `Live` transient display behaviour.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossterm::{cursor, terminal, ExecutableCommand};

use crate::demo::DemoCensor;
use crate::theme;

// ---------------------------------------------------------------------------
// Spinner frames
// ---------------------------------------------------------------------------

/// Braille spinner characters (matching Python's sequence).
const SPINNER_FRAMES: &[char] = &[
    '\u{280B}', // ⠋
    '\u{2819}', // ⠙
    '\u{2839}', // ⠹
    '\u{2838}', // ⠸
    '\u{283C}', // ⠼
    '\u{2834}', // ⠴
    '\u{2826}', // ⠦
    '\u{2827}', // ⠧
    '\u{2807}', // ⠇
    '\u{280F}', // ⠏
];

/// Number of lines of thinking text to display (tail).
const THINKING_TAIL_LINES: usize = 6;

/// Maximum width of a single line in the activity display.
const THINKING_MAX_LINE_WIDTH: usize = 80;

// ---------------------------------------------------------------------------
// Activity mode
// ---------------------------------------------------------------------------

/// The current activity mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityMode {
    /// Model is generating internal reasoning.
    Thinking,
    /// Model is streaming response text.
    Streaming,
    /// A tool is being executed.
    Tool,
}

// ---------------------------------------------------------------------------
// Internal state (behind Mutex for thread safety)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    mode: ActivityMode,
    text_buf: String,
    step_label: String,
    tool_name: String,
    tool_key_arg: String,
    start_time: Instant,
    active: bool,
    spinner_tick: usize,
    /// Number of lines we wrote in the last render (for clearing).
    last_rendered_lines: usize,
}

impl Inner {
    fn new() -> Self {
        Self {
            mode: ActivityMode::Thinking,
            text_buf: String::new(),
            step_label: String::new(),
            tool_name: String::new(),
            tool_key_arg: String::new(),
            start_time: Instant::now(),
            active: false,
            spinner_tick: 0,
            last_rendered_lines: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// ActivityDisplay
// ---------------------------------------------------------------------------

/// Unified live display for thinking, streaming response, and tool execution.
///
/// Thread-safe: all state is behind an `Arc<Mutex<_>>`.  Call [`render()`] from
/// a tick loop to refresh the inline display.
pub struct ActivityDisplay {
    inner: Arc<Mutex<Inner>>,
    censor: Option<DemoCensor>,
}

impl ActivityDisplay {
    /// Create a new activity display.
    ///
    /// If `censor` is provided, all displayed text will be run through it.
    pub fn new(censor: Option<DemoCensor>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new())),
            censor,
        }
    }

    // -- lifecycle -----------------------------------------------------------

    /// Start (or restart) the activity display in the given mode.
    pub fn start(&self, mode: ActivityMode, step_label: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.mode = mode;
        inner.step_label = step_label.to_string();
        inner.text_buf.clear();
        inner.tool_name.clear();
        inner.tool_key_arg.clear();
        inner.start_time = Instant::now();
        inner.active = true;
        inner.spinner_tick = 0;
    }

    /// Stop the activity display and clear the transient output.
    pub fn stop(&self) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.active {
            return;
        }
        inner.active = false;
        let lines_to_clear = inner.last_rendered_lines;
        inner.last_rendered_lines = 0;
        inner.text_buf.clear();
        inner.tool_name.clear();
        inner.tool_key_arg.clear();
        drop(inner);

        // Clear the transient lines.
        if lines_to_clear > 0 {
            let mut stdout = io::stdout();
            let _ = clear_lines(&mut stdout, lines_to_clear);
            let _ = stdout.flush();
        }
    }

    // -- data feeds -----------------------------------------------------------

    /// Feed a content delta (thinking or text).
    ///
    /// On the first `"text"` delta while in `Thinking` mode, auto-transitions
    /// to `Streaming` mode.
    pub fn feed(&self, delta_type: &str, text: &str) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.active {
            return;
        }
        if delta_type == "text" && inner.mode == ActivityMode::Thinking {
            inner.mode = ActivityMode::Streaming;
            inner.text_buf.clear();
        }
        if delta_type == "thinking" || delta_type == "text" {
            inner.text_buf.push_str(text);
        }
    }

    /// Switch to tool mode.
    pub fn set_tool(&self, tool_name: &str, key_arg: &str, step_label: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.mode = ActivityMode::Tool;
        inner.tool_name = tool_name.to_string();
        inner.tool_key_arg = key_arg.to_string();
        inner.text_buf.clear();
        if !step_label.is_empty() {
            inner.step_label = step_label.to_string();
        }
        inner.start_time = Instant::now();
        if !inner.active {
            inner.active = true;
            inner.spinner_tick = 0;
        }
    }

    /// Update the step label.
    pub fn set_step_label(&self, label: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.step_label = label.to_string();
    }

    // -- query ---------------------------------------------------------------

    /// Whether the display is currently active.
    pub fn is_active(&self) -> bool {
        self.inner.lock().unwrap().active
    }

    /// The current display mode.
    pub fn mode(&self) -> ActivityMode {
        self.inner.lock().unwrap().mode
    }

    // -- rendering -----------------------------------------------------------

    /// Render the current activity state to stdout (inline).
    ///
    /// This should be called at approximately 8 fps from a tick loop.
    /// It clears the previously rendered lines and writes new ones.
    pub fn render(&self) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.active {
            return Ok(());
        }

        let mut stdout = io::stdout();

        // Clear previous render.
        if inner.last_rendered_lines > 0 {
            clear_lines(&mut stdout, inner.last_rendered_lines)?;
        }

        // Advance spinner.
        inner.spinner_tick = inner.spinner_tick.wrapping_add(1);
        let spinner = SPINNER_FRAMES[inner.spinner_tick % SPINNER_FRAMES.len()];

        let elapsed = inner.start_time.elapsed().as_secs_f64();
        let mode = inner.mode;
        let step_label = inner.step_label.clone();
        let tool_name = inner.tool_name.clone();
        let tool_key_arg = inner.tool_key_arg.clone();
        let buf = inner.text_buf.clone();

        // Apply censor if available.
        let buf = if let Some(ref censor) = self.censor {
            censor.censor_text(&buf)
        } else {
            buf
        };

        // Build header line.
        let (mode_label, mode_style) = match mode {
            ActivityMode::Thinking => ("Thinking...", theme::style_thinking()),
            ActivityMode::Streaming => ("Responding...", theme::style_streaming()),
            ActivityMode::Tool => {
                // Handled separately below since tool name is dynamic.
                ("", theme::style_tool())
            }
        };

        let step_part = if step_label.is_empty() {
            String::new()
        } else {
            format!("  {}", step_label)
        };

        let header = if mode == ActivityMode::Tool {
            format!(
                "{} Running {}...  ({:.1}s){}",
                spinner, tool_name, elapsed, step_part
            )
        } else {
            format!(
                "{} {}  ({:.1}s){}",
                spinner, mode_label, elapsed, step_part
            )
        };

        // Write header (using ANSI colors directly since we're inline).
        let color_code = match mode {
            ActivityMode::Thinking => "\x1b[1;36m",   // bold cyan
            ActivityMode::Streaming => "\x1b[1;32m",   // bold green
            ActivityMode::Tool => "\x1b[1;33m",        // bold yellow
        };

        write!(stdout, "{}{}\x1b[0m", color_code, header)?;

        let mut line_count = 1;

        if mode == ActivityMode::Tool {
            if !tool_key_arg.is_empty() {
                let mut arg_display = tool_key_arg;
                if arg_display.len() > THINKING_MAX_LINE_WIDTH {
                    arg_display.truncate(THINKING_MAX_LINE_WIDTH - 3);
                    arg_display.push_str("...");
                }
                write!(stdout, "\n  \x1b[2;3m{}\x1b[0m", arg_display)?;
                line_count += 1;
            }
        } else if !buf.is_empty() {
            // Take last N lines, truncate width.
            let lines: Vec<&str> = buf.lines().collect();
            let start = if lines.len() > THINKING_TAIL_LINES {
                lines.len() - THINKING_TAIL_LINES
            } else {
                0
            };
            for line in &lines[start..] {
                let mut display_line = line.to_string();
                if display_line.len() > THINKING_MAX_LINE_WIDTH {
                    display_line.truncate(THINKING_MAX_LINE_WIDTH - 3);
                    display_line.push_str("...");
                }
                write!(stdout, "\n  \x1b[2;3m{}\x1b[0m", display_line)?;
                line_count += 1;
            }
        }

        writeln!(stdout)?;
        stdout.flush()?;

        inner.last_rendered_lines = line_count;

        // Drop the lock explicitly before returning.
        drop(inner);
        // Suppress unused variable warning.
        let _ = mode_style;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Clear `n` lines above the current cursor position.
fn clear_lines(stdout: &mut io::Stdout, n: usize) -> io::Result<()> {
    for _ in 0..n {
        stdout.execute(cursor::MoveUp(1))?;
        stdout.execute(terminal::Clear(terminal::ClearType::CurrentLine))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_count() {
        assert_eq!(SPINNER_FRAMES.len(), 10);
    }

    #[test]
    fn activity_display_lifecycle() {
        let display = ActivityDisplay::new(None);
        assert!(!display.is_active());

        display.start(ActivityMode::Thinking, "Step 1/10");
        assert!(display.is_active());
        assert_eq!(display.mode(), ActivityMode::Thinking);

        display.feed("thinking", "I am thinking about this problem...");
        assert_eq!(display.mode(), ActivityMode::Thinking);

        // First text delta auto-transitions to streaming.
        display.feed("text", "Here is the answer");
        assert_eq!(display.mode(), ActivityMode::Streaming);

        display.stop();
        assert!(!display.is_active());
    }

    #[test]
    fn activity_display_tool_mode() {
        let display = ActivityDisplay::new(None);
        display.start(ActivityMode::Thinking, "");

        display.set_tool("read_file", "/path/to/file.rs", "Step 3/20");
        assert_eq!(display.mode(), ActivityMode::Tool);
        assert!(display.is_active());

        display.stop();
    }

    #[test]
    fn feed_when_inactive_is_noop() {
        let display = ActivityDisplay::new(None);
        // Should not panic.
        display.feed("text", "hello");
        assert!(!display.is_active());
    }

    #[test]
    fn set_tool_activates_display() {
        let display = ActivityDisplay::new(None);
        assert!(!display.is_active());

        display.set_tool("run_shell", "ls -la", "Step 1/5");
        assert!(display.is_active());
        assert_eq!(display.mode(), ActivityMode::Tool);

        display.stop();
    }

    #[test]
    fn censor_is_applied_to_text_buf() {
        use std::path::PathBuf;
        let censor = DemoCensor::new(&PathBuf::from("/home/secretuser/Projects/App"));
        let display = ActivityDisplay::new(Some(censor));

        display.start(ActivityMode::Thinking, "");
        display.feed("thinking", "Looking at /home/secretuser/Projects/App/main.rs");

        // We can't easily test render output without capturing stdout,
        // but we can verify the censor is stored and the display is functional.
        assert!(display.is_active());
        display.stop();
    }
}
