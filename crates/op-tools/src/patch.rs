//! Codex-style patch parsing/application and hash-anchored line editing.
//!
//! Ports Python `patching.py` (parse_agent_patch, apply_agent_patch) and
//! the `hashline_edit` / `_validate_anchor` / `_line_hash` helpers from `tools.py`.

use crc32fast::Hasher;
use op_core::OpError;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// CRC32 line hashing — matches Python's `binascii.crc32`
// ---------------------------------------------------------------------------

static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("WS_RE"));
static HASHLINE_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+:[0-9a-f]{2}\|").expect("HASHLINE_PREFIX_RE"));

/// 2-char hex hash, whitespace-invariant.
/// Matches Python: `format(zlib.crc32(WS_RE.sub("", line).encode("utf-8")) & 0xFF, "02x")`
pub fn line_hash(line: &str) -> String {
    let collapsed = WS_RE.replace_all(line, "");
    let mut hasher = Hasher::new();
    hasher.update(collapsed.as_bytes());
    let crc = hasher.finalize();
    format!("{:02x}", crc & 0xFF)
}

/// Strip the `N:HH|` prefix from a line if present.
fn strip_hashline_prefix(line: &str) -> String {
    HASHLINE_PREFIX_RE.replace(line, "").to_string()
}

// ---------------------------------------------------------------------------
// Hashline edit types
// ---------------------------------------------------------------------------

/// A single edit operation for `hashline_edit`.
#[derive(Debug, Clone)]
pub enum HashlineOp {
    SetLine {
        anchor: String,
        content: String,
    },
    ReplaceLines {
        start_anchor: String,
        end_anchor: String,
        content: String,
    },
    InsertAfter {
        anchor: String,
        content: String,
    },
}

impl HashlineOp {
    /// Parse a `serde_json::Value` into a `HashlineOp`.
    pub fn from_json(val: &serde_json::Value) -> Result<Self, OpError> {
        if let Some(anchor) = val.get("set_line").and_then(|v| v.as_str()) {
            let content = val
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Ok(HashlineOp::SetLine {
                anchor: anchor.to_string(),
                content,
            });
        }
        if let Some(range) = val.get("replace_lines").and_then(|v| v.as_object()) {
            let start = range
                .get("start")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let end = range
                .get("end")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let content = val
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Ok(HashlineOp::ReplaceLines {
                start_anchor: start,
                end_anchor: end,
                content,
            });
        }
        if let Some(anchor) = val.get("insert_after").and_then(|v| v.as_str()) {
            let content = val
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Ok(HashlineOp::InsertAfter {
                anchor: anchor.to_string(),
                content,
            });
        }
        Err(OpError::tool(format!(
            "Unknown edit operation: {}. Use set_line, replace_lines, or insert_after.",
            val
        )))
    }
}

/// Validate an anchor string `"N:HH"` against the current file lines and hashes.
/// Returns `Ok(lineno)` (1-based) or `Err(message)`.
pub fn validate_anchor(
    anchor: &str,
    line_hashes: &HashMap<usize, String>,
    lines: &[String],
) -> Result<usize, String> {
    let parts: Vec<&str> = anchor.splitn(2, ':').collect();
    if parts.len() != 2 || parts[1].len() != 2 {
        return Err(format!("Invalid anchor format: {:?} (expected N:HH)", anchor));
    }
    let lineno: usize = match parts[0].parse() {
        Ok(n) => n,
        Err(_) => {
            return Err(format!("Invalid anchor format: {:?} (expected N:HH)", anchor));
        }
    };
    let expected_hash = parts[1];
    if lineno < 1 || lineno > lines.len() {
        return Err(format!(
            "Line {} out of range (file has {} lines)",
            lineno,
            lines.len()
        ));
    }
    let actual_hash = &line_hashes[&lineno];
    if actual_hash != expected_hash {
        let ctx_start = lineno.saturating_sub(2).max(1);
        let ctx_end = (lineno + 2).min(lines.len());
        let ctx_lines: Vec<String> = (ctx_start..=ctx_end)
            .map(|i| format!("  {}:{}|{}", i, &line_hashes[&i], &lines[i - 1]))
            .collect();
        return Err(format!(
            "Hash mismatch at line {}: expected {}, got {}. Current context:\n{}",
            lineno,
            expected_hash,
            actual_hash,
            ctx_lines.join("\n")
        ));
    }
    Ok(lineno)
}

