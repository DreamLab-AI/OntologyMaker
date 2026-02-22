//! Shell execution: run_shell, run_shell_bg, check_shell_bg, kill_shell_bg, cleanup_bg_jobs.
//!
//! Uses `tokio::process::Command` for async execution with timeouts.
//! Background jobs are tracked with temp-file output and process handles.

use crate::file_ops::clip;
use crate::policy::check_shell_policy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// A background job entry.
#[derive(Debug)]
pub struct BgJob {
    pub child: tokio::process::Child,
    pub out_path: PathBuf,
    pub pid: u32,
}

/// Background job manager. Tracks running background processes.
#[derive(Debug, Default)]
pub struct BgJobManager {
    jobs: Mutex<HashMap<u64, BgJob>>,
    next_id: Mutex<u64>,
}

impl BgJobManager {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    fn next_job_id(&self) -> u64 {
        let mut id = self.next_id.lock();
        let current = *id;
        *id += 1;
        current
    }
}

/// Run a shell command synchronously (with async timeout).
pub async fn run_shell(
    command: &str,
    shell: &str,
    root: &Path,
    timeout_sec: Option<u64>,
    default_timeout: u64,
    max_output_chars: usize,
) -> String {
    // Policy check
    if let Some(msg) = check_shell_policy(command) {
        return msg;
    }

    let effective_timeout = {
        let t = timeout_sec.unwrap_or(default_timeout);
        t.max(1).min(600)
    };

    let child = Command::new(shell)
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        // start a new process group
        .process_group(0)
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(exc) => return format!("$ {}\n[failed to start: {}]", command, exc),
    };

    let timeout_duration = std::time::Duration::from_secs(effective_timeout);

    // Separate stdout/stderr from the child so we can still kill on timeout
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    match tokio::time::timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => {
            let out = if let Some(mut so) = stdout {
                let mut buf = Vec::new();
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut so, &mut buf).await;
                String::from_utf8_lossy(&buf).into_owned()
            } else {
                String::new()
            };
            let err = if let Some(mut se) = stderr {
                let mut buf = Vec::new();
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut se, &mut buf).await;
                String::from_utf8_lossy(&buf).into_owned()
            } else {
                String::new()
            };
            let code = status.code().unwrap_or(-1);
            let merged = format!(
                "$ {}\n[exit_code={}]\n[stdout]\n{}\n[stderr]\n{}",
                command, code, out, err
            );
            clip(&merged, max_output_chars)
        }
        Ok(Err(e)) => format!("$ {}\n[failed to wait: {}]", command, e),
        Err(_) => {
            // Timeout — kill the child process
            let _ = child.kill().await;
            format!(
                "$ {}\n[timeout after {}s — processes killed]",
                command, effective_timeout
            )
        }
    }
}

/// Start a shell command in the background.
pub async fn run_shell_bg(
    command: &str,
    shell: &str,
    root: &Path,
    manager: &BgJobManager,
) -> String {
    // Policy check
    if let Some(msg) = check_shell_policy(command) {
        return msg;
    }

    let job_id = manager.next_job_id();
    let out_path = std::env::temp_dir().join(format!(".rlm_bg_{}.out", job_id));

    let out_file = match std::fs::File::create(&out_path) {
        Ok(f) => f,
        Err(e) => return format!("Failed to start background command: {}", e),
    };
    let out_stdio: std::process::Stdio = out_file.into();

    let child = Command::new(shell)
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdout(out_stdio)
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false)
        .process_group(0)
        .spawn();

    let child = match child {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&out_path);
            return format!("Failed to start background command: {}", e);
        }
    };

    let pid = child.id().unwrap_or(0);
    manager.jobs.lock().insert(
        job_id,
        BgJob {
            child,
            out_path,
            pid,
        },
    );

    format!("Background job started: job_id={}, pid={}", job_id, pid)
}

/// Check the status and output of a background job.
pub async fn check_shell_bg(
    job_id: u64,
    manager: &BgJobManager,
    max_output_chars: usize,
) -> String {
    let mut jobs = manager.jobs.lock();
    let entry = match jobs.get_mut(&job_id) {
        Some(e) => e,
        None => return format!("No background job with id {}", job_id),
    };

    // Read output
    let output = std::fs::read_to_string(&entry.out_path).unwrap_or_default();
    let output = clip(&output, max_output_chars);

    // Check if process has exited
    match entry.child.try_wait() {
        Ok(Some(status)) => {
            let code = status.code().unwrap_or(-1);
            let out_path = entry.out_path.clone();
            jobs.remove(&job_id);
            let _ = std::fs::remove_file(&out_path);
            format!("[job {} finished, exit_code={}]\n{}", job_id, code, output)
        }
        Ok(None) => {
            format!(
                "[job {} still running, pid={}]\n{}",
                job_id, entry.pid, output
            )
        }
        Err(_) => {
            format!(
                "[job {} still running, pid={}]\n{}",
                job_id, entry.pid, output
            )
        }
    }
}

