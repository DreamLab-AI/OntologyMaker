//! Codex-style patch parsing and application.
//!
//! Port of `agent/patching.py`.
//!
//! Patch format:
//! ```text
//! *** Begin Patch
//! *** Add File: path/to/file.rs
//! +line1
//! +line2
//! *** Update File: path/to/existing.rs
//! @@
//!  context line
//! -removed line
//! +added line
//! *** Delete File: path/to/old.rs
//! *** End Patch
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use op_core::OpError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A hunk of context/add/remove lines within an Update operation.
#[derive(Debug, Clone)]
struct PatchChunk {
    lines: Vec<String>,
}

/// Add a new file.
#[derive(Debug, Clone)]
pub struct AddFileOp {
    pub path: String,
    pub plus_lines: Vec<String>,
}

/// Delete an existing file.
#[derive(Debug, Clone)]
pub struct DeleteFileOp {
    pub path: String,
}

/// Update an existing file, optionally moving it.
#[derive(Debug, Clone)]
pub struct UpdateFileOp {
    pub path: String,
    pub raw_lines: Vec<String>,
    pub move_to: Option<String>,
}

/// A single patch operation.
#[derive(Debug, Clone)]
pub enum PatchOp {
    Add(AddFileOp),
    Delete(DeleteFileOp),
    Update(UpdateFileOp),
}

/// Report of what a patch application changed.
#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub deleted: Vec<String>,
    pub moved: Vec<String>,
}

impl ApplyReport {
    /// Render a human-readable summary.
    pub fn render(&self) -> String {
        let mut lines = vec!["Patch applied successfully.".to_string()];
        if !self.added.is_empty() {
            lines.push("Added:".to_string());
            for p in &self.added {
                lines.push(format!("- {}", p));
            }
        }
        if !self.updated.is_empty() {
            lines.push("Updated:".to_string());
            for p in &self.updated {
                lines.push(format!("- {}", p));
            }
        }
        if !self.deleted.is_empty() {
            lines.push("Deleted:".to_string());
            for p in &self.deleted {
                lines.push(format!("- {}", p));
            }
        }
        if !self.moved.is_empty() {
            lines.push("Moved:".to_string());
            for p in &self.moved {
                lines.push(format!("- {}", p));
            }
        }
        lines.join("\n")
    }
}

/// Path resolver: maps relative paths from the patch to absolute paths.
pub type ResolvePathFn = Box<dyn Fn(&str) -> PathBuf>;

/// Convenience: build a resolver that joins paths against a workspace root.
pub fn workspace_resolver(workspace: &Path) -> ResolvePathFn {
    let root = workspace.to_path_buf();
    Box::new(move |rel: &str| root.join(rel))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Collapse all whitespace runs to a single space and strip.
fn normalize_ws(line: &str) -> String {
    line.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Parse a Codex-style agent patch into a list of operations.
pub fn parse_agent_patch(patch_text: &str) -> Result<Vec<PatchOp>, OpError> {
    let lines: Vec<&str> = patch_text.lines().collect();
    if lines.is_empty() {
        return Err(OpError::patch("patch is empty"));
    }
    if lines[0].trim() != "*** Begin Patch" {
        return Err(OpError::patch("patch must start with '*** Begin Patch'"));
    }
    if lines[lines.len() - 1].trim() != "*** End Patch" {
        return Err(OpError::patch("patch must end with '*** End Patch'"));
    }

    let mut ops: Vec<PatchOp> = Vec::new();
    let mut i = 1;
    let end = lines.len() - 1;

    while i < end {
        let line = lines[i];

        // *** Add File: <path>
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = path.trim().to_string();
            i += 1;
            let mut plus_lines: Vec<String> = Vec::new();
            while i < end && !lines[i].starts_with("*** ") {
                let row = lines[i];
                if !row.starts_with('+') {
                    return Err(OpError::patch(format!(
                        "add file '{}' contains non '+' line: {:?}",
                        path, row
                    )));
                }
                plus_lines.push(row[1..].to_string());
                i += 1;
            }
            ops.push(PatchOp::Add(AddFileOp { path, plus_lines }));
            continue;
        }

        // *** Delete File: <path>
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = path.trim().to_string();
            ops.push(PatchOp::Delete(DeleteFileOp { path }));
            i += 1;
            continue;
        }

        // *** Update File: <path>
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = path.trim().to_string();
            i += 1;
            let mut move_to: Option<String> = None;
            if i < end {
                if let Some(mt) = lines[i].strip_prefix("*** Move to: ") {
                    move_to = Some(mt.trim().to_string());
                    i += 1;
                }
            }
            let mut raw_lines: Vec<String> = Vec::new();
            while i < end && !lines[i].starts_with("*** ") {
                raw_lines.push(lines[i].to_string());
                i += 1;
            }
            ops.push(PatchOp::Update(UpdateFileOp {
                path,
                raw_lines,
                move_to,
            }));
            continue;
        }

        // Blank lines between operations are allowed.
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        return Err(OpError::patch(format!("unexpected patch line: {:?}", line)));
    }

    if ops.is_empty() {
        return Err(OpError::patch("patch contains no operations"));
    }
    Ok(ops)
}

