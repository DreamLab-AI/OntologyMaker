//! Search operations: search_files, repo_map, symbol extraction.
//!
//! Ports the search/repo_map methods from Python `WorkspaceTools`.

use crate::file_ops::{clip, resolve_path};
use regex::Regex;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

const MAX_WALK_ENTRIES: usize = 50_000;

/// Language file extension mapping.
static LANGUAGE_BY_SUFFIX: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert(".py", "python");
    m.insert(".js", "javascript");
    m.insert(".jsx", "javascript");
    m.insert(".ts", "typescript");
    m.insert(".tsx", "typescript");
    m.insert(".go", "go");
    m.insert(".rs", "rust");
    m.insert(".java", "java");
    m.insert(".c", "c");
    m.insert(".h", "c");
    m.insert(".cpp", "cpp");
    m.insert(".hpp", "cpp");
    m.insert(".cs", "csharp");
    m.insert(".rb", "ruby");
    m.insert(".php", "php");
    m.insert(".swift", "swift");
    m.insert(".kt", "kotlin");
    m.insert(".scala", "scala");
    m.insert(".sh", "shell");
    m
});

// Generic symbol extraction regexes.
static FUNCTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*function\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("FUNCTION_RE")
});
static CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)\b").expect("CLASS_RE")
});
static CONST_FN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:const|let|var)\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*\(")
        .expect("CONST_FN_RE")
});

// Python-specific regexes for symbol extraction.
static PY_FUNCTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("PY_FUNCTION_RE")
});
static PY_CLASS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)\b").expect("PY_CLASS_RE")
});
static PY_METHOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^    (?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(")
        .expect("PY_METHOD_RE")
});

/// A symbol extracted from source code.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Symbol {
    pub kind: String,
    pub name: String,
    pub line: usize,
}

/// Search file contents using ripgrep (with os.walk fallback).
pub fn search_files(
    query: &str,
    glob: Option<&str>,
    root: &Path,
    max_search_hits: usize,
    command_timeout_sec: u64,
) -> String {
    if query.trim().is_empty() {
        return "query cannot be empty".to_string();
    }

    if which_rg() {
        let mut cmd = Command::new("rg");
        cmd.args(["-n", "--hidden", "-S", query, "."]);
        if let Some(g) = glob {
            cmd.args(["-g", g]);
        }
        cmd.current_dir(root);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = match cmd.output() {
            Ok(o) => o,
            Err(_) => return "(search_files timed out)".to_string(),
        };
        let _ = command_timeout_sec; // Timeout handled at higher level for async

        let stdout = String::from_utf8_lossy(&output.stdout);
        let out_lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();

        if out_lines.is_empty() {
            return "(no matches)".to_string();
        }

        let clipped: Vec<&&str> = out_lines.iter().take(max_search_hits).collect();
        let mut result: String = clipped.iter().map(|s| **s).collect::<Vec<_>>().join("\n");
        if out_lines.len() > clipped.len() {
            result.push_str(&format!(
                "\n...[omitted {} matches]...",
                out_lines.len() - clipped.len()
            ));
        }
        result
    } else {
        // Fallback: walk filesystem and search
        search_files_fallback(query, root, max_search_hits)
    }
}

/// Fallback search when ripgrep is unavailable.
fn search_files_fallback(query: &str, root: &Path, max_hits: usize) -> String {
    let lower_query = query.to_lowercase();
    let mut matches: Vec<String> = Vec::new();
    let mut count = 0;

    walk_search(root, root, &lower_query, &mut matches, &mut count, max_hits);

    if matches.is_empty() {
        "(no matches)".to_string()
    } else {
        matches.join("\n")
    }
}

fn walk_search(
    current: &Path,
    root: &Path,
    lower_query: &str,
    matches: &mut Vec<String>,
    count: &mut usize,
    max_hits: usize,
) {
    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if matches.len() >= max_hits {
            return;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git" {
            continue;
        }
        if path.is_dir() {
            walk_search(&path, root, lower_query, matches, count, max_hits);
        } else {
            *count += 1;
            if *count > MAX_WALK_ENTRIES {
                return;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            for (idx, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(lower_query) {
                    matches.push(format!("{}:{}:{}", rel, idx + 1, line));
                    if matches.len() >= max_hits {
                        matches.push("...[match limit reached]...".to_string());
                        return;
                    }
                }
            }
        }
    }
}

