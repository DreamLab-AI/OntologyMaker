use serde::{Deserialize, Serialize};

/// A single tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Base64-encoded image payload for vision-capable models.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageData {
    pub base64_data: String,
    pub media_type: String,
}

/// Result of executing a tool call.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    pub image: Option<ImageData>,
}

impl ToolResult {
    pub fn ok(tool_call_id: String, name: String, content: String) -> Self {
        Self {
            tool_call_id,
            name,
            content,
            is_error: false,
            image: None,
        }
    }

    pub fn err(tool_call_id: String, name: String, content: String) -> Self {
        Self {
            tool_call_id,
            name,
            content,
            is_error: true,
            image: None,
        }
    }
}

/// One assistant turn from the model.
#[derive(Debug, Clone)]
pub struct ModelTurn {
    pub tool_calls: Vec<ToolCall>,
    pub text: Option<String>,
    pub stop_reason: String,
    pub raw_response: serde_json::Value,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Default for ModelTurn {
    fn default() -> Self {
        Self {
            tool_calls: Vec::new(),
            text: None,
            stop_reason: String::new(),
            raw_response: serde_json::Value::Null,
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

/// Opaque conversation state owned by the model layer.
#[derive(Debug)]
pub struct Conversation {
    pub provider_messages: Vec<serde_json::Value>,
    pub system_prompt: String,
    pub turn_count: u32,
    pub stop_sequences: Vec<String>,
}

impl Conversation {
    pub fn new(system_prompt: String) -> Self {
        Self {
            provider_messages: Vec::new(),
            system_prompt,
            turn_count: 0,
            stop_sequences: Vec::new(),
        }
    }

    pub fn get_messages(&self) -> Vec<serde_json::Value> {
        self.provider_messages.clone()
    }
}

/// Token usage tracking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_call_serialize() {
        let tc = ToolCall {
            id: "tc_1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        };
        let json = serde_json::to_string(&tc).unwrap();
        assert!(json.contains("read_file"));
    }

    #[test]
    fn test_tool_result_ok_and_err() {
        let ok = ToolResult::ok("id1".into(), "read_file".into(), "content".into());
        assert!(!ok.is_error);
        assert!(ok.image.is_none());

        let err = ToolResult::err("id2".into(), "run_shell".into(), "failed".into());
        assert!(err.is_error);
    }

    #[test]
    fn test_model_turn_default() {
        let turn = ModelTurn::default();
        assert!(turn.tool_calls.is_empty());
        assert!(turn.text.is_none());
        assert_eq!(turn.input_tokens, 0);
    }

    #[test]
    fn test_conversation_new() {
        let conv = Conversation::new("system prompt".into());
        assert!(conv.provider_messages.is_empty());
        assert_eq!(conv.turn_count, 0);
        assert_eq!(conv.system_prompt, "system prompt");
    }

    #[test]
    fn test_token_usage() {
        let mut usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
        };
        assert_eq!(usage.total(), 150);
        usage.add(&TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
        });
        assert_eq!(usage.total(), 165);
    }
}
