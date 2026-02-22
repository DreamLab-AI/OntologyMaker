//! Session storage layer: directory-based session CRUD, metadata, state, events, artifacts.
//!
//! Port of `agent/runtime.py` `SessionStore` dataclass.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use op_core::{OpError, OpResult};
use rand::Rng;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current UTC time as ISO-8601 string.
fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Generate a new session id: `YYYYMMDD-HHMMSS-{3 hex bytes}`.
fn new_session_id() -> String {
    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let hex: String = {
        let mut rng = rand::thread_rng();
        let bytes: [u8; 3] = rng.gen();
        hex::encode(bytes)
    };
    format!("{}-{}", stamp, hex)
}

/// Sanitise a string to be safe as a filesystem component.
/// Replaces runs of non-alphanumeric characters (except `.`, `_`, `-`) with `-`.
fn safe_component(text: &str) -> String {
    let re = Regex::new(r"[^A-Za-z0-9._-]+").expect("valid regex");
    let replaced = re.replace_all(text, "-");
    let trimmed = replaced.trim_matches('-');
    if trimmed.is_empty() {
        "artifact".to_string()
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------
// Session metadata / state structs
// ---------------------------------------------------------------------------

/// Metadata persisted in `metadata.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub workspace: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Minimal state persisted in `state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    #[serde(default)]
    pub external_observations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

impl SessionState {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            external_observations: Vec::new(),
            saved_at: None,
        }
    }
}

/// A single event written to `events.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub ts: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
}

/// Summary returned by `list_sessions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub path: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// SessionStore
// ---------------------------------------------------------------------------

/// Directory-based session store.
///
/// Layout:
/// ```text
/// {workspace}/{session_root_dir}/sessions/{session_id}/
///     metadata.json
///     state.json
///     events.jsonl
///     artifacts/{category}/{name}
/// ```
pub struct SessionStore {
    workspace: PathBuf,
    root: PathBuf,
    sessions: PathBuf,
}

