//! Replay-capable LLM interaction logging with delta encoding.
//!
//! Port of `agent/replay_log.py` `ReplayLogger`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use op_core::OpResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Record types
// ---------------------------------------------------------------------------

/// Header record written once per conversation to capture configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub conversation_id: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub system_prompt: String,
    pub tool_defs: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

/// Call record: captures every LLM API call with delta-encoded messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub conversation_id: String,
    pub seq: u64,
    pub depth: u32,
    pub step: u32,
    pub ts: String,
    /// Full messages snapshot (only on seq 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages_snapshot: Option<Vec<Value>>,
    /// Delta messages since last call (seq > 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages_delta: Option<Vec<Value>>,
    pub response: Value,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub elapsed_sec: f64,
}

// ---------------------------------------------------------------------------
// ReplayLogger
// ---------------------------------------------------------------------------

/// Logs every LLM API call so any individual call can be replayed exactly.
///
/// Uses delta encoding: seq 0 stores a full messages snapshot, seq 1+
/// store only messages appended since the previous call.
///
/// Each conversation (root + subtasks) gets its own `conversation_id`.
/// All records append to the same JSONL file in chronological order.
pub struct ReplayLogger {
    path: PathBuf,
    conversation_id: String,
    seq: u64,
    last_msg_count: usize,
}

