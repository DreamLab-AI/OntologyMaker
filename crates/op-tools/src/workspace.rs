//! WorkspaceTools — aggregate root that wires together all tool subsystems.
//!
//! Mirrors the Python `WorkspaceTools` dataclass, providing a single struct
//! that owns the workspace root path, configuration limits, background job
//! manager, file-read tracking, write-conflict tracker, and Exa API client.

use crate::file_ops;
use crate::patch::{self, HashlineOp, PatchOp};
use crate::policy::{ExecutionScope, WriteConflictTracker};
use crate::search;
use crate::shell::{self, BgJobManager};
use crate::web::{self, ExaClient};
use op_core::{ImageData, OpError, OpResult};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Aggregate root for all workspace tool operations.
///
/// All public methods match the Python `WorkspaceTools` API.
pub struct WorkspaceTools {
    // Configuration
    pub root: PathBuf,
    pub shell: String,
    pub command_timeout_sec: u64,
    pub max_shell_output_chars: usize,
    pub max_file_chars: usize,
    pub max_files_listed: usize,
    pub max_search_hits: usize,

    // Subsystems
    pub exa_client: ExaClient,
    pub bg_jobs: BgJobManager,

    // Runtime policy state
    pub files_read: Mutex<HashSet<PathBuf>>,
    pub write_tracker: WriteConflictTracker,
    pub scope: Mutex<ExecutionScope>,
}