impl SessionStore {
    /// Create a new store, ensuring the sessions directory exists.
    pub fn new(workspace: &Path, session_root_dir: &str) -> OpResult<Self> {
        let workspace = dunce::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let root = workspace.join(session_root_dir);
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions)?;
        Ok(Self {
            workspace,
            root,
            sessions,
        })
    }

    // -- path helpers -------------------------------------------------------

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions.join(session_id)
    }

    fn metadata_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("metadata.json")
    }

    fn state_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("state.json")
    }

    fn events_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("events.jsonl")
    }

    fn artifacts_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("artifacts")
    }

    // -- queries ------------------------------------------------------------

    /// Find the most-recently modified session directory.
    pub fn latest_session_id(&self) -> OpResult<Option<String>> {
        let mut best: Option<(String, std::time::SystemTime)> = None;
        for entry in fs::read_dir(&self.sessions)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let mtime = entry.metadata()?.modified()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if best.as_ref().map_or(true, |(_, t)| mtime > *t) {
                best = Some((name, mtime));
            }
        }
        Ok(best.map(|(id, _)| id))
    }

    /// List sessions sorted by mtime (most recent first), limited to `limit`.
    pub fn list_sessions(&self, limit: usize) -> OpResult<Vec<SessionSummary>> {
        let mut dirs: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        for entry in fs::read_dir(&self.sessions)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let mtime = entry.metadata()?.modified()?;
            dirs.push((entry.path(), mtime));
        }
        // Sort descending by mtime.
        dirs.sort_by(|a, b| b.1.cmp(&a.1));

        let mut out = Vec::new();
        for (path, _) in dirs.into_iter().take(limit) {
            let session_id = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let meta_path = path.join("metadata.json");
            let (created_at, updated_at) = if meta_path.exists() {
                match fs::read_to_string(&meta_path) {
                    Ok(data) => {
                        let v: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                        (
                            v.get("created_at").and_then(|v| v.as_str()).map(String::from),
                            v.get("updated_at").and_then(|v| v.as_str()).map(String::from),
                        )
                    }
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };
            out.push(SessionSummary {
                session_id,
                path: path.to_string_lossy().to_string(),
                created_at,
                updated_at,
            });
        }
        Ok(out)
    }

    // -- open / create ------------------------------------------------------

    /// Open (or create) a session.
    ///
    /// Returns `(session_id, state, created_new)`.
    pub fn open_session(
        &self,
        session_id: Option<&str>,
        resume: bool,
    ) -> OpResult<(String, SessionState, bool)> {
        let mut sid = session_id.map(String::from);

        if resume && sid.is_none() {
            sid = self.latest_session_id()?;
            if sid.is_none() {
                return Err(OpError::session("No previous sessions found to resume."));
            }
        }
        if sid.is_none() {
            sid = Some(new_session_id());
        }
        let mut sid = sid.unwrap();

        let mut created_new = false;
        if resume {
            if !self.session_dir(&sid).exists() {
                return Err(OpError::session(format!(
                    "Cannot resume missing session: {}",
                    sid
                )));
            }
        } else {
            if self.session_dir(&sid).exists() {
                let mut rng = rand::thread_rng();
                let extra: [u8; 2] = rng.gen();
                sid = format!("{}-{}", sid, hex::encode(extra));
            }
            fs::create_dir_all(self.session_dir(&sid))?;
            created_new = true;
        }

        // Ensure dirs exist.
        fs::create_dir_all(self.session_dir(&sid))?;
        fs::create_dir_all(self.artifacts_dir(&sid))?;

        // Write metadata if absent.
        let meta_path = self.metadata_path(&sid);
        if !meta_path.exists() {
            let meta = SessionMetadata {
                session_id: sid.clone(),
                workspace: self.workspace.to_string_lossy().to_string(),
                created_at: utc_now(),
                updated_at: utc_now(),
            };
            let data = serde_json::to_string_pretty(&meta)?;
            fs::write(&meta_path, data)?;
        }

        let state = self.load_state(&sid)?;
        Ok((sid, state, created_new))
    }

    // -- state --------------------------------------------------------------

    /// Load session state from `state.json`, returning defaults if absent.
    pub fn load_state(&self, session_id: &str) -> OpResult<SessionState> {
        let path = self.state_path(session_id);
        if !path.exists() {
            return Ok(SessionState::new(session_id));
        }
        let data = fs::read_to_string(&path)?;
        serde_json::from_str::<SessionState>(&data).map_err(|e| {
            OpError::session(format!(
                "Session state is invalid JSON: {}: {}",
                path.display(),
                e
            ))
        })
    }

    /// Persist session state to `state.json`.
    pub fn save_state(&self, session_id: &str, state: &SessionState) -> OpResult<()> {
        let path = self.state_path(session_id);
        let data = serde_json::to_string_pretty(state)?;
        fs::write(&path, data)?;
        self.touch_metadata(session_id)?;
        Ok(())
    }

    // -- events -------------------------------------------------------------

    /// Append a single event to the session `events.jsonl`.
    pub fn append_event(
        &self,
        session_id: &str,
        event_type: &str,
        payload: &Value,
    ) -> OpResult<()> {
        use std::io::Write;
        let event = SessionEvent {
            ts: utc_now(),
            event_type: event_type.to_string(),
            payload: payload.clone(),
        };
        let line = serde_json::to_string(&event)?;
        let path = self.events_path(session_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{}", line)?;
        self.touch_metadata(session_id)?;
        Ok(())
    }

    // -- artifacts ----------------------------------------------------------

    /// Write an artifact file. Returns the relative path within the session dir.
    pub fn write_artifact(
        &self,
        session_id: &str,
        category: &str,
        name: &str,
        content: &str,
    ) -> OpResult<String> {
        let category_safe = safe_component(category);
        let name_safe = safe_component(name);
        let rel = format!("artifacts/{}/{}", category_safe, name_safe);
        let abs = self.session_dir(session_id).join(&rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&abs, content)?;
        self.touch_metadata(session_id)?;
        Ok(rel)
    }

    // -- internal -----------------------------------------------------------

    /// Update `updated_at` in metadata.json.
    fn touch_metadata(&self, session_id: &str) -> OpResult<()> {
        let path = self.metadata_path(session_id);
        let mut base: Value = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(data) => serde_json::from_str(&data).unwrap_or(Value::Object(Default::default())),
                Err(_) => Value::Object(Default::default()),
            }
        } else {
            Value::Object(Default::default())
        };

        let obj = base.as_object_mut().unwrap();
        obj.insert("session_id".into(), Value::String(session_id.to_string()));
        obj.insert(
            "workspace".into(),
            Value::String(self.workspace.to_string_lossy().to_string()),
        );
        if !obj.contains_key("created_at") {
            obj.insert("created_at".into(), Value::String(utc_now()));
        }
        obj.insert("updated_at".into(), Value::String(utc_now()));

        let data = serde_json::to_string_pretty(&base)?;
        fs::write(&path, data)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_workspace(name: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("op_runtime_test")
            .join(name)
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn test_safe_component() {
        assert_eq!(safe_component("hello world!"), "hello-world");
        assert_eq!(safe_component("foo/bar"), "foo-bar");
        assert_eq!(safe_component("normal"), "normal");
        assert_eq!(safe_component("!!!"), "artifact");
        assert_eq!(safe_component("a.b-c_d"), "a.b-c_d");
    }

    #[test]
    fn test_new_session_id_format() {
        let id = new_session_id();
        // Expect pattern like 20260222-123456-a1b2c3
        assert!(
            id.len() >= 20,
            "session id too short: {}",
            id
        );
        let parts: Vec<&str> = id.splitn(3, '-').collect();
        assert_eq!(parts.len(), 3, "expected 3 parts in session id: {}", id);
        assert_eq!(parts[0].len(), 8, "date part wrong length");
        assert_eq!(parts[1].len(), 6, "time part wrong length");
        assert_eq!(parts[2].len(), 6, "hex part wrong length");
    }

    #[test]
    fn test_session_store_create_and_list() {
        let ws = temp_workspace("create_list");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        // No sessions yet.
        assert!(store.latest_session_id().unwrap().is_none());
        assert!(store.list_sessions(10).unwrap().is_empty());

        // Open a new session.
        let (sid, state, created) = store.open_session(None, false).unwrap();
        assert!(created);
        assert_eq!(state.session_id, sid);
        assert!(state.external_observations.is_empty());

        // Should appear in list now.
        let sessions = store.list_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, sid);
        assert!(sessions[0].created_at.is_some());

        // latest_session_id should match.
        assert_eq!(store.latest_session_id().unwrap(), Some(sid.clone()));

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_session_store_open_with_specific_id() {
        let ws = temp_workspace("specific_id");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        let (sid, _, created) = store.open_session(Some("my-test-session"), false).unwrap();
        assert!(created);
        assert_eq!(sid, "my-test-session");

        // Opening same id again should append random suffix.
        let (sid2, _, created2) = store.open_session(Some("my-test-session"), false).unwrap();
        assert!(created2);
        assert_ne!(sid2, "my-test-session");
        assert!(sid2.starts_with("my-test-session-"));

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_session_store_resume() {
        let ws = temp_workspace("resume");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        let (sid, _, _) = store.open_session(None, false).unwrap();

        // Resume with explicit id.
        let (sid2, _, created) = store.open_session(Some(&sid), true).unwrap();
        assert!(!created);
        assert_eq!(sid2, sid);

        // Resume latest.
        let (sid3, _, created3) = store.open_session(None, true).unwrap();
        assert!(!created3);
        assert_eq!(sid3, sid);

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_session_store_resume_error() {
        let ws = temp_workspace("resume_err");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        // No sessions to resume.
        let result = store.open_session(None, true);
        assert!(result.is_err());

        // Missing session id.
        let result = store.open_session(Some("nonexistent"), true);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_state_save_load() {
        let ws = temp_workspace("state");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        let (sid, _, _) = store.open_session(None, false).unwrap();

        let mut state = SessionState::new(&sid);
        state.external_observations = vec!["obs1".into(), "obs2".into()];
        state.saved_at = Some(utc_now());
        store.save_state(&sid, &state).unwrap();

        let loaded = store.load_state(&sid).unwrap();
        assert_eq!(loaded.external_observations.len(), 2);
        assert_eq!(loaded.external_observations[0], "obs1");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_append_event() {
        let ws = temp_workspace("events");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        let (sid, _, _) = store.open_session(None, false).unwrap();

        store
            .append_event(&sid, "test_event", &serde_json::json!({"key": "value"}))
            .unwrap();
        store
            .append_event(&sid, "another", &serde_json::json!({"n": 42}))
            .unwrap();

        let events_path = store.events_path(&sid);
        let content = fs::read_to_string(&events_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let ev0: SessionEvent = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(ev0.event_type, "test_event");

        let ev1: SessionEvent = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ev1.event_type, "another");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_write_artifact() {
        let ws = temp_workspace("artifacts");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        let (sid, _, _) = store.open_session(None, false).unwrap();

        let rel = store
            .write_artifact(&sid, "patches", "fix-001.patch", "patch content here")
            .unwrap();
        assert_eq!(rel, "artifacts/patches/fix-001.patch");

        let abs = store.session_dir(&sid).join(&rel);
        assert!(abs.exists());
        assert_eq!(fs::read_to_string(&abs).unwrap(), "patch content here");

        // Category with special chars.
        let rel2 = store
            .write_artifact(&sid, "my category!", "my file!.txt", "data")
            .unwrap();
        assert_eq!(rel2, "artifacts/my-category/my-file-.txt");

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_touch_metadata_updates() {
        let ws = temp_workspace("touch_meta");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        let (sid, _, _) = store.open_session(None, false).unwrap();

        let meta_path = store.metadata_path(&sid);
        let data1 = fs::read_to_string(&meta_path).unwrap();
        let v1: Value = serde_json::from_str(&data1).unwrap();
        let created1 = v1["created_at"].as_str().unwrap().to_string();

        // Touch via an event.
        std::thread::sleep(std::time::Duration::from_millis(10));
        store
            .append_event(&sid, "test", &serde_json::json!({}))
            .unwrap();

        let data2 = fs::read_to_string(&meta_path).unwrap();
        let v2: Value = serde_json::from_str(&data2).unwrap();
        // created_at should be unchanged.
        assert_eq!(v2["created_at"].as_str().unwrap(), created1);
        // updated_at should be different.
        assert_ne!(
            v2["updated_at"].as_str().unwrap(),
            v1["updated_at"].as_str().unwrap()
        );

        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn test_list_sessions_limit() {
        let ws = temp_workspace("list_limit");
        let store = SessionStore::new(&ws, ".openplanter").unwrap();

        for _ in 0..5 {
            store.open_session(None, false).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }

        let all = store.list_sessions(100).unwrap();
        assert_eq!(all.len(), 5);

        let limited = store.list_sessions(2).unwrap();
        assert_eq!(limited.len(), 2);

        let _ = fs::remove_dir_all(&ws);
    }
}