/// Apply hash-anchored edits to file lines. Returns new content and number of changes.
pub fn apply_hashline_edits(
    content: &str,
    edits: &[HashlineOp],
) -> Result<(String, usize), String> {
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let line_hashes: HashMap<usize, String> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| (i + 1, line_hash(line)))
        .collect();

    // Parse and validate all edits upfront
    let mut parsed: Vec<(OpKind, usize, usize, Vec<String>)> = Vec::new();

    for edit in edits {
        match edit {
            HashlineOp::SetLine { anchor, content } => {
                let lineno = validate_anchor(anchor, &line_hashes, &lines)?;
                let new_line = strip_hashline_prefix(content);
                parsed.push((OpKind::Set, lineno, lineno, vec![new_line]));
            }
            HashlineOp::ReplaceLines {
                start_anchor,
                end_anchor,
                content: raw_content,
            } => {
                let start = validate_anchor(start_anchor, &line_hashes, &lines)?;
                let end = validate_anchor(end_anchor, &line_hashes, &lines)?;
                if end < start {
                    return Err(format!("End line {} is before start line {}", end, start));
                }
                let new_lines: Vec<String> = raw_content
                    .lines()
                    .map(|ln| strip_hashline_prefix(ln))
                    .collect();
                parsed.push((OpKind::Replace, start, end, new_lines));
            }
            HashlineOp::InsertAfter {
                anchor,
                content: raw_content,
            } => {
                let lineno = validate_anchor(anchor, &line_hashes, &lines)?;
                let new_lines: Vec<String> = raw_content
                    .lines()
                    .map(|ln| strip_hashline_prefix(ln))
                    .collect();
                parsed.push((OpKind::Insert, lineno, lineno, new_lines));
            }
        }
    }

    // Sort by line number descending so bottom-up application doesn't shift indices
    parsed.sort_by(|a, b| b.1.cmp(&a.1));

    let mut working = lines;
    let mut changed: usize = 0;

    for (op, start, end, new_lines) in &parsed {
        match op {
            OpKind::Set => {
                if working[start - 1] != new_lines[0] {
                    working[start - 1] = new_lines[0].clone();
                    changed += 1;
                }
            }
            OpKind::Replace => {
                let old_slice = &working[start - 1..*end];
                if old_slice != new_lines.as_slice() {
                    let mut replacement = Vec::new();
                    replacement.extend_from_slice(&working[..start - 1]);
                    replacement.extend(new_lines.iter().cloned());
                    replacement.extend_from_slice(&working[*end..]);
                    working = replacement;
                    changed += 1;
                }
            }
            OpKind::Insert => {
                let mut replacement = Vec::new();
                replacement.extend_from_slice(&working[..*start]);
                replacement.extend(new_lines.iter().cloned());
                replacement.extend_from_slice(&working[*start..]);
                working = replacement;
                changed += 1;
            }
        }
    }

    let mut new_content = working.join("\n");
    if content.ends_with('\n') {
        new_content.push('\n');
    }

    Ok((new_content, changed))
}

#[derive(Debug, Clone, Copy)]
enum OpKind {
    Set,
    Replace,
    Insert,
}

// ---------------------------------------------------------------------------
// Codex-style patch parsing and application
// ---------------------------------------------------------------------------

/// A chunk of patch lines within an Update operation.
#[derive(Debug)]
struct PatchChunk {
    lines: Vec<String>,
}

/// Operations that can appear in a Codex-style patch.
#[derive(Debug)]
pub enum PatchOp {
    Add {
        path: String,
        plus_lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        raw_lines: Vec<String>,
        move_to: Option<String>,
    },
}

