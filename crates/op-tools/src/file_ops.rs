//! File operations: list_files, read_file, write_file, edit_file, read_image.
//!
//! Ports the corresponding methods from Python `WorkspaceTools`.

use crate::patch::line_hash;
use base64::Engine;
use op_core::{ImageData, OpError};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_WALK_ENTRIES: usize = 50_000;

const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp"];
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024; // 20 MB

/// Clip text to a maximum character count, appending a truncation notice if needed.
pub fn clip(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let omitted = text.len() - max_chars;
    format!(
        "{}\n\n...[truncated {} chars]...",
        &text[..max_chars],
        omitted
    )
}

/// Resolve a raw path (relative or absolute) to an absolute path within the workspace.
/// Returns `Err` if the path escapes the workspace root.
pub fn resolve_path(raw_path: &str, root: &Path) -> Result<PathBuf, OpError> {
    let candidate = Path::new(raw_path);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    // Canonicalize might fail if path doesn't exist, so we do manual resolution
    let resolved = normalize_path(&candidate);
    let root_normalized = normalize_path(root);
    if resolved == root_normalized {
        return Ok(resolved);
    }
    if !resolved.starts_with(&root_normalized) {
        return Err(OpError::tool(format!("Path escapes workspace: {}", raw_path)));
    }
    Ok(resolved)
}

/// Normalize a path without requiring it to exist (no symlink resolution).
/// This collapses `.` and `..` components lexically.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

/// List files in the workspace. Uses `rg --files` if available, falls back to `os.walk`.
pub fn list_files(
    root: &Path,
    glob: Option<&str>,
    max_files_listed: usize,
    command_timeout_sec: u64,
) -> String {
    let lines = if which_rg() {
        let mut cmd = Command::new("rg");
        cmd.args(["--files", "--hidden", "-g", "!.git"]);
        if let Some(g) = glob {
            cmd.args(["-g", g]);
        }
        cmd.current_dir(root);
        match run_cmd_with_timeout(&mut cmd, command_timeout_sec) {
            Ok(output) => output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|s| s.to_string())
                .collect(),
            Err(_) => return "(list_files timed out)".to_string(),
        }
    } else {
        walk_files(root, glob, MAX_WALK_ENTRIES)
    };

    if lines.is_empty() {
        return "(no files)".to_string();
    }
    let clipped: Vec<&String> = lines.iter().take(max_files_listed).collect();
    let mut result = clipped
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > clipped.len() {
        result.push_str(&format!(
            "\n...[omitted {} files]...",
            lines.len() - clipped.len()
        ));
    }
    result
}

/// Read a file and return its contents with line numbers and optional hash prefixes.
pub fn read_file(
    path: &str,
    hashline: bool,
    root: &Path,
    max_file_chars: usize,
    files_read: &Mutex<HashSet<PathBuf>>,
) -> String {
    let resolved = match resolve_path(path, root) {
        Ok(p) => p,
        Err(e) => return e.to_string(),
    };
    if !resolved.exists() {
        return format!("File not found: {}", path);
    }
    if resolved.is_dir() {
        return format!("Path is a directory, not a file: {}", path);
    }
    let text = match std::fs::read_to_string(&resolved) {
        Ok(t) => t,
        Err(e) => return format!("Failed to read file {}: {}", path, e),
    };
    files_read.lock().insert(resolved.clone());
    let clipped = clip(&text, max_file_chars);
    let rel = resolved
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());

    let numbered = if hashline {
        clipped
            .lines()
            .enumerate()
            .map(|(i, line)| format!("{}:{}|{}", i + 1, line_hash(line), line))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        clipped
            .lines()
            .enumerate()
            .map(|(i, line)| format!("{}|{}", i + 1, line))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!("# {}\n{}", rel, numbered)
}

