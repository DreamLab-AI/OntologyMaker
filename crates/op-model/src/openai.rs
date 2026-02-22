use async_trait::async_trait;
use op_core::{Conversation, ModelTurn, OpError, OpResult, ToolCall, ToolResult};
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::accumulator::accumulate_openai_stream;
use crate::http::extract_content;
use crate::sse::{http_stream_sse, SseEventCb};
use crate::traits::{ContentDeltaCb, LlmModel};

/// OpenAI-compatible model implementation (native tool calling).
///
/// Supports standard OpenAI models, reasoning models (o-series, gpt-5),
/// and any OpenAI-compatible endpoint (Ollama, OpenRouter, etc.).
pub struct OpenAiModel {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub temperature: f64,
    pub reasoning_effort: Option<String>,
    pub timeout_sec: u64,
    pub extra_headers: Vec<(String, String)>,
    pub first_byte_timeout: f64,
    pub strict_tools: bool,
    pub tool_defs: Option<Vec<Value>>,
    pub on_content_delta: Option<Arc<ContentDeltaCb>>,
}

impl OpenAiModel {
    /// Create a new OpenAiModel with default settings.
    pub fn new(model: String, api_key: String) -> Self {
        Self {
            model,
            api_key,
            base_url: "https://api.openai.com/v1".to_string(),
            temperature: 0.0,
            reasoning_effort: None,
            timeout_sec: 300,
            extra_headers: Vec::new(),
            first_byte_timeout: 10.0,
            strict_tools: true,
            tool_defs: None,
            on_content_delta: None,
        }
    }

    /// Check if this is an OpenAI reasoning model (o-series, gpt-5).
    ///
    /// Reasoning models have different API constraints:
    /// - They don't support `temperature`
    /// - They use `max_completion_tokens` instead of `max_tokens`
    /// - They support `reasoning_effort`
    fn is_reasoning_model(&self) -> bool {
        let lower = self.model.to_lowercase();
        if lower.starts_with("o1-")
            || lower == "o1"
            || lower.starts_with("o3-")
            || lower == "o3"
            || lower.starts_with("o4-")
            || lower == "o4"
        {
            return true;
        }
        // GPT-5 series also supports reasoning_effort and uses max_completion_tokens.
        if lower.starts_with("gpt-5") {
            return true;
        }
        false
    }

    /// Build the list of tool definitions in OpenAI format.
    fn build_tools(&self) -> Value {
        match &self.tool_defs {
            Some(defs) => {
                let tools: Vec<Value> = defs
                    .iter()
                    .map(|def| {
                        let mut tool = Map::new();
                        tool.insert("type".to_string(), json!("function"));
                        let mut func = def.as_object().cloned().unwrap_or_default();
                        if self.strict_tools {
                            func.insert("strict".to_string(), json!(true));
                        }
                        tool.insert("function".to_string(), Value::Object(func));
                        Value::Object(tool)
                    })
                    .collect();
                Value::Array(tools)
            }
            None => Value::Array(Vec::new()),
        }
    }

    /// Build the SSE event callback for forwarding streaming text deltas.
    fn build_sse_callback(&self) -> Option<SseEventCb> {
        let cb = self.on_content_delta.clone()?;
        Some(Box::new(move |_event_type: &str, data: &Map<String, Value>| {
            let choices = match data.get("choices") {
                Some(Value::Array(arr)) if !arr.is_empty() => arr,
                _ => return,
            };
            let delta = match choices[0].as_object().and_then(|c| c.get("delta")) {
                Some(Value::Object(d)) if !d.is_empty() => d,
                _ => return,
            };
            if let Some(Value::String(content)) = delta.get("content") {
                if !content.is_empty() {
                    cb("text", content);
                }
            }
        }))
    }
}

#[async_trait]
impl LlmModel for OpenAiModel {
    fn create_conversation(
        &self,
        system_prompt: &str,
        initial_user_message: &str,
    ) -> Conversation {
        let messages = vec![
            json!({"role": "system", "content": system_prompt}),
            json!({"role": "user", "content": initial_user_message}),
        ];
        let mut conv = Conversation::new(system_prompt.to_string());
        conv.provider_messages = messages;
        conv
    }

