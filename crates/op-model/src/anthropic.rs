use async_trait::async_trait;
use op_core::{Conversation, ModelTurn, OpResult, ToolCall, ToolResult};
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::accumulator::accumulate_anthropic_stream;
use crate::sse::{http_stream_sse, SseEventCb};
use crate::traits::{ContentDeltaCb, LlmModel};

/// Anthropic model implementation (native tool calling).
///
/// Supports Claude models with thinking blocks, adaptive thinking for opus-4-6,
/// and the standard Anthropic Messages API.
pub struct AnthropicModel {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub temperature: f64,
    pub reasoning_effort: Option<String>,
    pub max_tokens: u64,
    pub timeout_sec: u64,
    pub tool_defs: Option<Vec<Value>>,
    pub on_content_delta: Option<Arc<ContentDeltaCb>>,
}

impl AnthropicModel {
    /// Create a new AnthropicModel with default settings.
    pub fn new(model: String, api_key: String) -> Self {
        Self {
            model,
            api_key,
            base_url: "https://api.anthropic.com/v1".to_string(),
            temperature: 0.0,
            reasoning_effort: None,
            max_tokens: 16384,
            timeout_sec: 300,
            tool_defs: None,
            on_content_delta: None,
        }
    }

    /// Check if the model is Claude Opus 4.6 (which uses adaptive thinking).
    fn is_opus_46(&self) -> bool {
        let lower = self.model.to_lowercase();
        lower.contains("opus-4-6") || lower.contains("opus-4.6")
    }

    /// Build the list of tool definitions in Anthropic format.
    fn build_tools(&self) -> Value {
        match &self.tool_defs {
            Some(defs) => Value::Array(defs.clone()),
            None => Value::Array(Vec::new()),
        }
    }

