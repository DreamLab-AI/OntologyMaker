//! Wiki seeding: copies baseline `wiki/` into the runtime `.openplanter/wiki/` directory.
//!
//! Port of `agent/runtime.py` `_seed_wiki`.

use std::fs;
use std::path::Path;

use tracing::debug;

/// Copy baseline `wiki/` into the runtime `.openplanter/wiki/` directory.
///
/// On first run, copies the entire tree. On subsequent runs, copies only
/// new baseline files -- never overwrites agent-modified entries.
///
/// Ignores hidden files/dirs (starting with `.`) and `__pycache__` directories.
pub fn seed_wiki(workspace: &Path, session_root_dir: &str) {
    if let Err(e) = seed_wiki_inner(workspace, session_root_dir) {
        debug!("wiki seeding failed (non-fatal): {}", e);
    }
}

fn seed_wiki_inner(workspace: &Path, session_root_dir: &str) -> Result<(), std::io::Error> {
    let baseline = workspace.join("wiki");
    if !baseline.is_dir() {
        return Ok(());
    }
    let runtime_wiki = workspace.join(session_root_dir).join("wiki");

    if !runtime_wiki.exists() {
        // First run: copy entire tree (excluding hidden and __pycache__).
        copy_tree_filtered(&baseline, &runtime_wiki)?;
        return Ok(());
    }

    // Incremental: copy only new baseline files.
    copy_new_files(&baseline, &baseline, &runtime_wiki)?;
    Ok(())
}

/// Recursively copy a directory tree, skipping hidden entries and `__pycache__`.
fn copy_tree_filtered(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "__pycache__" {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if src_path.is_dir() {
            copy_tree_filtered(&src_path, &dst_path)?;
        } else if src_path.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Walk `src_root` recursively, copying files to `dst_root` only if they don't
/// already exist at the destination. Skips hidden dirs and `__pycache__`.
fn copy_new_files(
    current: &Path,
    src_root: &Path,
    dst_root: &Path,
) -> Result<(), std::io::Error> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "__pycache__" {
            continue;
        }
        let src_path = entry.path();
        if src_path.is_dir() {
            copy_new_files(&src_path, src_root, dst_root)?;
        } else if src_path.is_file() {
            let rel = src_path
                .strip_prefix(src_root)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            let dst_path = dst_root.join(rel);
            if !dst_path.exists() {
                if let Some(parent) = dst_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&src_path, &dst_path)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir()
            .join("op_wiki_test")
            .join(name)
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn test_seed_wiki_no_baseline() {
        let ws = temp_dir("no_baseline");
        // No wiki/ dir exists -- should be a no-op.
        seed_wiki(&ws, ".openplanter");
        assert!(!ws.join(".openplanter").join("wiki").exists());

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_seed_wiki_first_run() {
        let ws = temp_dir("first_run");
        let wiki = ws.join("wiki");
        fs::create_dir_all(wiki.join("sub")).unwrap();
        fs::write(wiki.join("page.md"), "# Page").unwrap();
        fs::write(wiki.join("sub").join("nested.md"), "# Nested").unwrap();
        // Hidden file should be skipped.
        fs::write(wiki.join(".hidden"), "secret").unwrap();
        // __pycache__ should be skipped.
        fs::create_dir_all(wiki.join("__pycache__")).unwrap();
        fs::write(wiki.join("__pycache__").join("cache.pyc"), "bytecode").unwrap();

        seed_wiki(&ws, ".openplanter");

        let runtime_wiki = ws.join(".openplanter").join("wiki");
        assert!(runtime_wiki.join("page.md").exists());
        assert!(runtime_wiki.join("sub").join("nested.md").exists());
        assert!(!runtime_wiki.join(".hidden").exists());
        assert!(!runtime_wiki.join("__pycache__").exists());

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_seed_wiki_incremental() {
        let ws = temp_dir("incremental");
        let wiki = ws.join("wiki");
        fs::create_dir_all(&wiki).unwrap();
        fs::write(wiki.join("page.md"), "# Original").unwrap();

        // First seed.
        seed_wiki(&ws, ".openplanter");

        let runtime_wiki = ws.join(".openplanter").join("wiki");
        assert_eq!(
            fs::read_to_string(runtime_wiki.join("page.md")).unwrap(),
            "# Original"
        );

        // Modify the runtime version (simulating agent edit).
        fs::write(runtime_wiki.join("page.md"), "# Modified by agent").unwrap();

        // Add a new baseline file.
        fs::write(wiki.join("new_page.md"), "# New Page").unwrap();

        // Second seed: should NOT overwrite page.md, SHOULD copy new_page.md.
        seed_wiki(&ws, ".openplanter");

        assert_eq!(
            fs::read_to_string(runtime_wiki.join("page.md")).unwrap(),
            "# Modified by agent"
        );
        assert_eq!(
            fs::read_to_string(runtime_wiki.join("new_page.md")).unwrap(),
            "# New Page"
        );

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_seed_wiki_incremental_nested() {
        let ws = temp_dir("incr_nested");
        let wiki = ws.join("wiki");
        fs::create_dir_all(wiki.join("sub")).unwrap();
        fs::write(wiki.join("sub").join("a.md"), "A").unwrap();

        seed_wiki(&ws, ".openplanter");

        // Add new nested file.
        fs::write(wiki.join("sub").join("b.md"), "B").unwrap();

        seed_wiki(&ws, ".openplanter");

        let runtime_wiki = ws.join(".openplanter").join("wiki");
        assert!(runtime_wiki.join("sub").join("a.md").exists());
        assert!(runtime_wiki.join("sub").join("b.md").exists());

        let _ = fs::remove_dir_all(&ws);
    }
}
