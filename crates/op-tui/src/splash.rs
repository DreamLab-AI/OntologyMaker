//! ASCII art splash screen with potted-plant flanking.
//!
//! Matches the Python `_build_splash()` in `tui.py`.  The banner text is
//! embedded as a static ASCII-art fallback (no `pyfiglet` equivalent in Rust)
//! using the "Standard" figlet font output for "OpenPlanter".

/// Left-side potted plant art (bottom-aligned).
const PLANT_LEFT: &[&str] = &[
    " .oOo.  ",
    "oO.|.Oo ",
    "Oo.|.oO ",
    "  .|.   ",
    "[=====] ",
    " \\___/  ",
];

/// Right-side potted plant art (bottom-aligned).
const PLANT_RIGHT: &[&str] = &[
    "  .oOo. ",
    " oO.|.Oo",
    " Oo.|.oO",
    "   .|.  ",
    " [=====]",
    "  \\___/ ",
];

/// Pre-rendered "Standard" figlet font for "OpenPlanter".
///
/// Generated with `pyfiglet.figlet_format("OpenPlanter", font="standard")`.
const FIGLET_BANNER: &str = "\
  ___                   ____  _             _
 / _ \\ _ __   ___ _ __ |  _ \\| | __ _ _ __ | |_ ___ _ __
| | | | '_ \\ / _ \\ '_ \\| |_) | |/ _` | '_ \\| __/ _ \\ '__|
| |_| | |_) |  __/ | | |  __/| | (_| | | | | ||  __/ |
 \\___/| .__/ \\___|_| |_|_|   |_|\\__,_|_| |_|\\__\\___|_|
      |_|";

/// Build the splash screen string: `[plant_left]  [banner]  [plant_right]`.
///
/// Plants are bottom-aligned relative to the banner text.
pub fn build_splash() -> String {
    let lines: Vec<&str> = FIGLET_BANNER.lines().collect();

    // Strip common leading whitespace.
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let stripped: Vec<String> = lines.iter().map(|l| {
        if l.len() > min_indent {
            l[min_indent..].to_string()
        } else {
            String::new()
        }
    }).collect();

    let max_w = stripped.iter().map(|l| l.len()).max().unwrap_or(0);
    let padded: Vec<String> = stripped.iter().map(|l| format!("{:<width$}", l, width = max_w)).collect();

    let n = padded.len();

    // Plant widths.
    let pw_l = PLANT_LEFT.iter().map(|l| l.len()).max().unwrap_or(0);
    let pw_r = PLANT_RIGHT.iter().map(|l| l.len()).max().unwrap_or(0);

    // Bottom-align plants to the banner height.
    let left: Vec<String> = if n > PLANT_LEFT.len() {
        let mut v: Vec<String> = (0..(n - PLANT_LEFT.len()))
            .map(|_| " ".repeat(pw_l))
            .collect();
        v.extend(PLANT_LEFT.iter().map(|s| s.to_string()));
        v
    } else {
        PLANT_LEFT[PLANT_LEFT.len() - n..]
            .iter()
            .map(|s| s.to_string())
            .collect()
    };

    let right: Vec<String> = if n > PLANT_RIGHT.len() {
        let mut v: Vec<String> = (0..(n - PLANT_RIGHT.len()))
            .map(|_| " ".repeat(pw_r))
            .collect();
        v.extend(PLANT_RIGHT.iter().map(|s| s.to_string()));
        v
    } else {
        PLANT_RIGHT[PLANT_RIGHT.len() - n..]
            .iter()
            .map(|s| s.to_string())
            .collect()
    };

    (0..n)
        .map(|i| format!("{}  {}  {}", left[i], padded[i], right[i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lazily-computed splash art string.
///
/// Call [`splash_art()`] to obtain the string; the first invocation builds it.
pub fn splash_art() -> &'static str {
    use std::sync::OnceLock;
    static ART: OnceLock<String> = OnceLock::new();
    ART.get_or_init(build_splash)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_art_is_non_empty() {
        let art = splash_art();
        assert!(!art.is_empty());
    }

    #[test]
    fn splash_art_contains_figlet_characters() {
        let art = splash_art();
        // The figlet Standard font renders "OpenPlanter" as ASCII art.
        // Check for distinctive figlet character sequences.
        assert!(art.contains("___"), "splash art should contain figlet underscores");
        assert!(art.contains("|_|"), "splash art should contain figlet pipe patterns");
        assert!(art.contains("__/"), "splash art should contain figlet slash patterns");
    }

    #[test]
    fn splash_art_contains_plant_decorations() {
        let art = splash_art();
        assert!(art.contains("[=====]"), "splash art should contain plant pot");
        assert!(art.contains("\\___/"), "splash art should contain plant base");
    }

    #[test]
    fn splash_art_lines_are_consistent_width() {
        let art = build_splash();
        let lines: Vec<&str> = art.lines().collect();
        assert!(!lines.is_empty());
        // All lines should be approximately the same width (within plant padding).
        let lengths: Vec<usize> = lines.iter().map(|l| l.len()).collect();
        let max = *lengths.iter().max().unwrap();
        let min = *lengths.iter().min().unwrap();
        // Allow some variance due to trailing spaces, but should be close.
        assert!(
            max - min <= 10,
            "line lengths vary too much: min={}, max={}",
            min,
            max
        );
    }

    #[test]
    fn build_splash_has_correct_line_count() {
        let art = build_splash();
        let n_banner = FIGLET_BANNER.lines().count();
        let n_art = art.lines().count();
        assert_eq!(
            n_art, n_banner,
            "splash should have same number of lines as banner"
        );
    }
}