/// Read an image file and return metadata + base64-encoded data.
pub fn read_image(
    path: &str,
    root: &Path,
) -> (String, Option<ImageData>) {
    let resolved = match resolve_path(path, root) {
        Ok(p) => p,
        Err(e) => return (e.to_string(), None),
    };
    if !resolved.exists() {
        return (format!("File not found: {}", path), None);
    }
    if resolved.is_dir() {
        return (format!("Path is a directory, not a file: {}", path), None);
    }
    let ext = resolved
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return (
            format!(
                "Unsupported image format: {}. Supported: {}",
                ext,
                IMAGE_EXTENSIONS
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None,
        );
    }
    let size = match std::fs::metadata(&resolved) {
        Ok(m) => m.len(),
        Err(e) => return (format!("Failed to read image {}: {}", path, e), None),
    };
    if size > MAX_IMAGE_BYTES {
        return (
            format!(
                "Image too large: {} bytes (max {} bytes)",
                size, MAX_IMAGE_BYTES
            ),
            None,
        );
    }
    let raw = match std::fs::read(&resolved) {
        Ok(r) => r,
        Err(e) => return (format!("Failed to read image {}: {}", path, e), None),
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
    let media_type = match ext.as_str() {
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        _ => "application/octet-stream",
    };
    let rel = resolved
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    let text = format!("Image {} ({} bytes, {})", rel, raw.len(), media_type);
    (
        text,
        Some(ImageData {
            base64_data: b64,
            media_type: media_type.to_string(),
        }),
    )
}

/// Write (create/overwrite) a file in the workspace.
pub fn write_file(
    path: &str,
    content: &str,
    root: &Path,
    files_read: &Mutex<HashSet<PathBuf>>,
) -> String {
    let resolved = match resolve_path(path, root) {
        Ok(p) => p,
        Err(e) => return e.to_string(),
    };
    // Check if the file already exists but hasn't been read
    if resolved.exists() && resolved.is_file() && !files_read.lock().contains(&resolved) {
        return format!(
            "BLOCKED: {} already exists but has not been read. \
             Use read_file('{}') first, then edit via apply_patch or write_file.",
            path, path
        );
    }
    // Create parent directories if needed
    if let Some(parent) = resolved.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("Failed to write {}: {}", path, e);
        }
    }
    if let Err(e) = std::fs::write(&resolved, content) {
        return format!("Failed to write {}: {}", path, e);
    }
    files_read.lock().insert(resolved.clone());
    let rel = resolved
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    format!("Wrote {} chars to {}", content.len(), rel)
}

