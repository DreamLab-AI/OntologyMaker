use crate::sse::SseEvent;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Reconstruct an OpenAI non-streaming response dict from SSE delta chunks.
///
/// This is the Rust equivalent of Python's `_accumulate_openai_stream`.
pub fn accumulate_openai_stream(events: &[SseEvent]) -> Map<String, Value> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls_by_index: BTreeMap<i64, Map<String, Value>> = BTreeMap::new();
    let mut finish_reason = String::new();
    let mut usage: Option<Value> = None;

    for (_event_type, chunk) in events {
        // Usage may appear in a dedicated chunk or alongside the last delta
        if let Some(u) = chunk.get("usage") {
            if !u.is_null() {
                usage = Some(u.clone());
            }
        }

        let choices = match chunk.get("choices") {
            Some(Value::Array(arr)) if !arr.is_empty() => arr,
            _ => continue,
        };

        let choice = match choices[0].as_object() {
            Some(c) => c,
            None => continue,
        };

        if let Some(Value::String(fr)) = choice.get("finish_reason") {
            finish_reason = fr.clone();
        }

        let delta = match choice.get("delta") {
            Some(Value::Object(d)) if !d.is_empty() => d,
            _ => continue,
        };

        // Text content
        if let Some(Value::String(content)) = delta.get("content") {
            if !content.is_empty() {
                text_parts.push(content.clone());
            }
        }

        // Tool calls (streamed incrementally)
        if let Some(Value::Array(tc_deltas)) = delta.get("tool_calls") {
            for tc_delta_val in tc_deltas {
                let tc_delta = match tc_delta_val.as_object() {
                    Some(d) => d,
                    None => continue,
                };

                let idx = tc_delta
                    .get("index")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let tc = tool_calls_by_index.entry(idx).or_insert_with(|| {
                    let mut m = Map::new();
                    m.insert(
                        "id".to_string(),
                        Value::String(
                            tc_delta
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                        ),
                    );
                    m.insert("type".to_string(), Value::String("function".to_string()));
                    let mut func = Map::new();
                    func.insert("name".to_string(), Value::String(String::new()));
                    func.insert("arguments".to_string(), Value::String(String::new()));
                    m.insert("function".to_string(), Value::Object(func));
                    m
                });

                if let Some(Value::String(id)) = tc_delta.get("id") {
                    if !id.is_empty() {
                        tc.insert("id".to_string(), Value::String(id.clone()));
                    }
                }

                if let Some(Value::Object(func_delta)) = tc_delta.get("function") {
                    if let Some(func) = tc.get_mut("function").and_then(|f| f.as_object_mut()) {
                        if let Some(Value::String(name)) = func_delta.get("name") {
                            if !name.is_empty() {
                                func.insert("name".to_string(), Value::String(name.clone()));
                            }
                        }
                        if let Some(Value::String(args)) = func_delta.get("arguments") {
                            if !args.is_empty() {
                                let existing = func
                                    .get("arguments")
                                    .and_then(|a| a.as_str())
                                    .unwrap_or("");
                                let new_args = format!("{existing}{args}");
                                func.insert(
                                    "arguments".to_string(),
                                    Value::String(new_args),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // Build the final message
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert(
        "content".to_string(),
        if text_parts.is_empty() {
            Value::Null
        } else {
            Value::String(text_parts.concat())
        },
    );

    if tool_calls_by_index.is_empty() {
        message.insert("tool_calls".to_string(), Value::Null);
    } else {
        let tc_list: Vec<Value> = tool_calls_by_index
            .into_values()
            .map(Value::Object)
            .collect();
        message.insert("tool_calls".to_string(), Value::Array(tc_list));
    }

    let mut result = Map::new();
    let choice = json!({
        "message": Value::Object(message),
        "finish_reason": finish_reason,
    });
    result.insert("choices".to_string(), Value::Array(vec![choice]));
    if let Some(u) = usage {
        result.insert("usage".to_string(), u);
    }
    result
}

/// Reconstruct an Anthropic non-streaming response dict from SSE events.
///
/// This is the Rust equivalent of Python's `_accumulate_anthropic_stream`.
pub fn accumulate_anthropic_stream(events: &[SseEvent]) -> Map<String, Value> {
    let mut blocks_by_index: BTreeMap<i64, Map<String, Value>> = BTreeMap::new();
    let mut stop_reason = String::new();
    let mut usage = Map::new();

    for (event_type, data) in events {
        let msg_type = data
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or(event_type.as_str());

        match msg_type {
            "message_start" => {
                if let Some(Value::Object(msg)) = data.get("message") {
                    if let Some(Value::Object(msg_usage)) = msg.get("usage") {
                        for (k, v) in msg_usage {
                            usage.insert(k.clone(), v.clone());
                        }
                    }
                }
            }

            "content_block_start" => {
                let idx = data
                    .get("index")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(blocks_by_index.len() as i64);
                let block = data
                    .get("content_block")
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                let btype = block
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("text");

                let new_block = match btype {
                    "text" => {
                        let mut m = Map::new();
                        m.insert("type".to_string(), Value::String("text".to_string()));
                        m.insert(
                            "text".to_string(),
                            block
                                .get("text")
                                .cloned()
                                .unwrap_or(Value::String(String::new())),
                        );
                        m
                    }
                    "tool_use" => {
                        let mut m = Map::new();
                        m.insert("type".to_string(), Value::String("tool_use".to_string()));
                        m.insert(
                            "id".to_string(),
                            block
                                .get("id")
                                .cloned()
                                .unwrap_or(Value::String(String::new())),
                        );
                        m.insert(
                            "name".to_string(),
                            block
                                .get("name")
                                .cloned()
                                .unwrap_or(Value::String(String::new())),
                        );
                        m.insert("input".to_string(), Value::Object(Map::new()));
                        m.insert(
                            "_input_json".to_string(),
                            Value::String(String::new()),
                        );
                        m
                    }
                    "thinking" => {
                        let mut m = Map::new();
                        m.insert("type".to_string(), Value::String("thinking".to_string()));
                        m.insert(
                            "thinking".to_string(),
                            block
                                .get("thinking")
                                .cloned()
                                .unwrap_or(Value::String(String::new())),
                        );
                        m
                    }
                    _ => block,
                };

                blocks_by_index.insert(idx, new_block);
            }

            "content_block_delta" => {
                let idx = data.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
                let delta = match data.get("delta").and_then(|v| v.as_object()) {
                    Some(d) => d,
                    None => continue,
                };
                let delta_type = delta
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let block = match blocks_by_index.get_mut(&idx) {
                    Some(b) => b,
                    None => continue,
                };

                match delta_type {
                    "text_delta" => {
                        let existing = block
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let addition = delta
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        block.insert(
                            "text".to_string(),
                            Value::String(format!("{existing}{addition}")),
                        );
                    }
                    "input_json_delta" => {
                        let existing = block
                            .get("_input_json")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let addition = delta
                            .get("partial_json")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        block.insert(
                            "_input_json".to_string(),
                            Value::String(format!("{existing}{addition}")),
                        );
                    }
                    "thinking_delta" => {
                        let existing = block
                            .get("thinking")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let addition = delta
                            .get("thinking")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        block.insert(
                            "thinking".to_string(),
                            Value::String(format!("{existing}{addition}")),
                        );
                    }
                    "signature_delta" => {
                        if let Some(sig) = delta.get("signature") {
                            block.insert("signature".to_string(), sig.clone());
                        }
                    }
                    _ => {}
                }
            }

            "content_block_stop" => {
                let idx = data.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
                if let Some(block) = blocks_by_index.get_mut(&idx) {
                    if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        let raw_json = block
                            .remove("_input_json")
                            .and_then(|v| {
                                if let Value::String(s) = v {
                                    Some(s)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_default();
                        if !raw_json.is_empty() {
                            match serde_json::from_str::<Value>(&raw_json) {
                                Ok(parsed) => {
                                    block.insert("input".to_string(), parsed);
                                }
                                Err(_) => {
                                    block.insert(
                                        "input".to_string(),
                                        Value::Object(Map::new()),
                                    );
                                }
                            }
                        }
                    }
                }
            }

            "message_delta" => {
                if let Some(Value::Object(delta)) = data.get("delta") {
                    if let Some(Value::String(sr)) = delta.get("stop_reason") {
                        stop_reason = sr.clone();
                    }
                }
                if let Some(Value::Object(delta_usage)) = data.get("usage") {
                    for (k, v) in delta_usage {
                        usage.insert(k.clone(), v.clone());
                    }
                }
            }

            "message_stop" => {
                // End of stream
            }

            _ => {}
        }
    }

    // Finalize content blocks in index order
    let content_blocks: Vec<Value> = blocks_by_index
        .into_values()
        .map(|mut block| {
            block.remove("_input_json");
            Value::Object(block)
        })
        .collect();

    let mut result = Map::new();
    result.insert("content".to_string(), Value::Array(content_blocks));
    result.insert(
        "stop_reason".to_string(),
        Value::String(stop_reason),
    );
    result.insert("usage".to_string(), Value::Object(usage));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(event_type: &str, data: Value) -> SseEvent {
        let map = match data {
            Value::Object(m) => m,
            _ => panic!("test helper requires object"),
        };
        (event_type.to_string(), map)
    }

    #[test]
    fn test_accumulate_openai_stream_text() {
        let events = vec![
            make_event(
                "",
                json!({
                    "choices": [{
                        "delta": {"content": "Hello"},
                        "finish_reason": null
                    }]
                }),
            ),
            make_event(
                "",
                json!({
                    "choices": [{
                        "delta": {"content": " world"},
                        "finish_reason": "stop"
                    }]
                }),
            ),
            make_event(
                "",
                json!({
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5}
                }),
            ),
        ];

        let result = accumulate_openai_stream(&events);
        let choices = result.get("choices").unwrap().as_array().unwrap();
        let msg = choices[0]
            .as_object()
            .unwrap()
            .get("message")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(msg.get("content").unwrap().as_str().unwrap(), "Hello world");
        assert_eq!(msg.get("role").unwrap().as_str().unwrap(), "assistant");

        let fr = choices[0]
            .as_object()
            .unwrap()
            .get("finish_reason")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(fr, "stop");

        let usage = result.get("usage").unwrap().as_object().unwrap();
        assert_eq!(usage.get("prompt_tokens").unwrap().as_i64().unwrap(), 10);
        assert_eq!(usage.get("completion_tokens").unwrap().as_i64().unwrap(), 5);
    }

    #[test]
    fn test_accumulate_openai_stream_tool_calls() {
        let events = vec![
            make_event(
                "",
                json!({
                    "choices": [{
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "id": "call_1",
                                "function": {"name": "read_file", "arguments": ""}
                            }]
                        },
                        "finish_reason": null
                    }]
                }),
            ),
            make_event(
                "",
                json!({
                    "choices": [{
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "function": {"arguments": "{\"path\":"}
                            }]
                        },
                        "finish_reason": null
                    }]
                }),
            ),
            make_event(
                "",
                json!({
                    "choices": [{
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "function": {"arguments": " \"test.txt\"}"}
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
            ),
        ];

        let result = accumulate_openai_stream(&events);
        let choices = result.get("choices").unwrap().as_array().unwrap();
        let msg = choices[0]
            .as_object()
            .unwrap()
            .get("message")
            .unwrap()
            .as_object()
            .unwrap();
        let tcs = msg.get("tool_calls").unwrap().as_array().unwrap();
        assert_eq!(tcs.len(), 1);
        let tc = tcs[0].as_object().unwrap();
        assert_eq!(tc.get("id").unwrap().as_str().unwrap(), "call_1");
        let func = tc.get("function").unwrap().as_object().unwrap();
        assert_eq!(func.get("name").unwrap().as_str().unwrap(), "read_file");
        assert_eq!(
            func.get("arguments").unwrap().as_str().unwrap(),
            "{\"path\": \"test.txt\"}"
        );
    }

    #[test]
    fn test_accumulate_openai_stream_empty() {
        let result = accumulate_openai_stream(&[]);
        let choices = result.get("choices").unwrap().as_array().unwrap();
        let msg = choices[0]
            .as_object()
            .unwrap()
            .get("message")
            .unwrap()
            .as_object()
            .unwrap();
        assert!(msg.get("content").unwrap().is_null());
        assert!(msg.get("tool_calls").unwrap().is_null());
    }

    #[test]
    fn test_accumulate_anthropic_stream_text() {
        let events = vec![
            make_event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "usage": {"input_tokens": 20}
                    }
                }),
            ),
            make_event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""}
                }),
            ),
            make_event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "Hello"}
                }),
            ),
            make_event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": " world"}
                }),
            ),
            make_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
            make_event(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn"},
                    "usage": {"output_tokens": 10}
                }),
            ),
        ];

        let result = accumulate_anthropic_stream(&events);
        let content = result.get("content").unwrap().as_array().unwrap();
        assert_eq!(content.len(), 1);
        let block = content[0].as_object().unwrap();
        assert_eq!(block.get("type").unwrap().as_str().unwrap(), "text");
        assert_eq!(block.get("text").unwrap().as_str().unwrap(), "Hello world");
        assert_eq!(
            result.get("stop_reason").unwrap().as_str().unwrap(),
            "end_turn"
        );

        let usage = result.get("usage").unwrap().as_object().unwrap();
        assert_eq!(usage.get("input_tokens").unwrap().as_i64().unwrap(), 20);
        assert_eq!(usage.get("output_tokens").unwrap().as_i64().unwrap(), 10);
    }

    #[test]
    fn test_accumulate_anthropic_stream_tool_use() {
        let events = vec![
            make_event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "tool_1",
                        "name": "run_shell"
                    }
                }),
            ),
            make_event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": "{\"cmd\":"
                    }
                }),
            ),
            make_event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": " \"ls\"}"
                    }
                }),
            ),
            make_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
        ];

        let result = accumulate_anthropic_stream(&events);
        let content = result.get("content").unwrap().as_array().unwrap();
        assert_eq!(content.len(), 1);
        let block = content[0].as_object().unwrap();
        assert_eq!(block.get("type").unwrap().as_str().unwrap(), "tool_use");
        assert_eq!(block.get("id").unwrap().as_str().unwrap(), "tool_1");
        assert_eq!(block.get("name").unwrap().as_str().unwrap(), "run_shell");
        let input = block.get("input").unwrap().as_object().unwrap();
        assert_eq!(input.get("cmd").unwrap().as_str().unwrap(), "ls");
        // _input_json should be cleaned up
        assert!(!block.contains_key("_input_json"));
    }

    #[test]
    fn test_accumulate_anthropic_stream_thinking() {
        let events = vec![
            make_event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "thinking", "thinking": ""}
                }),
            ),
            make_event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "thinking_delta", "thinking": "Let me think..."}
                }),
            ),
            make_event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "signature_delta", "signature": "sig123"}
                }),
            ),
            make_event(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": 0}),
            ),
        ];

        let result = accumulate_anthropic_stream(&events);
        let content = result.get("content").unwrap().as_array().unwrap();
        let block = content[0].as_object().unwrap();
        assert_eq!(block.get("type").unwrap().as_str().unwrap(), "thinking");
        assert_eq!(
            block.get("thinking").unwrap().as_str().unwrap(),
            "Let me think..."
        );
        assert_eq!(
            block.get("signature").unwrap().as_str().unwrap(),
            "sig123"
        );
    }

    #[test]
    fn test_accumulate_anthropic_stream_empty() {
        let result = accumulate_anthropic_stream(&[]);
        let content = result.get("content").unwrap().as_array().unwrap();
        assert!(content.is_empty());
        assert_eq!(
            result.get("stop_reason").unwrap().as_str().unwrap(),
            ""
        );
    }
}