// ---------------------------------------------------------------------------
// Chunk helpers
// ---------------------------------------------------------------------------

/// Parse the raw_lines of an UpdateFileOp into chunks separated by `@@` markers.
fn parse_chunks(raw_lines: &[String]) -> Result<Vec<PatchChunk>, OpError> {
    let mut chunks: Vec<PatchChunk> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for row in raw_lines {
        if row.starts_with("@@") {
            if !current.is_empty() {
                chunks.push(PatchChunk { lines: current });
                current = Vec::new();
            }
            continue;
        }
        if row == "*** End of File" {
            continue;
        }
        if row.starts_with(' ') || row.starts_with('+') || row.starts_with('-') {
            current.push(row.clone());
            continue;
        }
        return Err(OpError::patch(format!("invalid update patch row: {:?}", row)));
    }
    if !current.is_empty() {
        chunks.push(PatchChunk { lines: current });
    }
    if chunks.is_empty() {
        return Err(OpError::patch("update operation contains no hunks"));
    }
    Ok(chunks)
}

/// Split a chunk's lines into (old_seq, new_seq) based on prefix characters.
fn chunk_to_old_new(chunk: &PatchChunk) -> Result<(Vec<String>, Vec<String>), OpError> {
    let mut old_seq: Vec<String> = Vec::new();
    let mut new_seq: Vec<String> = Vec::new();

    for row in &chunk.lines {
        if row.is_empty() {
            return Err(OpError::patch("empty line in patch chunk"));
        }
        let prefix = &row[..1];
        let payload = &row[1..];
        match prefix {
            " " => {
                old_seq.push(payload.to_string());
                new_seq.push(payload.to_string());
            }
            "-" => {
                old_seq.push(payload.to_string());
            }
            "+" => {
                new_seq.push(payload.to_string());
            }
            _ => {
                return Err(OpError::patch(format!("invalid row prefix: {:?}", prefix)));
            }
        }
    }
    Ok((old_seq, new_seq))
}

/// Find `needle` in `haystack` starting from `start_idx`.
///
/// Pass 1: exact match. Pass 2: whitespace-normalized match.
/// Returns the index or -1 if not found.
fn find_subsequence(haystack: &[String], needle: &[String], start_idx: usize) -> isize {
    if needle.is_empty() {
        return start_idx.min(haystack.len()) as isize;
    }
    let needle_len = needle.len();
    if haystack.len() < needle_len {
        return -1;
    }
    let max_start = haystack.len() - needle_len;

    // Pass 1: exact match.
    let begin = start_idx.min(max_start + 1);
    for i in begin..=max_start {
        if haystack[i..i + needle_len] == needle[..] {
            return i as isize;
        }
    }

    // Pass 2: whitespace-normalized match.
    let norm_needle: Vec<String> = needle.iter().map(|l| normalize_ws(l)).collect();
    for i in begin..=max_start {
        let norm_hay: Vec<String> = haystack[i..i + needle_len]
            .iter()
            .map(|l| normalize_ws(l))
            .collect();
        if norm_hay == norm_needle {
            return i as isize;
        }
    }

    -1
}