impl WorkspaceTools {
    /// Create a new `WorkspaceTools` from a root path with all defaults.
    pub fn new(root: &Path) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Self {
            root,
            shell: "/bin/sh".to_string(),
            command_timeout_sec: 45,
            max_shell_output_chars: 16000,
            max_file_chars: 20000,
            max_files_listed: 400,
            max_search_hits: 200,
            exa_client: ExaClient::new(None, "https://api.exa.ai", 45),
            bg_jobs: BgJobManager::new(),
            files_read: Mutex::new(HashSet::new()),
            write_tracker: WriteConflictTracker::new(),
            scope: Mutex::new(ExecutionScope::default()),
        }
    }

    /// Create a new `WorkspaceTools` instance with full configuration, validating the root.
    pub fn with_config(
        root: PathBuf,
        shell: Option<String>,
        command_timeout_sec: Option<u64>,
        max_shell_output_chars: Option<usize>,
        max_file_chars: Option<usize>,
        max_files_listed: Option<usize>,
        max_search_hits: Option<usize>,
        exa_api_key: Option<String>,
        exa_base_url: Option<String>,
    ) -> OpResult<Self> {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        if !root.exists() {
            return Err(OpError::tool(format!(
                "Workspace does not exist: {}",
                root.display()
            )));
        }
        if !root.is_dir() {
            return Err(OpError::tool(format!(
                "Workspace is not a directory: {}",
                root.display()
            )));
        }

        let timeout = command_timeout_sec.unwrap_or(45);
        let base_url = exa_base_url.unwrap_or_else(|| "https://api.exa.ai".to_string());

        Ok(Self {
            root,
            shell: shell.unwrap_or_else(|| "/bin/sh".to_string()),
            command_timeout_sec: timeout,
            max_shell_output_chars: max_shell_output_chars.unwrap_or(16000),
            max_file_chars: max_file_chars.unwrap_or(20000),
            max_files_listed: max_files_listed.unwrap_or(400),
            max_search_hits: max_search_hits.unwrap_or(200),
            exa_client: ExaClient::new(exa_api_key, &base_url, timeout),
            bg_jobs: BgJobManager::new(),
            files_read: Mutex::new(HashSet::new()),
            write_tracker: WriteConflictTracker::new(),
            scope: Mutex::new(ExecutionScope::default()),
        })
    }

    /// Set the EXA API key for web operations.
    pub fn set_exa_api_key(&mut self, key: Option<String>) {
        self.exa_client =
            ExaClient::new(key, &self.exa_client.base_url, self.exa_client.timeout_sec);
    }

    /// Validate and resolve a path, ensuring it doesn't escape the workspace.
    pub fn resolve_path(&self, raw_path: &str) -> OpResult<PathBuf> {
        file_ops::resolve_path(raw_path, &self.root)
    }

    // --- Parallel write group management ---

    /// Start a new parallel write group.
    pub fn begin_parallel_write_group(&self, group_id: &str) {
        self.write_tracker.begin_group(group_id);
    }

    /// End a parallel write group and release its claims.
    pub fn end_parallel_write_group(&self, group_id: &str) {
        self.write_tracker.end_group(group_id);
    }

    /// Set the execution scope (group_id/owner_id) for the current task.
    pub fn set_execution_scope(&self, group_id: Option<String>, owner_id: Option<String>) {
        let mut scope = self.scope.lock();
        scope.group_id = group_id;
        scope.owner_id = owner_id;
    }

    /// Register a write target with the conflict tracker, using the current scope.
    fn register_write_target(&self, resolved: &Path) -> Result<(), OpError> {
        let scope = self.scope.lock();
        match (&scope.group_id, &scope.owner_id) {
            (Some(gid), Some(oid)) => {
                self.write_tracker
                    .register_write(gid, oid, resolved, &self.root)
            }
            _ => Ok(()),
        }
    }

    // --- File operations ---

    /// List files in the workspace.
    pub fn list_files(&self, glob: Option<&str>) -> String {
        file_ops::list_files(
            &self.root,
            glob,
            self.max_files_listed,
            self.command_timeout_sec,
        )
    }

    /// Read a file with optional hashline formatting.
    pub fn read_file(&self, path: &str, hashline: bool) -> String {
        file_ops::read_file(
            path,
            hashline,
            &self.root,
            self.max_file_chars,
            &self.files_read,
        )
    }

    /// Read an image file and return text description + optional image data.
    pub fn read_image(&self, path: &str) -> (String, Option<ImageData>) {
        file_ops::read_image(path, &self.root)
    }

    /// Write a file, enforcing read-before-overwrite and parallel write policies.
    pub fn write_file(&self, path: &str, content: &str) -> String {
        let resolved = match file_ops::resolve_path(path, &self.root) {
            Ok(p) => p,
            Err(e) => return e.to_string(),
        };
        // Check parallel write conflict
        if let Err(e) = self.register_write_target(&resolved) {
            return format!("Blocked by policy: {}", e);
        }
        file_ops::write_file(path, content, &self.root, &self.files_read)
    }

    /// Edit a file by replacing a text span (with fuzzy fallback).
    pub fn edit_file(&self, path: &str, old_text: &str, new_text: &str) -> String {
        let resolved = match file_ops::resolve_path(path, &self.root) {
            Ok(p) => p,
            Err(e) => return e.to_string(),
        };
        // Check parallel write conflict
        if let Err(e) = self.register_write_target(&resolved) {
            return format!("Blocked by policy: {}", e);
        }
        file_ops::edit_file(path, old_text, new_text, &self.root, &self.files_read)
    }

    /// Edit a file using hash-anchored line references from JSON values.
    pub fn hashline_edit_json(&self, path: &str, edits: &[serde_json::Value]) -> String {
        let mut ops = Vec::new();
        for edit in edits {
            match HashlineOp::from_json(edit) {
                Ok(op) => ops.push(op),
                Err(e) => return e.to_string(),
            }
        }
        self.hashline_edit(path, &ops)
    }

    /// Edit a file using hash-anchored line references.
    pub fn hashline_edit(&self, path: &str, edits: &[HashlineOp]) -> String {
        let resolved = match file_ops::resolve_path(path, &self.root) {
            Ok(p) => p,
            Err(e) => return e.to_string(),
        };
        if !resolved.exists() {
            return format!("File not found: {}", path);
        }
        if resolved.is_dir() {
            return format!("Path is a directory, not a file: {}", path);
        }
        let content = match std::fs::read_to_string(&resolved) {
            Ok(c) => c,
            Err(e) => return format!("Failed to read file {}: {}", path, e),
        };
        self.files_read.lock().insert(resolved.clone());

        let (new_content, changed) = match patch::apply_hashline_edits(&content, edits) {
            Ok(result) => result,
            Err(e) => return e,
        };

        if changed == 0 {
            return format!("No changes needed in {}", path);
        }

        // Check parallel write conflict
        if let Err(e) = self.register_write_target(&resolved) {
            return format!("Blocked by policy: {}", e);
        }

        if let Err(e) = std::fs::write(&resolved, &new_content) {
            return format!("Failed to write {}: {}", path, e);
        }
        self.files_read.lock().insert(resolved.clone());
        let rel = resolved
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string());
        format!("Edited {} ({} edit(s) applied)", rel, changed)
    }

    // --- Patch operations ---

    /// Apply a Codex-style patch.
    pub fn apply_patch(&self, patch_text: &str) -> String {
        if patch_text.trim().is_empty() {
            return "apply_patch requires non-empty patch text".to_string();
        }

        // Pre-validate: check all write targets for parallel write conflicts
        match patch::parse_agent_patch(patch_text) {
            Ok(ops) => {
                for op in &ops {
                    let paths_to_check = match op {
                        PatchOp::Add { path, .. } => vec![path.clone()],
                        PatchOp::Delete { path } => vec![path.clone()],
                        PatchOp::Update {
                            path, move_to, ..
                        } => {
                            let mut v = vec![path.clone()];
                            if let Some(mt) = move_to {
                                v.push(mt.clone());
                            }
                            v
                        }
                    };
                    for p in paths_to_check {
                        let resolved = match file_ops::resolve_path(&p, &self.root) {
                            Ok(r) => r,
                            Err(e) => return format!("Blocked by policy: {}", e),
                        };
                        if let Err(e) = self.register_write_target(&resolved) {
                            return format!("Blocked by policy: {}", e);
                        }
                    }
                }
            }
            Err(e) => return format!("Patch failed: {}", e),
        }

        let root = self.root.clone();
        match patch::apply_agent_patch(patch_text, |p| file_ops::resolve_path(p, &root)) {
            Ok(report) => {
                // Track files that were added/updated as read
                for rel_path in report.added.iter().chain(report.updated.iter()) {
                    if let Ok(resolved) = file_ops::resolve_path(rel_path, &self.root) {
                        self.files_read.lock().insert(resolved);
                    }
                }
                report.render()
            }
            Err(e) => format!("Patch failed: {}", e),
        }
    }

    // --- Search operations ---

    /// Search file contents for a query.
    pub fn search_files(&self, query: &str, glob: Option<&str>) -> String {
        search::search_files(
            query,
            glob,
            &self.root,
            self.max_search_hits,
            self.command_timeout_sec,
        )
    }

    /// Build a lightweight repo map.
    pub fn repo_map(&self, glob: Option<&str>, max_files: Option<usize>) -> String {
        search::repo_map(
            &self.root,
            glob,
            max_files.unwrap_or(200),
            self.max_file_chars,
            self.command_timeout_sec,
        )
    }

    // --- Shell operations ---

    /// Run a shell command and return its output.
    pub async fn run_shell(&self, command: &str, timeout: Option<u64>) -> String {
        shell::run_shell(
            command,
            &self.shell,
            &self.root,
            timeout,
            self.command_timeout_sec,
            self.max_shell_output_chars,
        )
        .await
    }

    /// Start a shell command in the background.
    pub async fn run_shell_bg(&self, command: &str) -> String {
        shell::run_shell_bg(command, &self.shell, &self.root, &self.bg_jobs).await
    }

    /// Check the status of a background job.
    pub async fn check_shell_bg(&self, job_id: u64) -> String {
        shell::check_shell_bg(job_id, &self.bg_jobs, self.max_shell_output_chars).await
    }

    /// Kill a background job.
    pub async fn kill_shell_bg(&self, job_id: u64) -> String {
        shell::kill_shell_bg(job_id, &self.bg_jobs).await
    }

    /// Clean up all background jobs.
    pub async fn cleanup_bg_jobs(&self) {
        shell::cleanup_bg_jobs(&self.bg_jobs).await
    }

    // --- Web operations ---

    /// Perform a web search using the Exa API.
    pub async fn web_search(
        &self,
        query: &str,
        num_results: Option<u32>,
        include_text: bool,
    ) -> String {
        web::web_search(
            &self.exa_client,
            query,
            num_results,
            include_text,
            self.max_file_chars,
        )
        .await
    }

    /// Fetch URL contents using the Exa API.
    pub async fn fetch_url(&self, urls: &[String]) -> String {
        web::fetch_url(&self.exa_client, urls, self.max_file_chars).await
    }
}