    async fn complete(&self, conversation: &Conversation) -> OpResult<ModelTurn> {
        let is_reasoning = self.is_reasoning_model();

        let mut payload = Map::new();
        payload.insert("model".to_string(), json!(self.model));
        payload.insert(
            "messages".to_string(),
            Value::Array(conversation.provider_messages.clone()),
        );
        payload.insert("tools".to_string(), self.build_tools());
        payload.insert("tool_choice".to_string(), json!("auto"));
        payload.insert("stream".to_string(), json!(true));
        payload.insert(
            "stream_options".to_string(),
            json!({"include_usage": true}),
        );

        if !conversation.stop_sequences.is_empty() {
            payload.insert(
                "stop".to_string(),
                Value::Array(
                    conversation
                        .stop_sequences
                        .iter()
                        .map(|s| json!(s))
                        .collect(),
                ),
            );
        }

        // Reasoning models don't support temperature.
        if !is_reasoning {
            payload.insert("temperature".to_string(), json!(self.temperature));
        }

        // Chat Completions API uses flat `reasoning_effort`
        let effort = self
            .reasoning_effort
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if !effort.is_empty() {
            payload.insert("reasoning_effort".to_string(), json!(effort));
        }

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let auth_header = format!("Bearer {}", self.api_key);
        // We need to store these so the references remain valid
        let extra_headers_refs: Vec<(String, String)> = self.extra_headers.clone();

        // Build a vec of owned header pairs, then create references
        let mut owned_headers: Vec<(String, String)> = vec![
            ("Authorization".to_string(), auth_header),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        for (k, v) in &extra_headers_refs {
            owned_headers.push((k.clone(), v.clone()));
        }
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
            self.first_byte_timeout,
            self.timeout_sec as f64,
            3,
            sse_cb_ref,
        )
        .await;

        // Handle reasoning_effort fallback
        let events = match result {
            Ok(events) => events,
            Err(e) => {
                let text = e.to_string().to_lowercase();
                let unsupported_reasoning = !effort.is_empty()
                    && text.contains("reasoning_effort")
                    && (text.contains("unsupported_parameter") || text.contains("unknown"));
                if !unsupported_reasoning {
                    return Err(e);
                }
                // Retry without reasoning_effort
                payload.remove("reasoning_effort");
                let payload_val2 = Value::Object(payload);
                let sse_cb2 = self.build_sse_callback();
                let sse_cb_ref2 = sse_cb2.as_ref();
                http_stream_sse(
                    &url,
                    "POST",
                    &header_refs,
                    &payload_val2,
                    self.first_byte_timeout,
                    self.timeout_sec as f64,
                    3,
                    sse_cb_ref2,
                )
                .await?
            }
        };

        let parsed = accumulate_openai_stream(&events);
        self.parse_openai_response(&parsed)
    }

    fn append_assistant_turn(&self, conversation: &mut Conversation, turn: &ModelTurn) {
        // Replay the raw OpenAI message object to preserve tool_calls array
        conversation.provider_messages.push(turn.raw_response.clone());
        conversation.turn_count += 1;
    }

    fn append_tool_results(
        &self,
        conversation: &mut Conversation,
        results: &[ToolResult],
    ) {
        for r in results {
            conversation.provider_messages.push(json!({
                "role": "tool",
                "tool_call_id": r.tool_call_id,
                "name": r.name,
                "content": r.content,
            }));
            // OpenAI tool results are text-only; inject a user message with the image.
            if let Some(ref image) = r.image {
                conversation.provider_messages.push(json!({
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", image.media_type, image.base64_data),
                            },
                        },
                        {
                            "type": "text",
                            "text": format!("[Image from {}: {}]", r.name, r.content),
                        },
                    ],
                }));
            }
        }
    }

    fn condense_conversation(
        &self,
        conversation: &mut Conversation,
        keep_recent_turns: usize,
    ) -> usize {
        let msgs = &mut conversation.provider_messages;
        let placeholder = "[earlier tool output condensed]";

        // Find indices of tool-role messages
        let tool_indices: Vec<usize> = msgs
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.as_object()
                    .and_then(|o| o.get("role"))
                    .and_then(|r| r.as_str())
                    == Some("tool")
            })
            .map(|(i, _)| i)
            .collect();

        if tool_indices.len() <= keep_recent_turns {
            return 0;
        }

        let to_condense = &tool_indices[..tool_indices.len() - keep_recent_turns];
        let mut condensed = 0;

        for &idx in to_condense {
            if let Some(msg) = msgs.get_mut(idx) {
                if let Some(obj) = msg.as_object_mut() {
                    let current = obj
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    if current != placeholder {
                        obj.insert(
                            "content".to_string(),
                            Value::String(placeholder.to_string()),
                        );
                        condensed += 1;
                    }
                }
            }
        }

        condensed
    }
}