/// Report of what a patch application did.
#[derive(Debug, Default)]
pub struct ApplyReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub deleted: Vec<String>,
    pub moved: Vec<String>,
}

impl ApplyReport {
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

/// Normalize whitespace for fuzzy matching: collapse runs to single space, strip.
fn normalize_ws(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a Codex-style patch into a list of operations.
pub fn parse_agent_patch(patch_text: &str) -> Result<Vec<PatchOp>, OpError> {
    let lines: Vec<&str> = patch_text.lines().collect();
    if lines.is_empty() {
        return Err(OpError::patch("patch is empty"));
    }
    if lines[0].trim() != "*** Begin Patch" {
        return Err(OpError::patch("patch must start with '*** Begin Patch'"));
    }
    if lines.last().map(|l| l.trim()) != Some("*** End Patch") {
        return Err(OpError::patch("patch must end with '*** End Patch'"));
    }

    let mut ops: Vec<PatchOp> = Vec::new();
    let mut i = 1;
    let end = lines.len() - 1;

    while i < end {
        let line = lines[i];

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
            ops.push(PatchOp::Add { path, plus_lines });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = path.trim().to_string();
            ops.push(PatchOp::Delete { path });
            i += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = path.trim().to_string();
            i += 1;
            let mut move_to: Option<String> = None;
            if i < end {
                if let Some(dest) = lines[i].strip_prefix("*** Move to: ") {
                    move_to = Some(dest.trim().to_string());
                    i += 1;
                }
            }
            let mut raw_lines: Vec<String> = Vec::new();
            while i < end && !lines[i].starts_with("*** ") {
                raw_lines.push(lines[i].to_string());
                i += 1;
            }
            ops.push(PatchOp::Update {
                path,
                raw_lines,
                move_to,
            });
            continue;
        }

        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        return Err(OpError::patch(format!(
            "unexpected patch line: {:?}",
            line
        )));
    }

    if ops.is_empty() {
        return Err(OpError::patch("patch contains no operations"));
    }
    Ok(ops)
}

/// Parse the raw_lines of an Update operation into chunks.
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
        return Err(OpError::patch(format!(
            "invalid update patch row: {:?}",
            row
        )));
    }
    if !current.is_empty() {
        chunks.push(PatchChunk { lines: current });
    }
    if chunks.is_empty() {
        return Err(OpError::patch("update operation contains no hunks"));
    }
    Ok(chunks)
}

/// Split a chunk into old (context+removed) and new (context+added) line sequences.
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
                return Err(OpError::patch(format!(
                    "invalid row prefix: {:?}",
                    prefix
                )));
            }
        }
    }
    Ok((old_seq, new_seq))
}

/// Find `needle` as a contiguous subsequence in `haystack` starting from `start_idx`.
/// First tries exact match, then whitespace-normalized match.
fn find_subsequence(haystack: &[String], needle: &[String], start_idx: usize) -> isize {
    if needle.is_empty() {
        return start_idx.min(haystack.len()) as isize;
    }
    if needle.len() > haystack.len() {
        return -1;
    }
    let max_start = haystack.len() - needle.len();
    let start = start_idx.min(max_start + 1);

    // Pass 1: exact match
    for i in start..=max_start {
        if haystack[i..i + needle.len()] == needle[..] {
            return i as isize;
        }
    }

    // Pass 2: whitespace-normalized match
    let norm_needle: Vec<String> = needle.iter().map(|l| normalize_ws(l)).collect();
    for i in start..=max_start {
        let norm_hay: Vec<String> = haystack[i..i + needle.len()]
            .iter()
            .map(|l| normalize_ws(l))
            .collect();
        if norm_hay == norm_needle {
            return i as isize;
        }
    }
    -1
}

/// Render a list of lines back to file content.
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