/// Kill a background job.
pub async fn kill_shell_bg(job_id: u64, manager: &BgJobManager) -> String {
    let mut jobs = manager.jobs.lock();
    let mut entry = match jobs.remove(&job_id) {
        Some(e) => e,
        None => return format!("No background job with id {}", job_id),
    };

    let _ = entry.child.kill().await;
    let _ = std::fs::remove_file(&entry.out_path);

    format!("Background job {} killed.", job_id)
}

/// Clean up all background jobs (e.g., on shutdown).
pub async fn cleanup_bg_jobs(manager: &BgJobManager) {
    let mut jobs = manager.jobs.lock();
    for (_, mut entry) in jobs.drain() {
        let _ = entry.child.kill().await;
        let _ = std::fs::remove_file(&entry.out_path);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_shell_echo() {
        let root = std::env::temp_dir();
        let result = run_shell("echo hello", "/bin/sh", &root, None, 10, 16000).await;
        assert!(result.contains("hello"));
        assert!(result.contains("exit_code=0"));
    }

    #[tokio::test]
    async fn test_run_shell_exit_code() {
        let root = std::env::temp_dir();
        let result = run_shell("exit 42", "/bin/sh", &root, None, 10, 16000).await;
        assert!(result.contains("exit_code=42"));
    }

    #[tokio::test]
    async fn test_run_shell_timeout() {
        let root = std::env::temp_dir();
        let result = run_shell("sleep 60", "/bin/sh", &root, Some(1), 10, 16000).await;
        assert!(result.contains("timeout"));
    }

    #[tokio::test]
    async fn test_run_shell_policy_blocked() {
        let root = std::env::temp_dir();
        let result = run_shell("vim file.txt", "/bin/sh", &root, None, 10, 16000).await;
        assert!(result.contains("BLOCKED"));
    }

    #[tokio::test]
    async fn test_run_shell_heredoc_blocked() {
        let root = std::env::temp_dir();
        let result = run_shell(
            "cat << EOF\nhello\nEOF",
            "/bin/sh",
            &root,
            None,
            10,
            16000,
        )
        .await;
        assert!(result.contains("BLOCKED"));
    }

    #[tokio::test]
    async fn test_run_shell_output_clipping() {
        let root = std::env::temp_dir();
        // Generate lots of output
        let result = run_shell(
            "seq 1 10000",
            "/bin/sh",
            &root,
            None,
            10,
            200, // very small limit
        )
        .await;
        assert!(result.contains("truncated"));
    }

    #[tokio::test]
    async fn test_bg_job_lifecycle() {
        let manager = BgJobManager::new();
        let root = std::env::temp_dir();

        // Start a background job
        let result = run_shell_bg("echo bg_output", "/bin/sh", &root, &manager).await;
        assert!(result.contains("job_id=1"));

        // Wait a moment for it to complete
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Check it
        let result = check_shell_bg(1, &manager, 16000).await;
        assert!(result.contains("finished") || result.contains("still running"));
    }

    #[tokio::test]
    async fn test_bg_job_not_found() {
        let manager = BgJobManager::new();
        let result = check_shell_bg(999, &manager, 16000).await;
        assert!(result.contains("No background job"));
    }

    #[tokio::test]
    async fn test_kill_bg_job() {
        let manager = BgJobManager::new();
        let root = std::env::temp_dir();

        let result = run_shell_bg("sleep 60", "/bin/sh", &root, &manager).await;
        assert!(result.contains("job_id=1"));

        let result = kill_shell_bg(1, &manager).await;
        assert!(result.contains("killed"));
    }

    #[tokio::test]
    async fn test_kill_bg_job_not_found() {
        let manager = BgJobManager::new();
        let result = kill_shell_bg(999, &manager).await;
        assert!(result.contains("No background job"));
    }

    #[tokio::test]
    async fn test_cleanup_bg_jobs() {
        let manager = BgJobManager::new();
        let root = std::env::temp_dir();

        let _ = run_shell_bg("sleep 60", "/bin/sh", &root, &manager).await;
        let _ = run_shell_bg("sleep 60", "/bin/sh", &root, &manager).await;

        cleanup_bg_jobs(&manager).await;
        assert!(manager.jobs.lock().is_empty());
    }

    #[tokio::test]
    async fn test_bg_job_policy_blocked() {
        let manager = BgJobManager::new();
        let root = std::env::temp_dir();

        let result = run_shell_bg("vim file.txt", "/bin/sh", &root, &manager).await;
        assert!(result.contains("BLOCKED"));
    }
}