impl ReplayLogger {
    /// Create a new root-level replay logger.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            conversation_id: "root".to_string(),
            seq: 0,
            last_msg_count: 0,
        }
    }

    /// Create a replay logger with an explicit conversation id.
    pub fn with_conversation_id(path: PathBuf, conversation_id: String) -> Self {
        Self {
            path,
            conversation_id,
            seq: 0,
            last_msg_count: 0,
        }
    }

    /// Create a child logger for a subtask conversation.
    pub fn child(&self, depth: u32, step: u32) -> Self {
        let child_id = format!("{}/d{}s{}", self.conversation_id, depth, step);
        Self {
            path: self.path.clone(),
            conversation_id: child_id,
            seq: 0,
            last_msg_count: 0,
        }
    }

    /// Path accessor (useful for session_runtime).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current conversation id.
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    /// Write a header record capturing LLM configuration for this conversation.
    pub fn write_header(
        &self,
        provider: &str,
        model: &str,
        base_url: &str,
        system_prompt: &str,
        tool_defs: Vec<Value>,
        reasoning_effort: Option<&str>,
        temperature: Option<f64>,
    ) -> OpResult<()> {
        let record = HeaderRecord {
            record_type: "header".to_string(),
            conversation_id: self.conversation_id.clone(),
            provider: provider.to_string(),
            model: model.to_string(),
            base_url: base_url.to_string(),
            system_prompt: system_prompt.to_string(),
            tool_defs,
            reasoning_effort: reasoning_effort.map(String::from),
            temperature,
        };
        let value = serde_json::to_value(&record)?;
        self.append(&value)
    }

    /// Log an LLM API call with delta-encoded messages.
    pub fn log_call(
        &mut self,
        depth: u32,
        step: u32,
        messages: &[Value],
        response: &Value,
        input_tokens: u64,
        output_tokens: u64,
        elapsed_sec: f64,
    ) -> OpResult<()> {
        let (snapshot, delta) = if self.seq == 0 {
            (Some(messages.to_vec()), None)
        } else {
            let new_msgs = if self.last_msg_count < messages.len() {
                messages[self.last_msg_count..].to_vec()
            } else {
                Vec::new()
            };
            (None, Some(new_msgs))
        };

        let record = CallRecord {
            record_type: "call".to_string(),
            conversation_id: self.conversation_id.clone(),
            seq: self.seq,
            depth,
            step,
            ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            messages_snapshot: snapshot,
            messages_delta: delta,
            response: response.clone(),
            input_tokens,
            output_tokens,
            elapsed_sec: (elapsed_sec * 1000.0).round() / 1000.0,
        };

        self.last_msg_count = messages.len();
        self.seq += 1;

        let value = serde_json::to_value(&record)?;
        self.append(&value)
    }

    /// Append a JSON record as a single line to the JSONL file.
    fn append(&self, record: &Value) -> OpResult<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(record)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_path(name: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("op_replay_test")
            .join(name)
            .join(format!("{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p.join("replay.jsonl")
    }

    #[test]
    fn test_child_conversation_id() {
        let logger = ReplayLogger::new(PathBuf::from("/tmp/test.jsonl"));
        let child = logger.child(1, 2);
        assert_eq!(child.conversation_id(), "root/d1s2");

        let grandchild = child.child(2, 0);
        assert_eq!(grandchild.conversation_id(), "root/d1s2/d2s0");
    }

    #[test]
    fn test_write_header() {
        let path = temp_path("header");
        let logger = ReplayLogger::new(path.clone());
        logger
            .write_header(
                "openai",
                "gpt-4",
                "https://api.openai.com/v1",
                "You are a helpful assistant.",
                vec![json!({"name": "tool1"})],
                Some("high"),
                Some(0.7),
            )
            .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);

        let record: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(record["type"], "header");
        assert_eq!(record["conversation_id"], "root");
        assert_eq!(record["provider"], "openai");
        assert_eq!(record["model"], "gpt-4");
        assert_eq!(record["reasoning_effort"], "high");
        assert_eq!(record["temperature"], 0.7);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_delta_encoding() {
        let path = temp_path("delta");
        let mut logger = ReplayLogger::new(path.clone());

        // First call: full snapshot.
        let msgs1 = vec![json!({"role": "user", "content": "hello"})];
        logger
            .log_call(0, 0, &msgs1, &json!({"response": "hi"}), 10, 5, 1.234)
            .unwrap();

        // Second call: delta only.
        let msgs2 = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
            json!({"role": "user", "content": "how are you?"}),
        ];
        logger
            .log_call(0, 1, &msgs2, &json!({"response": "good"}), 20, 10, 0.5)
            .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // First record has snapshot.
        let r0: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(r0["seq"], 0);
        assert!(r0.get("messages_snapshot").is_some());
        assert!(r0.get("messages_delta").is_none());
        let snapshot = r0["messages_snapshot"].as_array().unwrap();
        assert_eq!(snapshot.len(), 1);

        // Second record has delta.
        let r1: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r1["seq"], 1);
        assert!(r1.get("messages_snapshot").is_none());
        assert!(r1.get("messages_delta").is_some());
        let delta = r1["messages_delta"].as_array().unwrap();
        assert_eq!(delta.len(), 2); // 2 new messages since last call

        // elapsed_sec should be rounded.
        assert_eq!(r0["elapsed_sec"], 1.234);
        assert_eq!(r1["elapsed_sec"], 0.5);

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_multiple_conversations_in_same_file() {
        let path = temp_path("multi_conv");
        let mut root = ReplayLogger::new(path.clone());
        let mut child = root.child(1, 0);

        root.log_call(0, 0, &[json!("msg1")], &json!("resp1"), 1, 1, 0.1)
            .unwrap();
        child
            .log_call(1, 0, &[json!("child_msg")], &json!("child_resp"), 2, 2, 0.2)
            .unwrap();
        root.log_call(0, 1, &[json!("msg1"), json!("msg2")], &json!("resp2"), 3, 3, 0.3)
            .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);

        let r0: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(r0["conversation_id"], "root");

        let r1: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(r1["conversation_id"], "root/d1s0");

        let r2: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(r2["conversation_id"], "root");
        // r2 should be a delta (seq 1).
        assert_eq!(r2["seq"], 1);
        assert!(r2.get("messages_delta").is_some());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_header_without_optional_fields() {
        let path = temp_path("header_minimal");
        let logger = ReplayLogger::new(path.clone());
        logger
            .write_header(
                "anthropic",
                "claude-3",
                "https://api.anthropic.com/v1",
                "System prompt",
                vec![],
                None,
                None,
            )
            .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let record: Value = serde_json::from_str(content.trim()).unwrap();
        assert!(record.get("reasoning_effort").is_none());
        assert!(record.get("temperature").is_none());

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
