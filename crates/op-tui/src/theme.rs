//! Color constants and style definitions for the TUI layer.
//!
//! Mirrors the Rich color palette used in the Python TUI (tui.py).
//! All styles are built on top of ratatui's [`Style`] / [`Color`] primitives.

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Named colours (matching Rich's named palette)
// ---------------------------------------------------------------------------

/// Cyan accent used for "thinking" indicators and step rules.
pub const CYAN: Color = Color::Cyan;

/// Green accent used for "responding / streaming" indicators.
pub const GREEN: Color = Color::Green;

/// Yellow accent used for tool-execution indicators.
pub const YELLOW: Color = Color::Yellow;

/// Red for errors.
pub const RED: Color = Color::Red;

/// Dimmed foreground for secondary information.
pub const DIM_FG: Color = Color::DarkGray;

/// Default foreground (terminal default).
pub const DEFAULT_FG: Color = Color::Reset;

/// Default background (terminal default).
pub const DEFAULT_BG: Color = Color::Reset;

/// Magenta for prompt.
pub const MAGENTA: Color = Color::Magenta;

/// White for emphasis.
pub const WHITE: Color = Color::White;

// ---------------------------------------------------------------------------
// Pre-built styles
// ---------------------------------------------------------------------------

/// Bold cyan: thinking header, splash art.
pub fn style_thinking() -> Style {
    Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
}

/// Bold green: streaming / responding header.
pub fn style_streaming() -> Style {
    Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
}

/// Bold yellow: tool execution header.
pub fn style_tool() -> Style {
    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
}

/// Bold red: error messages.
pub fn style_error() -> Style {
    Style::default().fg(RED).add_modifier(Modifier::BOLD)
}

/// Dim italic: secondary text, tool arguments, thinking snippets.
pub fn style_dim() -> Style {
    Style::default().fg(DIM_FG)
}

/// Dim italic: secondary text in italic contexts.
pub fn style_dim_italic() -> Style {
    Style::default()
        .fg(DIM_FG)
        .add_modifier(Modifier::ITALIC)
}

/// Bold: emphasis for step headers.
pub fn style_bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Cyan text (non-bold): slash-command output.
pub fn style_info() -> Style {
    Style::default().fg(CYAN)
}

/// Prompt style: bold magenta.
pub fn style_prompt() -> Style {
    Style::default()
        .fg(MAGENTA)
        .add_modifier(Modifier::BOLD)
}

/// Normal / default style.
pub fn style_normal() -> Style {
    Style::default()
}

/// Markdown heading level 1: bold + underlined.
pub fn style_heading1() -> Style {
    Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED)
}

/// Markdown heading level 2: bold.
pub fn style_heading2() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Markdown code/pre style: dim background feel (using a distinct fg).
pub fn style_code() -> Style {
    Style::default().fg(Color::LightYellow)
}

/// Markdown bold.
pub fn style_md_bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// Markdown italic.
pub fn style_md_italic() -> Style {
    Style::default().add_modifier(Modifier::ITALIC)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_distinct() {
        // Ensure the key styles differ from one another.
        let thinking = style_thinking();
        let streaming = style_streaming();
        let tool = style_tool();
        let error = style_error();

        assert_ne!(thinking.fg, streaming.fg);
        assert_ne!(streaming.fg, tool.fg);
        assert_ne!(tool.fg, error.fg);
    }

    #[test]
    fn style_dim_has_dimmed_fg() {
        let s = style_dim();
        assert_eq!(s.fg, Some(DIM_FG));
    }

    #[test]
    fn style_bold_has_bold_modifier() {
        let s = style_bold();
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn style_prompt_is_bold_magenta() {
        let s = style_prompt();
        assert_eq!(s.fg, Some(MAGENTA));
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }
}
