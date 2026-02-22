//! Demo mode: censor workspace path segments in TUI output.
//!
//! Censoring is UI-only -- the agent's internal state is unaffected.  Block
//! characters (`\u{2588}`) replace sensitive text at the same length so
//! display alignment is preserved.
//!
//! Entity-name censoring is handled by a prompt instruction (see prompts.rs)
//! rather than regex post-processing.
//!
//! Port of Python `agent/demo.py`.

use std::path::Path;

// ---------------------------------------------------------------------------
// Generic path parts exempt from censoring
// ---------------------------------------------------------------------------

/// Path components that should NOT be censored because they are generic /
/// well-known directory names.
const GENERIC_PATH_PARTS: &[&str] = &[
    "/",
    "Users",
    "home",
    "Documents",
    "Desktop",
    "Downloads",
    "Projects",
    "repos",
    "src",
    "var",
    "tmp",
    "opt",
    "etc",
    "Library",
    "Applications",
    "volumes",
    "mnt",
    "media",
    "nix",
    "store",
    "run",
    "snap",
];

/// Check whether a path component is in the generic exemption set.
fn is_generic(part: &str) -> bool {
    GENERIC_PATH_PARTS.contains(&part)
}

// ---------------------------------------------------------------------------
// DemoCensor
// ---------------------------------------------------------------------------

/// Builds replacement tables from a workspace path and censors text.
///
/// Matches Python's `DemoCensor` class in `agent/demo.py`.
#[derive(Debug, Clone)]
pub struct DemoCensor {
    /// `(original, replacement)` pairs sorted longest-first.
    replacements: Vec<(String, String)>,
}

impl DemoCensor {
    /// Construct a new censor from the workspace path.
    ///
    /// Non-generic, non-project-name path segments are added to the
    /// replacement table.  The project name (final component of `workspace`)
    /// is intentionally left visible.
    pub fn new(workspace: &Path) -> Self {
        let mut replacements = Vec::new();
        let project_name = workspace
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        for component in workspace.components() {
            let part = component.as_os_str().to_string_lossy();
            let part = part.as_ref();

            if part.is_empty() {
                continue;
            }
            if is_generic(part) {
                continue;
            }
            if part == project_name {
                continue;
            }

            let replacement = "\u{2588}".repeat(part.len());
            replacements.push((part.to_string(), replacement));
        }

        // Sort longest-first so longer matches take precedence.
        replacements.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        Self { replacements }
    }

    /// Apply workspace-path segment replacements to `text`.
    pub fn censor_text(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (original, replacement) in &self.replacements {
            result = result.replace(original.as_str(), replacement.as_str());
        }
        result
    }

    /// Return whether this censor has any active replacements.
    pub fn is_active(&self) -> bool {
        !self.replacements.is_empty()
    }

    /// Return the number of replacement rules.
    pub fn replacement_count(&self) -> usize {
        self.replacements.len()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_censor_basic_path() {
        let workspace = PathBuf::from("/home/alice/Projects/MyApp");
        let censor = DemoCensor::new(&workspace);

        // "alice" is non-generic, non-project-name, so it should be censored.
        let input = "Reading /home/alice/Projects/MyApp/src/main.rs";
        let output = censor.censor_text(input);

        assert!(!output.contains("alice"), "alice should be censored: {}", output);
        // "home", "Projects", "MyApp" (project name), "src" are exempt.
        assert!(output.contains("home"), "home should be preserved: {}", output);
        assert!(output.contains("Projects"), "Projects should be preserved: {}", output);
        assert!(output.contains("MyApp"), "MyApp (project name) should be preserved: {}", output);
        assert!(output.contains("src"), "src should be preserved: {}", output);
    }

    #[test]
    fn test_censor_replaces_with_block_chars() {
        let workspace = PathBuf::from("/home/bob/Projects/Demo");
        let censor = DemoCensor::new(&workspace);

        let input = "user bob logged in";
        let output = censor.censor_text(input);

        // "bob" is 3 chars, replaced with 3 block chars.
        assert!(output.contains("\u{2588}\u{2588}\u{2588}"));
        assert!(!output.contains("bob"));
    }

    #[test]
    fn test_censor_preserves_length() {
        let workspace = PathBuf::from("/home/username123/Projects/Test");
        let censor = DemoCensor::new(&workspace);

        let original = "username123";
        let censored = censor.censor_text(original);

        // Censored text should be the same character count.
        assert_eq!(
            censored.chars().count(),
            original.len(),
            "censored length should match original"
        );
    }

    #[test]
    fn test_censor_empty_workspace() {
        let workspace = PathBuf::from("/");
        let censor = DemoCensor::new(&workspace);

        // "/" is generic, so no replacements.
        assert!(!censor.is_active());
        assert_eq!(censor.censor_text("hello world"), "hello world");
    }

    #[test]
    fn test_censor_generic_parts_not_replaced() {
        let workspace = PathBuf::from("/home/Documents/src/MyProject");
        let censor = DemoCensor::new(&workspace);

        let input = "Found in /home/Documents/src/MyProject/file.txt";
        let output = censor.censor_text(input);

        // All path components are either generic or the project name.
        assert!(output.contains("home"));
        assert!(output.contains("Documents"));
        assert!(output.contains("src"));
        assert!(output.contains("MyProject"));
    }

    #[test]
    fn test_censor_longest_match_first() {
        // If workspace has overlapping substrings, longest should match first.
        let workspace = PathBuf::from("/home/ab/abc/Projects/Proj");
        let censor = DemoCensor::new(&workspace);

        // "abc" should be censored as 3 blocks, "ab" as 2 blocks.
        let input = "abc and ab";
        let output = censor.censor_text(input);

        // "abc" replaced first (longest), then "ab".
        // After censoring "abc" -> "?????????", any remaining "ab" in the text
        // should also be censored.
        assert!(!output.contains("abc"), "abc should be censored: {}", output);
        assert!(!output.contains("ab"), "ab should be censored: {}", output);
    }

    #[test]
    fn test_censor_no_op_when_text_has_no_matches() {
        let workspace = PathBuf::from("/home/secretuser/Projects/App");
        let censor = DemoCensor::new(&workspace);

        let input = "This text has nothing to censor";
        let output = censor.censor_text(input);
        assert_eq!(output, input);
    }

    #[test]
    fn test_replacement_count() {
        let workspace = PathBuf::from("/home/alice/bob/Projects/MyApp");
        let censor = DemoCensor::new(&workspace);

        // "alice" and "bob" should be in replacements.
        assert_eq!(censor.replacement_count(), 2);
    }
}