/// Edit a file by replacing a specific text span.
pub fn edit_file(
    path: &str,
    old_text: &str,
    new_text: &str,
    root: &Path,
    files_read: &Mutex<HashSet<PathBuf>>,
) -> String {
    let resolved = match resolve_path(path, root) {
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
    files_read.lock().insert(resolved.clone());

    let new_content = if content.contains(old_text) {
        let count = content.matches(old_text).count();
        if count > 1 {
            return format!(
                "edit_file failed: old_text appears {} times in {}. Provide more context to make it unique.",
                count, path
            );
        }
        content.replacen(old_text, new_text, 1)
    } else {
        // Fuzzy fallback: whitespace-normalized match
        let norm_old: String = old_text.split_whitespace().collect::<Vec<_>>().join(" ");
        let old_lines: Vec<&str> = old_text.lines().collect();
        let lines: Vec<&str> = content.lines().collect();
        let mut found = false;
        let mut result_content = content.clone();

        if !old_lines.is_empty() && old_lines.len() <= lines.len() {
            for i in 0..=lines.len() - old_lines.len() {
                let candidate: String = lines[i..i + old_lines.len()].join("\n");
                let norm_candidate: String =
                    candidate.split_whitespace().collect::<Vec<_>>().join(" ");
                if norm_candidate == norm_old {
                    // Reconstruct with original line endings
                    let before: String = if i > 0 {
                        let mut s = lines[..i].join("\n");
                        s.push('\n');
                        s
                    } else {
                        String::new()
                    };
                    let after: String = if i + old_lines.len() < lines.len() {
                        let mut s = String::from("\n");
                        s.push_str(&lines[i + old_lines.len()..].join("\n"));
                        s
                    } else {
                        String::new()
                    };
                    result_content = format!("{}{}{}", before, new_text, after);
                    found = true;
                    break;
                }
            }
        }

        if !found {
            return format!("edit_file failed: old_text not found in {}", path);
        }
        result_content
    };

    if let Err(e) = std::fs::write(&resolved, &new_content) {
        return format!("Failed to write {}: {}", path, e);
    }
    files_read.lock().insert(resolved.clone());
    let rel = resolved
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    format!("Edited {}", rel)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check if `rg` (ripgrep) is on PATH.
fn which_rg() -> bool {
    Command::new("which")
        .arg("rg")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a command with a timeout (synchronous). Returns stdout on success.
fn run_cmd_with_timeout(cmd: &mut Command, timeout_sec: u64) -> Result<String, OpError> {
    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| OpError::tool(format!("failed to start: {}", e)))?;

    let output = child
        .wait_with_output()
        .map_err(|e| OpError::tool(format!("failed to wait: {}", e)))?;

    // Note: std::process::Command doesn't natively support timeout, so we rely
    // on the command finishing. For the async path, `shell.rs` uses tokio timeout.
    // Here we just capture output.
    let _ = timeout_sec; // Used conceptually; the sync path doesn't enforce it.
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Walk the filesystem to list files (fallback when ripgrep is unavailable).
fn walk_files(root: &Path, glob_pattern: Option<&str>, max_entries: usize) -> Vec<String> {
    let mut all_paths: Vec<String> = Vec::new();
    let mut count = 0;
    walk_recursive(root, root, glob_pattern, &mut all_paths, &mut count, max_entries);
    all_paths.sort();
    all_paths
}

fn walk_recursive(
    current: &Path,
    root: &Path,
    glob_pattern: Option<&str>,
    paths: &mut Vec<String>,
    count: &mut usize,
    max_entries: usize,
) {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str == ".git" {
            continue;
        }

        if path.is_dir() {
            walk_recursive(&path, root, glob_pattern, paths, count, max_entries);
        } else {
            *count += 1;
            if *count > max_entries {
                return;
            }
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy().to_string();
                if let Some(pattern) = glob_pattern {
                    if !simple_glob_match(pattern, &rel_str) {
                        continue;
                    }
                }
                paths.push(rel_str);
            }
        }
    }
}

/// Very basic glob matching (only supports `*` and `**` patterns).
pub fn simple_glob_match(pattern: &str, path: &str) -> bool {
    // Use fnmatch-style matching
    let pattern = pattern.replace(".", r"\.");
    let pattern = pattern.replace("**", "DOUBLESTAR");
    let pattern = pattern.replace('*', "[^/]*");
    let pattern = pattern.replace("DOUBLESTAR", ".*");
    regex::Regex::new(&format!("^{}$", pattern))
        .map(|re| re.is_match(path))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_clip_short() {
        let text = "hello";
        assert_eq!(clip(text, 100), "hello");
    }

    #[test]
    fn test_clip_truncate() {
        let text = "hello world this is long";
        let result = clip(text, 10);
        assert!(result.contains("truncated"));
        assert!(result.starts_with("hello worl"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let root = Path::new("/workspace");
        let result = resolve_path("src/main.rs", root).unwrap();
        assert_eq!(result, PathBuf::from("/workspace/src/main.rs"));
    }

    #[test]
    fn test_resolve_path_absolute_inside() {
        let root = Path::new("/workspace");
        let result = resolve_path("/workspace/src/main.rs", root).unwrap();
        assert_eq!(result, PathBuf::from("/workspace/src/main.rs"));
    }

    #[test]
    fn test_resolve_path_escape() {
        let root = Path::new("/workspace");
        let result = resolve_path("../../etc/passwd", root);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_path_root_itself() {
        let root = Path::new("/workspace");
        let result = resolve_path("/workspace", root).unwrap();
        assert_eq!(result, PathBuf::from("/workspace"));
    }

    #[test]
    fn test_read_file_basic() {
        let dir = std::env::temp_dir().join("op_file_ops_test_read");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.txt"), "hello\nworld\n").unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = read_file("test.txt", true, &dir, 20000, &files_read);
        assert!(result.contains("# test.txt"));
        assert!(result.contains("hello"));
        assert!(result.contains("world"));
        // Should have hash prefixes
        assert!(result.contains("|hello"));
        assert!(files_read.lock().contains(&dir.join("test.txt")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_file_no_hash() {
        let dir = std::env::temp_dir().join("op_file_ops_test_read_nohash");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.txt"), "hello\n").unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = read_file("test.txt", false, &dir, 20000, &files_read);
        assert!(result.contains("1|hello"));
        // Should NOT have hash prefix
        assert!(!result.contains(":"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_file_not_found() {
        let dir = std::env::temp_dir().join("op_file_ops_test_notfound");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = read_file("nonexistent.txt", true, &dir, 20000, &files_read);
        assert!(result.contains("File not found"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_file_new() {
        let dir = std::env::temp_dir().join("op_file_ops_test_write_new");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = write_file("new.txt", "hello world", &dir, &files_read);
        assert!(result.contains("Wrote 11 chars"));
        assert_eq!(fs::read_to_string(dir.join("new.txt")).unwrap(), "hello world");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_file_blocked_unread() {
        let dir = std::env::temp_dir().join("op_file_ops_test_write_blocked");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("existing.txt"), "old content").unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = write_file("existing.txt", "new content", &dir, &files_read);
        assert!(result.contains("BLOCKED"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_file_allowed_after_read() {
        let dir = std::env::temp_dir().join("op_file_ops_test_write_allowed");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("existing.txt"), "old content").unwrap();

        let files_read = Mutex::new(HashSet::new());
        // First read it
        let _ = read_file("existing.txt", false, &dir, 20000, &files_read);
        // Now write should succeed
        let result = write_file("existing.txt", "new content", &dir, &files_read);
        assert!(result.contains("Wrote"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edit_file_exact_match() {
        let dir = std::env::temp_dir().join("op_file_ops_test_edit_exact");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.rs"), "fn main() {\n    old_code();\n}\n").unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = edit_file("test.rs", "old_code()", "new_code()", &dir, &files_read);
        assert!(result.contains("Edited"));
        let content = fs::read_to_string(dir.join("test.rs")).unwrap();
        assert!(content.contains("new_code()"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edit_file_multiple_matches() {
        let dir = std::env::temp_dir().join("op_file_ops_test_edit_multi");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.rs"), "foo\nfoo\n").unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = edit_file("test.rs", "foo", "bar", &dir, &files_read);
        assert!(result.contains("appears 2 times"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edit_file_fuzzy_match() {
        let dir = std::env::temp_dir().join("op_file_ops_test_edit_fuzzy");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.rs"), "  hello   world  \nfoo\n").unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = edit_file("test.rs", "hello world", "goodbye world", &dir, &files_read);
        assert!(result.contains("Edited"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edit_file_not_found() {
        let dir = std::env::temp_dir().join("op_file_ops_test_edit_notfound");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let files_read = Mutex::new(HashSet::new());
        let result = edit_file("missing.rs", "old", "new", &dir, &files_read);
        assert!(result.contains("File not found"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_image_not_found() {
        let root = std::env::temp_dir().join("op_file_ops_test_img_nf");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let (text, data) = read_image("missing.png", &root);
        assert!(text.contains("File not found"));
        assert!(data.is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_read_image_unsupported_format() {
        let root = std::env::temp_dir().join("op_file_ops_test_img_bad_ext");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("test.bmp"), b"BM").unwrap();

        let (text, data) = read_image("test.bmp", &root);
        assert!(text.contains("Unsupported image format"));
        assert!(data.is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_read_image_success() {
        let root = std::env::temp_dir().join("op_file_ops_test_img_ok");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // Write a tiny fake PNG (just the header bytes for testing)
        let fake_png = b"\x89PNG\r\n\x1a\n";
        fs::write(root.join("test.png"), fake_png).unwrap();

        let (text, data) = read_image("test.png", &root);
        assert!(text.contains("Image test.png"));
        assert!(text.contains("image/png"));
        let img = data.unwrap();
        assert_eq!(img.media_type, "image/png");
        assert!(!img.base64_data.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_simple_glob_match() {
        assert!(simple_glob_match("*.rs", "main.rs"));
        assert!(!simple_glob_match("*.rs", "src/main.rs"));
        assert!(simple_glob_match("**/*.rs", "src/main.rs"));
        assert!(simple_glob_match("src/*.rs", "src/main.rs"));
    }

    #[test]
    fn test_normalize_path() {
        let p = normalize_path(Path::new("/a/b/../c/./d"));
        assert_eq!(p, PathBuf::from("/a/c/d"));
    }

    #[test]
    fn test_list_files_basic() {
        let dir = std::env::temp_dir().join("op_file_ops_test_list");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "").unwrap();
        fs::write(dir.join("sub/b.txt"), "").unwrap();

        let result = list_files(&dir, None, 400, 45);
        assert!(result.contains("a.txt"));
        assert!(result.contains("b.txt"));

        let _ = fs::remove_dir_all(&dir);
    }
}