/// Render lines back into a string.
fn render_lines(lines: &[String], prefer_trailing_newline: bool) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join("\n");
    if prefer_trailing_newline {
        text.push('\n');
    }
    text
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

/// Apply a Codex-style agent patch against the filesystem.
///
/// `resolve_path` maps relative paths from the patch to absolute filesystem paths.
pub fn apply_agent_patch(
    patch_text: &str,
    resolve_path: &dyn Fn(&str) -> PathBuf,
) -> Result<ApplyReport, OpError> {
    let ops = parse_agent_patch(patch_text)?;
    let mut report = ApplyReport::default();

    for op in &ops {
        match op {
            PatchOp::Add(add) => {
                let target = resolve_path(&add.path);
                if target.exists() {
                    return Err(OpError::patch(format!(
                        "cannot add existing file: {}",
                        add.path
                    )));
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let content = render_lines(&add.plus_lines, true);
                fs::write(&target, &content)?;
                report.added.push(add.path.clone());
            }
            PatchOp::Delete(del) => {
                let target = resolve_path(&del.path);
                if !target.exists() {
                    return Err(OpError::patch(format!(
                        "cannot delete missing file: {}",
                        del.path
                    )));
                }
                if target.is_dir() {
                    return Err(OpError::patch(format!(
                        "cannot delete directory with patch: {}",
                        del.path
                    )));
                }
                fs::remove_file(&target)?;
                report.deleted.push(del.path.clone());
            }
            PatchOp::Update(upd) => {
                let source = resolve_path(&upd.path);
                if !source.exists() {
                    return Err(OpError::patch(format!(
                        "cannot update missing file: {}",
                        upd.path
                    )));
                }
                if source.is_dir() {
                    return Err(OpError::patch(format!(
                        "cannot update directory: {}",
                        upd.path
                    )));
                }
                let original_text = fs::read_to_string(&source)?;
                let old_lines: Vec<String> =
                    original_text.lines().map(String::from).collect();
                let had_trailing_nl = original_text.ends_with('\n');
                let mut working = old_lines;

                let mut cursor: usize = 0;
                let chunks = parse_chunks(&upd.raw_lines)?;
                for chunk in &chunks {
                    let (old_seq, new_seq) = chunk_to_old_new(chunk)?;
                    let mut idx = find_subsequence(&working, &old_seq, cursor);
                    if idx < 0 {
                        // Retry from beginning.
                        idx = find_subsequence(&working, &old_seq, 0);
                    }
                    if idx < 0 {
                        let preview: String = old_seq
                            .iter()
                            .take(8)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n");
                        return Err(OpError::patch(format!(
                            "failed applying chunk to {}; could not locate:\n{}",
                            upd.path, preview
                        )));
                    }
                    let idx = idx as usize;
                    let mut new_working =
                        Vec::with_capacity(working.len() - old_seq.len() + new_seq.len());
                    new_working.extend_from_slice(&working[..idx]);
                    new_working.extend(new_seq.iter().cloned());
                    new_working.extend_from_slice(&working[idx + old_seq.len()..]);
                    cursor = idx + new_seq.len();
                    working = new_working;
                }

                let output = render_lines(&working, had_trailing_nl);
                let destination = if let Some(ref move_to) = upd.move_to {
                    let dest = resolve_path(move_to);
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::remove_file(&source)?;
                    report.moved.push(format!("{} -> {}", upd.path, move_to));
                    dest
                } else {
                    source
                };
                fs::write(&destination, &output)?;
                report
                    .updated
                    .push(upd.move_to.clone().unwrap_or_else(|| upd.path.clone()));
            }
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("op_patch_test")
            .join(name)
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // -- parse tests --------------------------------------------------------

    #[test]
    fn test_parse_empty_patch() {
        let result = parse_agent_patch("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_begin() {
        let result = parse_agent_patch("hello\n*** End Patch");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_end() {
        let result = parse_agent_patch("*** Begin Patch\n*** Add File: foo.txt\n+hello");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_ops() {
        let result = parse_agent_patch("*** Begin Patch\n*** End Patch");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_add_file() {
        let patch = "*** Begin Patch\n*** Add File: src/new.rs\n+fn main() {}\n+\n*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PatchOp::Add(add) => {
                assert_eq!(add.path, "src/new.rs");
                assert_eq!(add.plus_lines, vec!["fn main() {}", ""]);
            }
            _ => panic!("expected Add"),
        }
    }

    #[test]
    fn test_parse_delete_file() {
        let patch = "*** Begin Patch\n*** Delete File: old.txt\n*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PatchOp::Delete(del) => assert_eq!(del.path, "old.txt"),
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn test_parse_update_file() {
        let patch = "\
*** Begin Patch
*** Update File: lib.rs
@@
 fn old() {
-    println!(\"old\");
+    println!(\"new\");
 }
*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PatchOp::Update(upd) => {
                assert_eq!(upd.path, "lib.rs");
                assert!(upd.move_to.is_none());
                assert_eq!(upd.raw_lines.len(), 5); // @@, context, -, +, context
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn test_parse_move_file() {
        let patch = "\
*** Begin Patch
*** Update File: old/path.rs
*** Move to: new/path.rs
@@
 line1
*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        match &ops[0] {
            PatchOp::Update(upd) => {
                assert_eq!(upd.path, "old/path.rs");
                assert_eq!(upd.move_to.as_deref(), Some("new/path.rs"));
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn test_parse_multiple_ops() {
        let patch = "\
*** Begin Patch
*** Add File: a.txt
+hello
*** Delete File: b.txt
*** Update File: c.txt
@@
 context
-old
+new
*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], PatchOp::Add(_)));
        assert!(matches!(&ops[1], PatchOp::Delete(_)));
        assert!(matches!(&ops[2], PatchOp::Update(_)));
    }

    #[test]
    fn test_parse_add_non_plus_line_error() {
        let patch = "*** Begin Patch\n*** Add File: x.txt\n+ok\nbad line\n*** End Patch";
        let result = parse_agent_patch(patch);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unexpected_line_error() {
        let patch = "*** Begin Patch\nrandom garbage\n*** End Patch";
        let result = parse_agent_patch(patch);
        assert!(result.is_err());
    }

    // -- apply tests --------------------------------------------------------

    #[test]
    fn test_apply_add_file() {
        let dir = temp_dir("apply_add");
        let patch = "\
*** Begin Patch
*** Add File: hello.txt
+Hello, world!
+Second line.
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.added, vec!["hello.txt"]);
        let content = fs::read_to_string(dir.join("hello.txt")).unwrap();
        assert_eq!(content, "Hello, world!\nSecond line.\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_add_existing_file_error() {
        let dir = temp_dir("apply_add_existing");
        fs::write(dir.join("exists.txt"), "old").unwrap();
        let patch = "\
*** Begin Patch
*** Add File: exists.txt
+new content
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let result = apply_agent_patch(patch, &resolve);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_add_nested_dir() {
        let dir = temp_dir("apply_add_nested");
        let patch = "\
*** Begin Patch
*** Add File: deep/nested/file.txt
+content
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.added, vec!["deep/nested/file.txt"]);
        assert!(dir.join("deep/nested/file.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_delete_file() {
        let dir = temp_dir("apply_delete");
        fs::write(dir.join("target.txt"), "to delete").unwrap();
        let patch = "\
*** Begin Patch
*** Delete File: target.txt
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.deleted, vec!["target.txt"]);
        assert!(!dir.join("target.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_delete_missing_error() {
        let dir = temp_dir("apply_delete_missing");
        let patch = "\
*** Begin Patch
*** Delete File: nonexistent.txt
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let result = apply_agent_patch(patch, &resolve);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_update_file() {
        let dir = temp_dir("apply_update");
        fs::write(
            dir.join("code.rs"),
            "fn main() {\n    println!(\"old\");\n}\n",
        )
        .unwrap();

        let patch = "\
*** Begin Patch
*** Update File: code.rs
@@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
 }
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.updated, vec!["code.rs"]);

        let content = fs::read_to_string(dir.join("code.rs")).unwrap();
        assert_eq!(content, "fn main() {\n    println!(\"new\");\n}\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_update_preserves_trailing_newline() {
        let dir = temp_dir("apply_trailing_nl");
        // File with trailing newline.
        fs::write(dir.join("a.txt"), "line1\nline2\n").unwrap();
        let patch = "\
*** Begin Patch
*** Update File: a.txt
@@
-line2
+line2_modified
*** End Patch";
        let resolve = workspace_resolver(&dir);
        apply_agent_patch(patch, &resolve).unwrap();
        let content = fs::read_to_string(dir.join("a.txt")).unwrap();
        assert!(content.ends_with('\n'));
        assert!(content.contains("line2_modified"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_update_no_trailing_newline() {
        let dir = temp_dir("apply_no_trailing_nl");
        // File without trailing newline.
        fs::write(dir.join("b.txt"), "line1\nline2").unwrap();
        let patch = "\
*** Begin Patch
*** Update File: b.txt
@@
-line2
+line2_new
*** End Patch";
        let resolve = workspace_resolver(&dir);
        apply_agent_patch(patch, &resolve).unwrap();
        let content = fs::read_to_string(dir.join("b.txt")).unwrap();
        assert!(!content.ends_with('\n'));
        assert!(content.contains("line2_new"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_update_missing_file_error() {
        let dir = temp_dir("apply_update_missing");
        let patch = "\
*** Begin Patch
*** Update File: ghost.txt
@@
-old
+new
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let result = apply_agent_patch(patch, &resolve);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_update_chunk_not_found_error() {
        let dir = temp_dir("apply_chunk_nf");
        fs::write(dir.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let patch = "\
*** Begin Patch
*** Update File: f.txt
@@
-nonexistent line
+replacement
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let result = apply_agent_patch(patch, &resolve);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_update_with_move() {
        let dir = temp_dir("apply_move");
        fs::write(dir.join("old.txt"), "content\n").unwrap();
        let patch = "\
*** Begin Patch
*** Update File: old.txt
*** Move to: subdir/new.txt
@@
-content
+modified content
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.moved, vec!["old.txt -> subdir/new.txt"]);
        assert_eq!(report.updated, vec!["subdir/new.txt"]);
        assert!(!dir.join("old.txt").exists());
        let content = fs::read_to_string(dir.join("subdir/new.txt")).unwrap();
        assert_eq!(content, "modified content\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_fuzzy_whitespace_matching() {
        let dir = temp_dir("apply_fuzzy");
        // File with irregular whitespace.
        fs::write(dir.join("ws.txt"), "  hello   world  \nline2\n").unwrap();
        // Patch refers to normalised version.
        let patch = "\
*** Begin Patch
*** Update File: ws.txt
@@
- hello   world
+ hello world
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.updated, vec!["ws.txt"]);
        let content = fs::read_to_string(dir.join("ws.txt")).unwrap();
        assert!(content.contains(" hello world"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_multiple_chunks() {
        let dir = temp_dir("apply_multi_chunk");
        fs::write(
            dir.join("multi.txt"),
            "alpha\nbeta\ngamma\ndelta\nepsilon\n",
        )
        .unwrap();

        let patch = "\
*** Begin Patch
*** Update File: multi.txt
@@
-beta
+BETA
@@
-delta
+DELTA
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.updated, vec!["multi.txt"]);

        let content = fs::read_to_string(dir.join("multi.txt")).unwrap();
        assert_eq!(content, "alpha\nBETA\ngamma\nDELTA\nepsilon\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_report_render() {
        let report = ApplyReport {
            added: vec!["new.rs".into()],
            updated: vec!["lib.rs".into()],
            deleted: vec!["old.rs".into()],
            moved: vec!["a.rs -> b.rs".into()],
        };
        let text = report.render();
        assert!(text.contains("Patch applied successfully."));
        assert!(text.contains("Added:"));
        assert!(text.contains("- new.rs"));
        assert!(text.contains("Updated:"));
        assert!(text.contains("- lib.rs"));
        assert!(text.contains("Deleted:"));
        assert!(text.contains("- old.rs"));
        assert!(text.contains("Moved:"));
        assert!(text.contains("- a.rs -> b.rs"));
    }

    #[test]
    fn test_apply_report_render_empty() {
        let report = ApplyReport::default();
        assert_eq!(report.render(), "Patch applied successfully.");
    }

    #[test]
    fn test_normalize_ws() {
        assert_eq!(normalize_ws("  hello   world  "), "hello world");
        assert_eq!(normalize_ws("a"), "a");
        assert_eq!(normalize_ws("  "), "");
    }

    #[test]
    fn test_find_subsequence_exact() {
        let hay: Vec<String> = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let needle: Vec<String> = vec!["b".into(), "c".into()];
        assert_eq!(find_subsequence(&hay, &needle, 0), 1);
        assert_eq!(find_subsequence(&hay, &needle, 1), 1);
        assert_eq!(find_subsequence(&hay, &needle, 2), -1);
    }

    #[test]
    fn test_find_subsequence_empty_needle() {
        let hay: Vec<String> = vec!["a".into(), "b".into()];
        assert_eq!(find_subsequence(&hay, &[], 0), 0);
        assert_eq!(find_subsequence(&hay, &[], 5), 2);
    }

    #[test]
    fn test_find_subsequence_fuzzy() {
        let hay: Vec<String> = vec!["  hello   world  ".into()];
        let needle: Vec<String> = vec!["hello world".into()];
        // Exact match fails, but fuzzy should succeed.
        assert_eq!(find_subsequence(&hay, &needle, 0), 0);
    }

    #[test]
    fn test_complex_patch_scenario() {
        let dir = temp_dir("apply_complex");
        fs::write(
            dir.join("main.rs"),
            "use std::io;\n\nfn main() {\n    let x = 1;\n    let y = 2;\n    println!(\"{}\", x + y);\n}\n",
        )
        .unwrap();
        fs::write(dir.join("helper.rs"), "pub fn help() {}\n").unwrap();

        let patch = "\
*** Begin Patch
*** Add File: config.rs
+pub const VERSION: &str = \"1.0\";
*** Update File: main.rs
@@
 use std::io;
+use std::fmt;
@@
-    let x = 1;
-    let y = 2;
+    let x = 10;
+    let y = 20;
+    let z = 30;
*** Delete File: helper.rs
*** End Patch";
        let resolve = workspace_resolver(&dir);
        let report = apply_agent_patch(patch, &resolve).unwrap();
        assert_eq!(report.added, vec!["config.rs"]);
        assert_eq!(report.updated, vec!["main.rs"]);
        assert_eq!(report.deleted, vec!["helper.rs"]);

        assert!(dir.join("config.rs").exists());
        assert!(!dir.join("helper.rs").exists());

        let main_content = fs::read_to_string(dir.join("main.rs")).unwrap();
        assert!(main_content.contains("use std::fmt;"));
        assert!(main_content.contains("let x = 10;"));
        assert!(main_content.contains("let z = 30;"));
        assert!(!main_content.contains("let x = 1;"));

        let _ = fs::remove_dir_all(&dir);
    }
}