impl OpenAiModel {
    /// Parse the accumulated OpenAI response into a ModelTurn.
    fn parse_openai_response(
        &self,
        parsed: &Map<String, Value>,
    ) -> OpResult<ModelTurn> {
        let message = parsed
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.as_object())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.as_object())
            .ok_or_else(|| {
                OpError::model(format!("Model response missing content: {:?}", parsed))
            })?;

        let finish_reason = parsed
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c.as_object())
            .and_then(|c| c.get("finish_reason"))
            .and_then(|fr| fr.as_str())
            .unwrap_or("")
            .to_string();

        // Parse tool calls
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        if let Some(Value::Array(raw_tcs)) = message.get("tool_calls") {
            for tc in raw_tcs {
                let tc_obj = match tc.as_object() {
                    Some(o) => o,
                    None => continue,
                };
                let func = tc_obj
                    .get("function")
                    .and_then(|f| f.as_object())
                    .cloned()
                    .unwrap_or_default();
                let args_str = func
                    .get("arguments")
                    .and_then(|a| a.as_str())
                    .unwrap_or("{}");
                let args: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                let args_obj = match args {
                    Value::Object(m) => Value::Object(m),
                    _ => json!({}),
                };
                tool_calls.push(ToolCall {
                    id: tc_obj
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    name: func
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    arguments: args_obj,
                });
            }
        }

        // Extract text content
        let content_val = message
            .get("content")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        let text_content = extract_content(&content_val);
        let text = if text_content.trim().is_empty() {
            None
        } else {
            Some(text_content)
        };

        // Extract token usage
        let usage = parsed
            .get("usage")
            .and_then(|u| u.as_object());
        let input_tokens = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Ok(ModelTurn {
            tool_calls,
            text,
            stop_reason: finish_reason,
            raw_response: Value::Object(message.clone()),
            input_tokens,
            output_tokens,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model() -> OpenAiModel {
        OpenAiModel::new("gpt-4".to_string(), "test-key".to_string())
    }

    #[test]
    fn test_is_reasoning_model() {
        let mut model = make_model();
        assert!(!model.is_reasoning_model());

        model.model = "o1".to_string();
        assert!(model.is_reasoning_model());

        model.model = "o1-mini".to_string();
        assert!(model.is_reasoning_model());

        model.model = "o3-mini".to_string();
        assert!(model.is_reasoning_model());

        model.model = "o4-mini".to_string();
        assert!(model.is_reasoning_model());

        model.model = "gpt-5".to_string();
        assert!(model.is_reasoning_model());

        model.model = "gpt-5-turbo".to_string();
        assert!(model.is_reasoning_model());

        model.model = "gpt-4o".to_string();
        assert!(!model.is_reasoning_model());
    }

    #[test]
    fn test_create_conversation() {
        let model = make_model();
        let conv = model.create_conversation("You are helpful.", "Hello");
        assert_eq!(conv.provider_messages.len(), 2);
        assert_eq!(
            conv.provider_messages[0].as_object().unwrap()["role"],
            "system"
        );
        assert_eq!(
            conv.provider_messages[1].as_object().unwrap()["role"],
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
            raw_response: json!({"role": "assistant", "content": "Hello!"}),
            ..Default::default()
        };
        model.append_assistant_turn(&mut conv, &turn);
        assert_eq!(conv.provider_messages.len(), 3);
        assert_eq!(conv.turn_count, 1);
    }

    #[test]
    fn test_append_tool_results() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");
        let results = vec![ToolResult::ok(
            "call_1".to_string(),
            "read_file".to_string(),
            "file content".to_string(),
        )];
        model.append_tool_results(&mut conv, &results);
        assert_eq!(conv.provider_messages.len(), 3);
        let tool_msg = conv.provider_messages[2].as_object().unwrap();
        assert_eq!(tool_msg["role"], "tool");
        assert_eq!(tool_msg["tool_call_id"], "call_1");
    }

    #[test]
    fn test_append_tool_results_with_image() {
        use op_core::ImageData;

        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");
        let mut result =
            ToolResult::ok("call_1".to_string(), "screenshot".to_string(), "img".to_string());
        result.image = Some(ImageData {
            base64_data: "abc123".to_string(),
            media_type: "image/png".to_string(),
        });
        model.append_tool_results(&mut conv, &[result]);
        // Should have tool message + user image message
        assert_eq!(conv.provider_messages.len(), 4);
        let user_msg = &conv.provider_messages[3];
        assert_eq!(user_msg["role"], "user");
    }

    #[test]
    fn test_condense_conversation() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");

        // Add 6 tool messages
        for i in 0..6 {
            conv.provider_messages.push(json!({
                "role": "tool",
                "tool_call_id": format!("call_{i}"),
                "content": format!("result {i}"),
            }));
        }

        let condensed = model.condense_conversation(&mut conv, 4);
        assert_eq!(condensed, 2);

        // First 2 tool messages should be condensed
        assert_eq!(
            conv.provider_messages[2]["content"],
            "[earlier tool output condensed]"
        );
        assert_eq!(
            conv.provider_messages[3]["content"],
            "[earlier tool output condensed]"
        );
        // Last 4 should be untouched
        assert_eq!(conv.provider_messages[4]["content"], "result 2");
    }

    #[test]
    fn test_condense_conversation_not_enough() {
        let model = make_model();
        let mut conv = model.create_conversation("sys", "hi");

        // Add 3 tool messages, keep_recent=4 => nothing condensed
        for i in 0..3 {
            conv.provider_messages.push(json!({
                "role": "tool",
                "tool_call_id": format!("call_{i}"),
                "content": format!("result {i}"),
            }));
        }

        let condensed = model.condense_conversation(&mut conv, 4);
        assert_eq!(condensed, 0);
    }

    #[test]
    fn test_parse_openai_response_text() {
        let model = make_model();
        let mut parsed = Map::new();
        parsed.insert(
            "choices".to_string(),
            json!([{
                "message": {
                    "role": "assistant",
                    "content": "Hello!",
                    "tool_calls": null,
                },
                "finish_reason": "stop",
            }]),
        );
        parsed.insert(
            "usage".to_string(),
            json!({"prompt_tokens": 10, "completion_tokens": 5}),
        );

        let turn = model.parse_openai_response(&parsed).unwrap();
        assert_eq!(turn.text.as_deref(), Some("Hello!"));
        assert!(turn.tool_calls.is_empty());
        assert_eq!(turn.stop_reason, "stop");
        assert_eq!(turn.input_tokens, 10);
        assert_eq!(turn.output_tokens, 5);
    }

    #[test]
    fn test_parse_openai_response_tool_calls() {
        let model = make_model();
        let mut parsed = Map::new();
        parsed.insert(
            "choices".to_string(),
            json!([{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\": \"test.txt\"}"
                        }
                    }],
                },
                "finish_reason": "tool_calls",
            }]),
        );

        let turn = model.parse_openai_response(&parsed).unwrap();
        assert!(turn.text.is_none());
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "call_1");
        assert_eq!(turn.tool_calls[0].name, "read_file");
        assert_eq!(turn.tool_calls[0].arguments["path"], "test.txt");
    }

    #[test]
    fn test_parse_openai_response_missing_content() {
        let model = make_model();
        let parsed = Map::new(); // empty
        let result = model.parse_openai_response(&parsed);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_tools_none() {
        let model = make_model();
        let tools = model.build_tools();
        assert_eq!(tools, Value::Array(Vec::new()));
    }

    #[test]
    fn test_build_tools_with_defs() {
        let mut model = make_model();
        model.tool_defs = Some(vec![json!({
            "name": "test_tool",
            "description": "A test tool",
            "parameters": {
                "type": "object",
                "properties": {}
            }
        })]);
        let tools = model.build_tools();
        let arr = tools.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "function");
        assert_eq!(arr[0]["function"]["name"], "test_tool");
        assert_eq!(arr[0]["function"]["strict"], true);
    }

    #[test]
    fn test_build_tools_no_strict() {
        let mut model = make_model();
        model.strict_tools = false;
        model.tool_defs = Some(vec![json!({
            "name": "test_tool",
            "description": "A test tool",
            "parameters": {}
        })]);
        let tools = model.build_tools();
        let arr = tools.as_array().unwrap();
        assert!(!arr[0]["function"]
            .as_object()
            .unwrap()
            .contains_key("strict"));
    }
}