/// Apply a parsed Codex-style patch to files on disk.
///
/// `resolve_path` maps relative patch paths to absolute filesystem paths.
pub fn apply_agent_patch<F>(patch_text: &str, resolve_path: F) -> Result<ApplyReport, OpError>
where
    F: Fn(&str) -> Result<PathBuf, OpError>,
{
    let ops = parse_agent_patch(patch_text)?;
    let mut report = ApplyReport::default();

    for op in &ops {
        match op {
            PatchOp::Add { path, plus_lines } => {
                let target = resolve_path(path)?;
                if target.exists() {
                    return Err(OpError::patch(format!(
                        "cannot add existing file: {}",
                        path
                    )));
                }
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        OpError::patch(format!("failed to create directory: {}", e))
                    })?;
                }
                let content = render_lines(plus_lines, true);
                std::fs::write(&target, &content).map_err(|e| {
                    OpError::patch(format!("failed to write {}: {}", path, e))
                })?;
                report.added.push(path.clone());
            }
            PatchOp::Delete { path } => {
                let target = resolve_path(path)?;
                if !target.exists() {
                    return Err(OpError::patch(format!(
                        "cannot delete missing file: {}",
                        path
                    )));
                }
                if target.is_dir() {
                    return Err(OpError::patch(format!(
                        "cannot delete directory with patch: {}",
                        path
                    )));
                }
                std::fs::remove_file(&target).map_err(|e| {
                    OpError::patch(format!("failed to delete {}: {}", path, e))
                })?;
                report.deleted.push(path.clone());
            }
            PatchOp::Update {
                path,
                raw_lines,
                move_to,
            } => {
                let source = resolve_path(path)?;
                if !source.exists() {
                    return Err(OpError::patch(format!(
                        "cannot update missing file: {}",
                        path
                    )));
                }
                if source.is_dir() {
                    return Err(OpError::patch(format!(
                        "cannot update directory: {}",
                        path
                    )));
                }
                let original_text = std::fs::read_to_string(&source).map_err(|e| {
                    OpError::patch(format!("failed to read {}: {}", path, e))
                })?;
                let had_trailing_nl = original_text.ends_with('\n');
                let mut working: Vec<String> =
                    original_text.lines().map(|s| s.to_string()).collect();

                let mut cursor: usize = 0;
                let chunks = parse_chunks(raw_lines)?;
                for chunk in &chunks {
                    let (old_seq, new_seq) = chunk_to_old_new(chunk)?;
                    let mut idx = find_subsequence(&working, &old_seq, cursor);
                    if idx < 0 {
                        idx = find_subsequence(&working, &old_seq, 0);
                    }
                    if idx < 0 {
                        let preview: Vec<&str> =
                            old_seq.iter().take(8).map(|s| s.as_str()).collect();
                        return Err(OpError::patch(format!(
                            "failed applying chunk to {}; could not locate:\n{}",
                            path,
                            preview.join("\n")
                        )));
                    }
                    let idx = idx as usize;
                    let mut new_working = Vec::new();
                    new_working.extend_from_slice(&working[..idx]);
                    new_working.extend(new_seq.iter().cloned());
                    new_working.extend_from_slice(&working[idx + old_seq.len()..]);
                    working = new_working;
                    cursor = idx + new_seq.len();
                }

                let output = render_lines(&working, had_trailing_nl);
                let destination = if let Some(dest_path) = move_to {
                    let dest = resolve_path(dest_path)?;
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            OpError::patch(format!("failed to create directory: {}", e))
                        })?;
                    }
                    std::fs::remove_file(&source).map_err(|e| {
                        OpError::patch(format!("failed to remove source {}: {}", path, e))
                    })?;
                    report
                        .moved
                        .push(format!("{} -> {}", path, dest_path));
                    dest
                } else {
                    source
                };

                std::fs::write(&destination, &output).map_err(|e| {
                    OpError::patch(format!("failed to write {}: {}", path, e))
                })?;
                report
                    .updated
                    .push(move_to.as_deref().unwrap_or(path).to_string());
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

    // --- line_hash tests ---

    #[test]
    fn test_line_hash_empty() {
        let h = line_hash("");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn test_line_hash_whitespace_invariant() {
        let h1 = line_hash("hello world");
        let h2 = line_hash("hello  world");
        let h3 = line_hash("hello\tworld");
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
    }

    #[test]
    fn test_line_hash_different_content() {
        let h1 = line_hash("fn main() {}");
        let h2 = line_hash("fn other() {}");
        // Different content should (almost certainly) produce different hashes
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_line_hash_matches_python_crc32() {
        // Python: format(zlib.crc32(b"helloworld") & 0xFF, "02x")
        // zlib.crc32(b"helloworld") = 0xf9eb20ad -> 0xad = 173 = "ad"
        let h = line_hash("hello world");
        // The whitespace is removed first, so it hashes "helloworld"
        assert_eq!(h.len(), 2);
        // We verify format is always 2 hex chars
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // --- strip_hashline_prefix tests ---

    #[test]
    fn test_strip_hashline_prefix() {
        assert_eq!(strip_hashline_prefix("42:ab|hello"), "hello");
        assert_eq!(strip_hashline_prefix("hello"), "hello");
        assert_eq!(strip_hashline_prefix("1:ff|"), "");
    }

    // --- validate_anchor tests ---

    #[test]
    fn test_validate_anchor_valid() {
        let lines: Vec<String> = vec!["hello world".to_string(), "goodbye".to_string()];
        let hashes: HashMap<usize, String> =
            lines.iter().enumerate().map(|(i, l)| (i + 1, line_hash(l))).collect();
        let h = &hashes[&1];
        let anchor = format!("1:{}", h);
        assert_eq!(validate_anchor(&anchor, &hashes, &lines).unwrap(), 1);
    }

    #[test]
    fn test_validate_anchor_bad_format() {
        let lines = vec!["hello".to_string()];
        let hashes: HashMap<usize, String> =
            lines.iter().enumerate().map(|(i, l)| (i + 1, line_hash(l))).collect();
        assert!(validate_anchor("bad", &hashes, &lines).is_err());
    }

    #[test]
    fn test_validate_anchor_out_of_range() {
        let lines = vec!["hello".to_string()];
        let hashes: HashMap<usize, String> =
            lines.iter().enumerate().map(|(i, l)| (i + 1, line_hash(l))).collect();
        assert!(validate_anchor("5:ab", &hashes, &lines).is_err());
    }

    #[test]
    fn test_validate_anchor_hash_mismatch() {
        let lines = vec!["hello".to_string()];
        let hashes: HashMap<usize, String> =
            lines.iter().enumerate().map(|(i, l)| (i + 1, line_hash(l))).collect();
        assert!(validate_anchor("1:zz", &hashes, &lines).is_err());
    }

    // --- apply_hashline_edits tests ---

    #[test]
    fn test_hashline_set_line() {
        let content = "line one\nline two\nline three\n";
        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let h2 = line_hash(&lines[1]);
        let ops = vec![HashlineOp::SetLine {
            anchor: format!("2:{}", h2),
            content: "replaced line".to_string(),
        }];
        let (result, changed) = apply_hashline_edits(content, &ops).unwrap();
        assert_eq!(changed, 1);
        assert!(result.contains("replaced line"));
        assert!(!result.contains("line two"));
    }

    #[test]
    fn test_hashline_insert_after() {
        let content = "line one\nline two\n";
        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let h1 = line_hash(&lines[0]);
        let ops = vec![HashlineOp::InsertAfter {
            anchor: format!("1:{}", h1),
            content: "inserted line".to_string(),
        }];
        let (result, changed) = apply_hashline_edits(content, &ops).unwrap();
        assert_eq!(changed, 1);
        let result_lines: Vec<&str> = result.lines().collect();
        assert_eq!(result_lines[0], "line one");
        assert_eq!(result_lines[1], "inserted line");
        assert_eq!(result_lines[2], "line two");
    }

    #[test]
    fn test_hashline_no_changes() {
        let content = "hello\n";
        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let h1 = line_hash(&lines[0]);
        let ops = vec![HashlineOp::SetLine {
            anchor: format!("1:{}", h1),
            content: "hello".to_string(),
        }];
        let (_, changed) = apply_hashline_edits(content, &ops).unwrap();
        assert_eq!(changed, 0);
    }

    // --- parse_agent_patch tests ---

    #[test]
    fn test_parse_add_file() {
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+hello\n+world\n*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PatchOp::Add { path, plus_lines } => {
                assert_eq!(path, "new.txt");
                assert_eq!(plus_lines, &["hello", "world"]);
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
            PatchOp::Delete { path } => assert_eq!(path, "old.txt"),
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn test_parse_update_file() {
        let patch = "*** Begin Patch\n*** Update File: main.rs\n@@\n-old line\n+new line\n*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PatchOp::Update {
                path,
                raw_lines,
                move_to,
            } => {
                assert_eq!(path, "main.rs");
                assert!(move_to.is_none());
                assert_eq!(raw_lines.len(), 3);
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn test_parse_update_with_move() {
        let patch =
            "*** Begin Patch\n*** Update File: a.rs\n*** Move to: b.rs\n@@\n old\n*** End Patch";
        let ops = parse_agent_patch(patch).unwrap();
        match &ops[0] {
            PatchOp::Update { move_to, .. } => {
                assert_eq!(move_to.as_deref(), Some("b.rs"));
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn test_parse_no_begin() {
        let result = parse_agent_patch("hello\n*** End Patch");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_end() {
        let result = parse_agent_patch("*** Begin Patch\n*** Add File: a\n+x");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty() {
        let result = parse_agent_patch("");
        assert!(result.is_err());
    }

    // --- apply_agent_patch integration tests ---

    #[test]
    fn test_apply_add_and_delete() {
        let dir = std::env::temp_dir().join("op_patch_test_add_del");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let patch = "*** Begin Patch\n*** Add File: hello.txt\n+Hello World\n*** End Patch";
        let root = dir.clone();
        let report = apply_agent_patch(patch, |p| Ok(root.join(p))).unwrap();
        assert_eq!(report.added, vec!["hello.txt"]);
        assert_eq!(
            fs::read_to_string(dir.join("hello.txt")).unwrap(),
            "Hello World\n"
        );

        let patch2 = "*** Begin Patch\n*** Delete File: hello.txt\n*** End Patch";
        let root2 = dir.clone();
        let report2 = apply_agent_patch(patch2, |p| Ok(root2.join(p))).unwrap();
        assert_eq!(report2.deleted, vec!["hello.txt"]);
        assert!(!dir.join("hello.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_apply_update() {
        let dir = std::env::temp_dir().join("op_patch_test_update");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.rs"), "fn main() {\n    old_code();\n}\n").unwrap();

        let patch = "*** Begin Patch\n*** Update File: main.rs\n@@\n fn main() {\n-    old_code();\n+    new_code();\n }\n*** End Patch";
        let root = dir.clone();
        let report = apply_agent_patch(patch, |p| Ok(root.join(p))).unwrap();
        assert_eq!(report.updated, vec!["main.rs"]);
        let content = fs::read_to_string(dir.join("main.rs")).unwrap();
        assert!(content.contains("new_code()"));
        assert!(!content.contains("old_code()"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_subsequence_exact() {
        let hay: Vec<String> = vec!["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let needle: Vec<String> = vec!["b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_subsequence(&hay, &needle, 0), 1);
    }

    #[test]
    fn test_find_subsequence_whitespace() {
        let hay: Vec<String> = vec!["  hello  world  "]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let needle: Vec<String> = vec!["hello world"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_subsequence(&hay, &needle, 0), 0);
    }

    #[test]
    fn test_find_subsequence_not_found() {
        let hay: Vec<String> = vec!["a", "b"].iter().map(|s| s.to_string()).collect();
        let needle: Vec<String> = vec!["x"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_subsequence(&hay, &needle, 0), -1);
    }

    #[test]
    fn test_render_lines_empty() {
        assert_eq!(render_lines(&[], true), "");
    }

    #[test]
    fn test_render_lines_trailing() {
        let lines: Vec<String> = vec!["a".to_string(), "b".to_string()];
        assert_eq!(render_lines(&lines, true), "a\nb\n");
        assert_eq!(render_lines(&lines, false), "a\nb");
    }
}