impl std::fmt::Debug for WorkspaceTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceTools")
            .field("root", &self.root)
            .field("shell", &self.shell)
            .field("command_timeout_sec", &self.command_timeout_sec)
            .field("max_shell_output_chars", &self.max_shell_output_chars)
            .field("max_file_chars", &self.max_file_chars)
            .field("max_files_listed", &self.max_files_listed)
            .field("max_search_hits", &self.max_search_hits)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn make_workspace_named(name: &str) -> (PathBuf, WorkspaceTools) {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("op_ws_{}_{}", name, id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ws = WorkspaceTools::new(&dir);
        (fs::canonicalize(&dir).unwrap(), ws)
    }

    #[test]
    fn test_workspace_new() {
        let (dir, ws) = make_workspace_named("new");
        assert_eq!(ws.root, dir);
        assert_eq!(ws.shell, "/bin/sh");
        assert_eq!(ws.command_timeout_sec, 45);
        assert_eq!(ws.max_file_chars, 20000);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_workspace_with_config_nonexistent() {
        let result = WorkspaceTools::with_config(
            PathBuf::from("/nonexistent/workspace/path"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_path() {
        let (dir, ws) = make_workspace_named("resolve");
        let result = ws.resolve_path("file.txt");
        assert!(result.is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_list_files() {
        let (dir, ws) = make_workspace_named("list");
        fs::write(dir.join("foo.txt"), "hello").unwrap();
        let result = ws.list_files(None);
        assert!(result.contains("foo.txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_file() {
        let (dir, ws) = make_workspace_named("read");
        fs::write(dir.join("test.txt"), "hello world\n").unwrap();
        let result = ws.read_file("test.txt", true);
        assert!(result.contains("# test.txt"));
        assert!(result.contains("hello world"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_file_new() {
        let (dir, ws) = make_workspace_named("write_new");
        let result = ws.write_file("new.txt", "new content");
        assert!(result.contains("Wrote"));
        assert_eq!(
            fs::read_to_string(dir.join("new.txt")).unwrap(),
            "new content"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_file_blocked() {
        let (dir, ws) = make_workspace_named("write_blocked");
        fs::write(dir.join("existing.txt"), "old").unwrap();
        let result = ws.write_file("existing.txt", "new");
        assert!(result.contains("BLOCKED"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edit_file() {
        let (dir, ws) = make_workspace_named("edit");
        fs::write(dir.join("edit_me.txt"), "hello old world\n").unwrap();
        let _ = ws.read_file("edit_me.txt", false);
        let result = ws.edit_file("edit_me.txt", "old", "new");
        assert!(result.contains("Edited"));
        let content = fs::read_to_string(dir.join("edit_me.txt")).unwrap();
        assert!(content.contains("new"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_patch_add() {
        let (dir, ws) = make_workspace_named("patch_add");
        let patch =
            "*** Begin Patch\n*** Add File: patched.txt\n+Line 1\n+Line 2\n*** End Patch";
        let result = ws.apply_patch(patch);
        assert!(result.contains("Patch applied"));
        assert!(result.contains("patched.txt"));
        let content = fs::read_to_string(dir.join("patched.txt")).unwrap();
        assert!(content.contains("Line 1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_files() {
        let (dir, ws) = make_workspace_named("search");
        fs::write(dir.join("search_me.txt"), "unique_token_12345\n").unwrap();
        let result = ws.search_files("unique_token_12345", None);
        assert!(result.contains("unique_token_12345"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_run_shell() {
        let (dir, ws) = make_workspace_named("shell");
        let result = ws.run_shell("echo hello", None).await;
        assert!(result.contains("hello"));
        assert!(result.contains("exit_code=0"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parallel_write_conflict() {
        let (dir, ws) = make_workspace_named("conflict");

        ws.begin_parallel_write_group("g1");
        ws.set_execution_scope(Some("g1".to_string()), Some("task_a".to_string()));

        let _ = ws.write_file("new1.txt", "from task_a");

        ws.set_execution_scope(Some("g1".to_string()), Some("task_b".to_string()));

        let result = ws.write_file("new1.txt", "from task_b");
        assert!(result.contains("Blocked by policy") || result.contains("conflict"));

        ws.end_parallel_write_group("g1");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_repo_map() {
        let (dir, ws) = make_workspace_named("repomap");
        fs::write(dir.join("main.py"), "def hello():\n    pass\n").unwrap();
        let result = ws.repo_map(None, None);
        assert!(result.contains("main.py") || result.contains("no files"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_image_not_found() {
        let (dir, ws) = make_workspace_named("img_nf");
        let (text, data) = ws.read_image("missing.png");
        assert!(text.contains("File not found"));
        assert!(data.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_hashline_edit() {
        let (dir, ws) = make_workspace_named("hashline");
        fs::write(dir.join("hl.txt"), "first\nsecond\nthird\n").unwrap();

        let read_result = ws.read_file("hl.txt", true);
        let lines: Vec<&str> = read_result.lines().collect();
        let line2 = lines[2];
        let pipe_idx = line2.find('|').unwrap();
        let anchor = &line2[..pipe_idx];

        let ops = vec![HashlineOp::SetLine {
            anchor: anchor.to_string(),
            content: "REPLACED".to_string(),
        }];
        let result = ws.hashline_edit("hl.txt", &ops);
        assert!(result.contains("Edited") || result.contains("edit"));

        let content = fs::read_to_string(dir.join("hl.txt")).unwrap();
        assert!(content.contains("REPLACED"));
        assert!(!content.contains("second"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_hashline_edit_json() {
        let (dir, ws) = make_workspace_named("hashline_json");
        fs::write(dir.join("hl2.txt"), "alpha\nbeta\n").unwrap();

        let read_result = ws.read_file("hl2.txt", true);
        let lines: Vec<&str> = read_result.lines().collect();
        let line1 = lines[1];
        let pipe_idx = line1.find('|').unwrap();
        let anchor = &line1[..pipe_idx];

        let edits = vec![serde_json::json!({
            "set_line": anchor,
            "content": "ALPHA_REPLACED"
        })];
        let result = ws.hashline_edit_json("hl2.txt", &edits);
        assert!(result.contains("Edited") || result.contains("edit"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_debug_format() {
        let (dir, ws) = make_workspace_named("debug");
        let debug = format!("{:?}", ws);
        assert!(debug.contains("WorkspaceTools"));
        assert!(debug.contains("root"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_set_exa_api_key() {
        let (dir, mut ws) = make_workspace_named("exa_key");
        assert!(ws.exa_client.api_key.is_none());
        ws.set_exa_api_key(Some("test-key".to_string()));
        assert_eq!(ws.exa_client.api_key.as_deref(), Some("test-key"));
        let _ = fs::remove_dir_all(&dir);
    }
}
