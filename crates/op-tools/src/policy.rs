//! Runtime policy enforcement for shell commands and parallel write conflict detection.
//!
//! Mirrors the Python `_check_shell_policy`, `_register_write_target`,
//! `begin_parallel_write_group`, and `end_parallel_write_group` logic.

use op_core::OpError;
use parking_lot::Mutex;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Shell policy regexes (compiled once)
// ---------------------------------------------------------------------------

static HEREDOC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<<-?\s*['"]?\w+['"]?"#).expect("HEREDOC_RE"));

static INTERACTIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[;&|]\s*)(vim|nano|less|more|top|htop|man)\b").expect("INTERACTIVE_RE")
});

/// Check a shell command against runtime policy.
/// Returns `Some(message)` if the command is blocked, `None` if it is allowed.
pub fn check_shell_policy(command: &str) -> Option<String> {
    if HEREDOC_RE.is_match(command) {
        return Some(
            "BLOCKED: Heredoc syntax (<< EOF) is not allowed by runtime policy. \
             Use write_file/apply_patch for multi-line content."
                .to_string(),
        );
    }
    if INTERACTIVE_RE.is_match(command) {
        return Some(
            "BLOCKED: Interactive terminal programs are not allowed by runtime policy \
             (vim/nano/less/more/top/htop/man)."
                .to_string(),
        );
    }
    None
}

// ---------------------------------------------------------------------------
// Parallel write conflict detection
// ---------------------------------------------------------------------------

/// Tracks per-group file write claims so that parallel sibling tasks don't
/// accidentally clobber each other's files.
#[derive(Debug, Default)]
pub struct WriteConflictTracker {
    /// group_id -> (resolved_path -> owner_id)
    claims: Mutex<HashMap<String, HashMap<PathBuf, String>>>,
}

impl WriteConflictTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new parallel write group.
    pub fn begin_group(&self, group_id: &str) {
        let mut claims = self.claims.lock();
        claims.insert(group_id.to_string(), HashMap::new());
    }

    /// End a parallel write group and release its claims.
    pub fn end_group(&self, group_id: &str) {
        let mut claims = self.claims.lock();
        claims.remove(group_id);
    }

    /// Register that `owner_id` intends to write `resolved` within `group_id`.
    /// Returns `Err` if another owner already claimed the same path.
    pub fn register_write(
        &self,
        group_id: &str,
        owner_id: &str,
        resolved: &Path,
        workspace_root: &Path,
    ) -> Result<(), OpError> {
        let mut all_claims = self.claims.lock();
        let group = match all_claims.get_mut(group_id) {
            Some(g) => g,
            None => {
                // No active group — nothing to track.
                return Ok(());
            }
        };
        match group.get(resolved) {
            None => {
                group.insert(resolved.to_path_buf(), owner_id.to_string());
                Ok(())
            }
            Some(existing_owner) if existing_owner == owner_id => Ok(()),
            Some(existing_owner) => {
                let rel = resolved
                    .strip_prefix(workspace_root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| resolved.display().to_string());
                Err(OpError::tool(format!(
                    "Parallel write conflict: '{}' is already claimed by sibling task {}.",
                    rel, existing_owner
                )))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Execution scope — per-task group/owner context
// ---------------------------------------------------------------------------

/// Holds the group_id and owner_id for the current execution scope.
/// Used by workspace methods that need to register write targets.
#[derive(Debug, Clone, Default)]
pub struct ExecutionScope {
    pub group_id: Option<String>,
    pub owner_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Shell policy tests ---

    #[test]
    fn test_heredoc_blocked() {
        let msg = check_shell_policy("cat << EOF\nhello\nEOF");
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("BLOCKED"));
    }

    #[test]
    fn test_heredoc_quoted() {
        let msg = check_shell_policy("cat <<'END'\nhello\nEND");
        assert!(msg.is_some());
    }

    #[test]
    fn test_heredoc_dash() {
        let msg = check_shell_policy("cat <<- MARKER");
        assert!(msg.is_some());
    }

    #[test]
    fn test_interactive_vim_blocked() {
        let msg = check_shell_policy("vim file.txt");
        assert!(msg.is_some());
        assert!(msg.unwrap().contains("BLOCKED"));
    }

    #[test]
    fn test_interactive_nano_after_semicolon() {
        let msg = check_shell_policy("echo hello; nano file.txt");
        assert!(msg.is_some());
    }

    #[test]
    fn test_interactive_htop_after_pipe() {
        let msg = check_shell_policy("echo x | htop");
        assert!(msg.is_some());
    }

    #[test]
    fn test_allowed_command() {
        assert!(check_shell_policy("ls -la").is_none());
    }

    #[test]
    fn test_allowed_echo() {
        assert!(check_shell_policy("echo hello world").is_none());
    }

    #[test]
    fn test_allowed_grep() {
        // 'man' in 'manifest' should NOT trigger the interactive check
        // because the regex requires a word boundary.
        assert!(check_shell_policy("grep manifest Cargo.toml").is_none());
    }

    // --- Write conflict tracker tests ---

    #[test]
    fn test_no_conflict_single_owner() {
        let tracker = WriteConflictTracker::new();
        tracker.begin_group("g1");
        let root = Path::new("/workspace");
        let p = PathBuf::from("/workspace/foo.rs");
        assert!(tracker.register_write("g1", "task_a", &p, root).is_ok());
        // Same owner, same file → ok
        assert!(tracker.register_write("g1", "task_a", &p, root).is_ok());
    }

    #[test]
    fn test_conflict_different_owner() {
        let tracker = WriteConflictTracker::new();
        tracker.begin_group("g1");
        let root = Path::new("/workspace");
        let p = PathBuf::from("/workspace/foo.rs");
        assert!(tracker.register_write("g1", "task_a", &p, root).is_ok());
        let result = tracker.register_write("g1", "task_b", &p, root);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Parallel write conflict"));
        assert!(msg.contains("task_a"));
    }

    #[test]
    fn test_end_group_clears_claims() {
        let tracker = WriteConflictTracker::new();
        tracker.begin_group("g1");
        let root = Path::new("/workspace");
        let p = PathBuf::from("/workspace/foo.rs");
        assert!(tracker.register_write("g1", "task_a", &p, root).is_ok());
        tracker.end_group("g1");
        // After ending, a new group can claim the same file
        tracker.begin_group("g2");
        assert!(tracker.register_write("g2", "task_b", &p, root).is_ok());
    }

    #[test]
    fn test_no_active_group_is_ok() {
        let tracker = WriteConflictTracker::new();
        let root = Path::new("/workspace");
        let p = PathBuf::from("/workspace/foo.rs");
        // No group started — register_write should be a no-op
        assert!(tracker
            .register_write("nonexistent", "task_a", &p, root)
            .is_ok());
    }

    #[test]
    fn test_different_files_same_group_different_owners() {
        let tracker = WriteConflictTracker::new();
        tracker.begin_group("g1");
        let root = Path::new("/workspace");
        let p1 = PathBuf::from("/workspace/a.rs");
        let p2 = PathBuf::from("/workspace/b.rs");
        assert!(tracker.register_write("g1", "task_a", &p1, root).is_ok());
        assert!(tracker.register_write("g1", "task_b", &p2, root).is_ok());
    }
}