/// Get a list of files in the workspace (for repo_map).
fn repo_files(
    root: &Path,
    glob: Option<&str>,
    max_files: usize,
    command_timeout_sec: u64,
) -> Vec<String> {
    let lines = if which_rg() {
        let mut cmd = Command::new("rg");
        cmd.args(["--files", "--hidden", "-g", "!.git"]);
        if let Some(g) = glob {
            cmd.args(["-g", g]);
        }
        cmd.current_dir(root);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|s| s.to_string())
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    } else {
        let mut paths = Vec::new();
        let mut count = 0;
        walk_for_files(root, root, glob, &mut paths, &mut count, MAX_WALK_ENTRIES);
        paths
    };
    let _ = command_timeout_sec;
    lines.into_iter().take(max_files).collect()
}

fn walk_for_files(
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
            walk_for_files(&path, root, glob_pattern, paths, count, max_entries);
        } else {
            *count += 1;
            if *count > max_entries {
                return;
            }
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy().to_string();
                if let Some(pattern) = glob_pattern {
                    if !crate::file_ops::simple_glob_match(pattern, &rel_str) {
                        continue;
                    }
                }
                paths.push(rel_str);
            }
        }
    }
}

/// Extract symbols from Python source code.
fn python_symbols(text: &str) -> Vec<Symbol> {
    let mut symbols: Vec<Symbol> = Vec::new();

    // Find top-level classes
    let mut class_ranges: Vec<(String, usize)> = Vec::new();
    for cap in PY_CLASS_RE.captures_iter(text) {
        let name = cap[1].to_string();
        let line = text[..cap.get(0).unwrap().start()].lines().count() + 1;
        class_ranges.push((name.clone(), line));
        symbols.push(Symbol {
            kind: "class".to_string(),
            name,
            line,
        });
    }

    // Find top-level functions (not indented)
    for cap in PY_FUNCTION_RE.captures_iter(text) {
        let name = cap[1].to_string();
        let pos = cap.get(0).unwrap().start();
        let line = text[..pos].lines().count() + 1;
        // Check if this is at the start of a line (not indented)
        let line_start = text[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
        if pos == line_start {
            symbols.push(Symbol {
                kind: "function".to_string(),
                name,
                line,
            });
        }
    }

    // Find methods (indented with 4 spaces — inside classes)
    for cap in PY_METHOD_RE.captures_iter(text) {
        let method_name = cap[1].to_string();
        let pos = cap.get(0).unwrap().start();
        let line = text[..pos].lines().count() + 1;

        // Find the most recent class that started before this method
        let mut parent_class: Option<&str> = None;
        for (class_name, class_line) in class_ranges.iter().rev() {
            if *class_line < line {
                parent_class = Some(class_name);
                break;
            }
        }

        if let Some(cls) = parent_class {
            symbols.push(Symbol {
                kind: "method".to_string(),
                name: format!("{}.{}", cls, method_name),
                line,
            });
        }
    }

    symbols.sort_by_key(|s| s.line);
    symbols
}

/// Extract symbols from source code using generic regex patterns.
fn generic_symbols(text: &str) -> Vec<Symbol> {
    let mut symbols: Vec<Symbol> = Vec::new();

    for cap in FUNCTION_RE.captures_iter(text) {
        let name = cap[1].to_string();
        let line = text[..cap.get(0).unwrap().start()].lines().count() + 1;
        symbols.push(Symbol {
            kind: "function".to_string(),
            name,
            line,
        });
    }

    for cap in CLASS_RE.captures_iter(text) {
        let name = cap[1].to_string();
        let line = text[..cap.get(0).unwrap().start()].lines().count() + 1;
        symbols.push(Symbol {
            kind: "class".to_string(),
            name,
            line,
        });
    }

    for cap in CONST_FN_RE.captures_iter(text) {
        let name = cap[1].to_string();
        let line = text[..cap.get(0).unwrap().start()].lines().count() + 1;
        symbols.push(Symbol {
            kind: "function".to_string(),
            name,
            line,
        });
    }

    symbols.sort_by_key(|s| s.line);
    symbols
}

/// Build a lightweight map of source files and symbols.
pub fn repo_map(
    root: &Path,
    glob: Option<&str>,
    max_files: usize,
    max_file_chars: usize,
    command_timeout_sec: u64,
) -> String {
    let clamped = max_files.max(1).min(500);
    let candidates = repo_files(root, glob, clamped, command_timeout_sec);

    if candidates.is_empty() {
        return "(no files)".to_string();
    }

    let mut files: Vec<serde_json::Value> = Vec::new();

    for rel in &candidates {
        let suffix = Path::new(rel)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        let language = match LANGUAGE_BY_SUFFIX.get(suffix.as_str()) {
            Some(lang) => *lang,
            None => continue,
        };

        let resolved = match resolve_path(rel, root) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if !resolved.exists() || resolved.is_dir() {
            continue;
        }
        let text = match std::fs::read_to_string(&resolved) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let symbols: Vec<Symbol> = if language == "python" {
            python_symbols(&text)
        } else {
            generic_symbols(&text)
        };

        let symbols_json: Vec<serde_json::Value> = symbols
            .iter()
            .take(200)
            .map(|s| {
                serde_json::json!({
                    "kind": s.kind,
                    "name": s.name,
                    "line": s.line,
                })
            })
            .collect();

        files.push(serde_json::json!({
            "path": rel,
            "language": language,
            "lines": text.lines().count(),
            "symbols": symbols_json,
        }));
    }

    let output = serde_json::json!({
        "root": root.to_string_lossy(),
        "files": files,
        "total": files.len(),
    });

    let json_str = serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string());
    clip(&json_str, max_file_chars)
}