    /// Build the SSE event callback for forwarding streaming deltas.
    fn build_sse_callback(&self) -> Option<SseEventCb> {
        let cb = self.on_content_delta.clone()?;
        Some(Box::new(move |_event_type: &str, data: &Map<String, Value>| {
            let msg_type = data
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or(_event_type);
            if msg_type != "content_block_delta" {
                return;
            }
            let delta = match data.get("delta").and_then(|d| d.as_object()) {
                Some(d) => d,
                None => return,
            };
            let delta_type = delta
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match delta_type {
                "thinking_delta" => {
                    if let Some(text) = delta.get("thinking").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            cb("thinking", text);
                        }
                    }
                }
                "text_delta" => {
                    if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            cb("text", text);
                        }
                    }
                }
                _ => {}
            }
        }))
    }

    /// Parse the accumulated Anthropic response into a ModelTurn.
    fn parse_anthropic_response(
        &self,
        parsed: &Map<String, Value>,
    ) -> OpResult<ModelTurn> {
        let stop_reason = parsed
            .get("stop_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let content_blocks = parsed
            .get("content")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut text_parts: Vec<String> = Vec::new();

        for block in &content_blocks {
            let block_obj = match block.as_object() {
                Some(o) => o,
                None => continue,
            };
            let block_type = block_obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match block_type {
                "tool_use" => {
                    let input = block_obj
                        .get("input")
                        .cloned()
                        .unwrap_or(json!({}));
                    let args = match input {
                        Value::Object(_) => input,
                        _ => json!({}),
                    };
                    tool_calls.push(ToolCall {
                        id: block_obj
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: block_obj
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        arguments: args,
                    });
                }
                "text" => {
                    if let Some(t) = block_obj.get("text").and_then(|v| v.as_str()) {
                        if !t.trim().is_empty() {
                            text_parts.push(t.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        let text_content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join("\n"))
        };

        // Extract token usage
        let usage = parsed.get("usage").and_then(|u| u.as_object());
        let input_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Ok(ModelTurn {
            tool_calls,
            text: text_content,
            stop_reason,
            raw_response: Value::Array(content_blocks),
            input_tokens,
            output_tokens,
        })
    }
}

#[async_trait]
impl LlmModel for AnthropicModel {
    fn create_conversation(
        &self,
        system_prompt: &str,
        initial_user_message: &str,
    ) -> Conversation {
        let messages = vec![json!({"role": "user", "content": initial_user_message})];
        let mut conv = Conversation::new(system_prompt.to_string());
        conv.provider_messages = messages;
        conv
    }

    async fn complete(&self, conversation: &Conversation) -> OpResult<ModelTurn> {
        let effort = self
            .reasoning_effort
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let use_thinking = matches!(effort.as_str(), "low" | "medium" | "high");

        let mut payload = Map::new();
        payload.insert("model".to_string(), json!(self.model));
        payload.insert("max_tokens".to_string(), json!(self.max_tokens));
        payload.insert(
            "messages".to_string(),
            Value::Array(conversation.provider_messages.clone()),
        );
        payload.insert("tools".to_string(), self.build_tools());
        payload.insert("stream".to_string(), json!(true));

        if !conversation.stop_sequences.is_empty() {
            payload.insert(
                "stop_sequences".to_string(),
                Value::Array(
                    conversation
                        .stop_sequences
                        .iter()
                        .map(|s| json!(s))
                        .collect(),
                ),
            );
        }

        // Thinking is incompatible with temperature
        if !use_thinking {
            payload.insert("temperature".to_string(), json!(self.temperature));
        }

        if use_thinking {
            if self.is_opus_46() {
                // Opus 4.6: adaptive thinking (manual mode deprecated).
                payload.insert("thinking".to_string(), json!({"type": "adaptive"}));
                payload.insert("output_config".to_string(), json!({"effort": effort}));
            } else {
                // Older models: manual thinking with explicit budget.
                let budget: u64 = match effort.as_str() {
                    "low" => 1024,
                    "medium" => 4096,
                    "high" => 8192,
                    _ => 4096,
                };
                let max_tokens = payload
                    .get("max_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(self.max_tokens);
                if max_tokens <= budget {
                    payload.insert(
                        "max_tokens".to_string(),
                        json!(budget + 8192),
                    );
                }
                payload.insert(
                    "thinking".to_string(),
                    json!({"type": "enabled", "budget_tokens": budget}),
                );
            }
        }

        if !conversation.system_prompt.is_empty() {
            payload.insert(
                "system".to_string(),
                json!(conversation.system_prompt),
            );
        }

        let url = format!("{}/messages", self.base_url.trim_end_matches('/'));
        let owned_headers: Vec<(String, String)> = vec![
            ("x-api-key".to_string(), self.api_key.clone()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        let header_refs: Vec<(&str, &str)> = owned_headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let sse_cb = self.build_sse_callback();
        let sse_cb_ref = sse_cb.as_ref();

        let payload_val = Value::Object(payload.clone());

        let result = http_stream_sse(
            &url,
            "POST",
            &header_refs,
            &payload_val,
            10.0, // first_byte_timeout
            self.timeout_sec as f64,
            3,
            sse_cb_ref,
        )
        .await;

        // Handle thinking fallback
        let events = match result {
            Ok(events) => events,
            Err(e) => {
                let text = e.to_string().to_lowercase();
                let unsupported_thinking = use_thinking
                    && text.contains("thinking")
                    && (text.contains("unknown")
                        || text.contains("unsupported")
                        || text.contains("invalid"));
                if !unsupported_thinking {
                    return Err(e);
                }
                // Retry without thinking
                payload.remove("thinking");
                payload.remove("output_config");
                let payload_val2 = Value::Object(payload);
                let sse_cb2 = self.build_sse_callback();
                let sse_cb_ref2 = sse_cb2.as_ref();
                http_stream_sse(
                    &url,
                    "POST",
                    &header_refs,
                    &payload_val2,
                    10.0,
                    self.timeout_sec as f64,
                    3,
                    sse_cb_ref2,
                )
                .await?
            }
        };

        let parsed = accumulate_anthropic_stream(&events);
        self.parse_anthropic_response(&parsed)
    }

    fn append_assistant_turn(&self, conversation: &mut Conversation, turn: &ModelTurn) {
        // Replay the full content block array (including thinking blocks)
        conversation.provider_messages.push(json!({
            "role": "assistant",
            "content": turn.raw_response,
        }));
        conversation.turn_count += 1;
    }

    fn append_tool_results(
        &self,
        conversation: &mut Conversation,
        results: &[ToolResult],
    ) {
        let mut tool_result_blocks: Vec<Value> = Vec::new();

        for r in results {
            let content = if let Some(ref image) = r.image {
                json!([
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": image.media_type,
                            "data": image.base64_data,
                        },
                    },
                    {"type": "text", "text": r.content},
                ])
            } else {
                json!(r.content)
            };

            let mut block = Map::new();
            block.insert("type".to_string(), json!("tool_result"));
            block.insert("tool_use_id".to_string(), json!(r.tool_call_id));
            block.insert("content".to_string(), content);
            if r.is_error {
                block.insert("is_error".to_string(), json!(true));
            }
            tool_result_blocks.push(Value::Object(block));
        }

        conversation.provider_messages.push(json!({
            "role": "user",
            "content": tool_result_blocks,
        }));
    }

    fn condense_conversation(
        &self,
        conversation: &mut Conversation,
        keep_recent_turns: usize,
    ) -> usize {
        let msgs = &mut conversation.provider_messages;
        let placeholder = "[earlier tool output condensed]";

        // Find indices of user messages that contain tool_result blocks.
        let tool_msg_indices: Vec<usize> = msgs
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let obj = match m.as_object() {
                    Some(o) => o,
                    None => return false,
                };
                if obj.get("role").and_then(|r| r.as_str()) != Some("user") {
                    return false;
                }
                let content = match obj.get("content").and_then(|c| c.as_array()) {
                    Some(arr) => arr,
                    None => return false,
                };
                content.iter().any(|b| {
                    b.as_object()
                        .and_then(|o| o.get("type"))
                        .and_then(|t| t.as_str())
                        == Some("tool_result")
                })
            })
            .map(|(i, _)| i)
            .collect();

        if tool_msg_indices.len() <= keep_recent_turns {
            return 0;
        }

        let to_condense =
            &tool_msg_indices[..tool_msg_indices.len() - keep_recent_turns];
        let mut condensed = 0;

        for &idx in to_condense {
            let content = match msgs[idx]
                .as_object_mut()
                .and_then(|o| o.get_mut("content"))
                .and_then(|c| c.as_array_mut())
            {
                Some(arr) => arr,
                None => continue,
            };

            for block in content.iter_mut() {
                let block_obj = match block.as_object_mut() {
                    Some(o) => o,
                    None => continue,
                };
                if block_obj.get("type").and_then(|t| t.as_str()) != Some("tool_result")
                {
                    continue;
                }
                let current = block_obj
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                if current != placeholder {
                    block_obj.insert(
                        "content".to_string(),
                        Value::String(placeholder.to_string()),
                    );
                    condensed += 1;
                }
            }
        }

        condensed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model() -> AnthropicModel {
        AnthropicModel::new("claude-3-5-sonnet".to_string(), "test-key".to_string())
    }

    #[test]
    fn test_is_opus_46() {
        let mut model = make_model();
        assert!(!model.is_opus_46());

        model.model = "claude-opus-4-6-20260201".to_string();
        assert!(model.is_opus_46());

        model.model = "claude-opus-4.6".to_string();
        assert!(model.is_opus_46());

        model.model = "claude-3-5-sonnet".to_string();
        assert!(!model.is_opus_46());
    }

    #[test]
    fn test_create_conversation() {
        let model = make_model();
        let conv = model.create_conversation("You are helpful.", "Hello");
        // Anthropic: no system message in provider_messages
        assert_eq!(conv.provider_messages.len(), 1);
        assert_eq!(
            conv.provider_messages[0].as_object().unwrap()["role"],
            "user"
        );
        assert_eq!(conv.system_prompt, "You are helpful.");
    }

    #[test]
    fn test_append_assistant_turn() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");
        let turn = ModelTurn {
            text: Some("Hello!".to_string()),
            raw_response: json!([{"type": "text", "text": "Hello!"}]),
            ..Default::default()
        };
        model.append_assistant_turn(&mut conv, &turn);
        assert_eq!(conv.provider_messages.len(), 2);
        assert_eq!(conv.turn_count, 1);
        assert_eq!(conv.provider_messages[1]["role"], "assistant");
    }

    #[test]
    fn test_append_tool_results() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");
        let results = vec![ToolResult::ok(
            "tool_1".to_string(),
            "read_file".to_string(),
            "file content".to_string(),
        )];
        model.append_tool_results(&mut conv, &results);
        assert_eq!(conv.provider_messages.len(), 2);
        let user_msg = conv.provider_messages[1].as_object().unwrap();
        assert_eq!(user_msg["role"], "user");
        let content = user_msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_result");
        assert_eq!(content[0]["tool_use_id"], "tool_1");
    }

    #[test]
    fn test_append_tool_results_with_error() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");
        let results = vec![ToolResult::err(
            "tool_1".to_string(),
            "run_shell".to_string(),
            "command failed".to_string(),
        )];
        model.append_tool_results(&mut conv, &results);
        let user_msg = conv.provider_messages[1].as_object().unwrap();
        let content = user_msg["content"].as_array().unwrap();
        assert_eq!(content[0]["is_error"], true);
    }

    #[test]
    fn test_append_tool_results_with_image() {
        use op_core::ImageData;

        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");
        let mut result = ToolResult::ok(
            "tool_1".to_string(),
            "screenshot".to_string(),
            "captured".to_string(),
        );
        result.image = Some(ImageData {
            base64_data: "abc123".to_string(),
            media_type: "image/png".to_string(),
        });
        model.append_tool_results(&mut conv, &[result]);
        let user_msg = conv.provider_messages[1].as_object().unwrap();
        let content = user_msg["content"].as_array().unwrap();
        let block_content = content[0]["content"].as_array().unwrap();
        assert_eq!(block_content.len(), 2);
        assert_eq!(block_content[0]["type"], "image");
        assert_eq!(block_content[1]["type"], "text");
    }

    #[test]
    fn test_condense_conversation() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");

        // Add assistant + tool_result turns
        for i in 0..6 {
            conv.provider_messages.push(json!({
                "role": "assistant",
                "content": [{"type": "tool_use", "id": format!("t{i}"), "name": "test", "input": {}}]
            }));
            conv.provider_messages.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": format!("t{i}"),
                    "content": format!("result {i}")
                }]
            }));
        }

        let condensed = model.condense_conversation(&mut conv, 4);
        assert_eq!(condensed, 2);

        // Check first 2 tool result messages are condensed
        let msg1 = &conv.provider_messages[2]; // first user w/ tool_result
        let content1 = msg1["content"].as_array().unwrap();
        assert_eq!(content1[0]["content"], "[earlier tool output condensed]");

        let msg2 = &conv.provider_messages[4]; // second user w/ tool_result
        let content2 = msg2["content"].as_array().unwrap();
        assert_eq!(content2[0]["content"], "[earlier tool output condensed]");

        // Check last 4 are untouched
        let msg3 = &conv.provider_messages[6]; // third user w/ tool_result
        let content3 = msg3["content"].as_array().unwrap();
        assert_eq!(content3[0]["content"], "result 2");
    }

    #[test]
    fn test_condense_conversation_not_enough() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");

        for i in 0..3 {
            conv.provider_messages.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": format!("t{i}"),
                    "content": format!("result {i}")
                }]
            }));
        }

        let condensed = model.condense_conversation(&mut conv, 4);
        assert_eq!(condensed, 0);
    }

    #[test]
    fn test_parse_anthropic_response_text() {
        let model = make_model();
        let mut parsed = Map::new();
        parsed.insert(
            "content".to_string(),
            json!([{"type": "text", "text": "Hello world"}]),
        );
        parsed.insert("stop_reason".to_string(), json!("end_turn"));
        parsed.insert(
            "usage".to_string(),
            json!({"input_tokens": 20, "output_tokens": 10}),
        );

        let turn = model.parse_anthropic_response(&parsed).unwrap();
        assert_eq!(turn.text.as_deref(), Some("Hello world"));
        assert!(turn.tool_calls.is_empty());
        assert_eq!(turn.stop_reason, "end_turn");
        assert_eq!(turn.input_tokens, 20);
        assert_eq!(turn.output_tokens, 10);
    }

    #[test]
    fn test_parse_anthropic_response_tool_use() {
        let model = make_model();
        let mut parsed = Map::new();
        parsed.insert(
            "content".to_string(),
            json!([{
                "type": "tool_use",
                "id": "tool_1",
                "name": "read_file",
                "input": {"path": "test.txt"}
            }]),
        );
        parsed.insert("stop_reason".to_string(), json!("tool_use"));
        parsed.insert("usage".to_string(), json!({}));

        let turn = model.parse_anthropic_response(&parsed).unwrap();
        assert!(turn.text.is_none());
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "tool_1");
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.tool_calls[0].arguments["path"], "test.txt");
    }

    #[test]
    fn test_parse_anthropic_response_thinking_and_text() {
        let model = make_model();
        let mut parsed = Map::new();
        parsed.insert(
            "content".to_string(),
            json!([
                {"type": "thinking", "thinking": "Let me think..."},
                {"type": "text", "text": "Here is my answer"}
            ]),
        );
        parsed.insert("stop_reason".to_string(), json!("end_turn"));
        parsed.insert("usage".to_string(), json!({}));

        let turn = model.parse_anthropic_response(&parsed).unwrap();
        // Thinking blocks are not included in text
        assert_eq!(turn.text.as_deref(), Some("Here is my answer"));
        // But they are in raw_response
        let raw = turn.raw_response.as_array().unwrap();
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0]["type"], "thinking");
    }
}