/// Check if `rg` (ripgrep) is on PATH.
fn which_rg() -> bool {
    Command::new("which")
        .arg("rg")
        .output()
        .map(|o| o.status.success())
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
    fn test_python_symbols_function() {
        let code = "def hello():\n    pass\n";
        let syms = python_symbols(code);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].kind, "function");
        assert_eq!(syms[0].name, "hello");
        assert_eq!(syms[0].line, 1);
    }

    #[test]
    fn test_python_symbols_class_and_method() {
        let code = "class Foo:\n    def bar(self):\n        pass\n";
        let syms = python_symbols(code);
        // Should find class Foo and method Foo.bar
        let class_sym = syms.iter().find(|s| s.kind == "class").unwrap();
        assert_eq!(class_sym.name, "Foo");
        let method_sym = syms.iter().find(|s| s.kind == "method").unwrap();
        assert_eq!(method_sym.name, "Foo.bar");
    }

    #[test]
    fn test_python_symbols_async_function() {
        let code = "async def fetch():\n    pass\n";
        let syms = python_symbols(code);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].kind, "function");
        assert_eq!(syms[0].name, "fetch");
    }

    #[test]
    fn test_generic_symbols_function() {
        let code = "function hello() {\n  return 1;\n}\n";
        let syms = generic_symbols(code);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].kind, "function");
        assert_eq!(syms[0].name, "hello");
    }

    #[test]
    fn test_generic_symbols_class() {
        let code = "class MyClass {\n  constructor() {}\n}\n";
        let syms = generic_symbols(code);
        let class_sym = syms.iter().find(|s| s.kind == "class").unwrap();
        assert_eq!(class_sym.name, "MyClass");
    }

    #[test]
    fn test_generic_symbols_const_fn() {
        let code = "const handler = (req, res) => {};\n";
        let syms = generic_symbols(code);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].kind, "function");
        assert_eq!(syms[0].name, "handler");
    }

    #[test]
    fn test_search_files_empty_query() {
        let root = std::env::temp_dir().join("op_search_test_empty");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let result = search_files("", None, &root, 200, 45);
        assert_eq!(result, "query cannot be empty");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_search_files_no_match() {
        let root = std::env::temp_dir().join("op_search_test_nomatch");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("test.txt"), "hello world").unwrap();

        let result = search_files("xyz_not_found_xyz", None, &root, 200, 45);
        assert!(result.contains("no matches"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_search_files_found() {
        let root = std::env::temp_dir().join("op_search_test_found");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("test.txt"), "hello world\nfoo bar\n").unwrap();

        let result = search_files("hello", None, &root, 200, 45);
        assert!(result.contains("hello"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_repo_map_basic() {
        let root = std::env::temp_dir().join("op_search_test_repomap");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("main.py"), "def main():\n    pass\n").unwrap();
        fs::write(root.join("readme.txt"), "not a code file").unwrap();

        let result = repo_map(&root, None, 200, 20000, 45);
        assert!(result.contains("main.py"));
        assert!(result.contains("python"));
        // readme.txt has no recognized extension, so it shouldn't appear
        assert!(!result.contains("readme.txt"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_repo_map_empty() {
        let root = std::env::temp_dir().join("op_search_test_repomap_empty");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let result = repo_map(&root, None, 200, 20000, 45);
        // Either "(no files)" if rg returns nothing, or a JSON with no files
        assert!(result.contains("no files") || result.contains("\"total\": 0"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_language_by_suffix() {
        assert_eq!(LANGUAGE_BY_SUFFIX.get(".py"), Some(&"python"));
        assert_eq!(LANGUAGE_BY_SUFFIX.get(".rs"), Some(&"rust"));
        assert_eq!(LANGUAGE_BY_SUFFIX.get(".js"), Some(&"javascript"));
        assert_eq!(LANGUAGE_BY_SUFFIX.get(".unknown"), None);
    }
}
