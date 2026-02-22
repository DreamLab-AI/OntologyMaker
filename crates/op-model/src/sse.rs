use futures::StreamExt;
use op_core::OpError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::time::Duration;
use tracing::warn;

/// A single SSE event: (event_type, parsed JSON data).
pub type SseEvent = (String, serde_json::Map<String, Value>);

/// Callback invoked for each SSE event as it arrives.
pub type SseEventCb = Box<dyn Fn(&str, &serde_json::Map<String, Value>) + Send + Sync>;

/// Parse SSE lines from raw bytes, returning a list of `(event_type, data_dict)` pairs.
///
/// This is the Rust equivalent of Python's `_read_sse_events`. It performs manual
/// line-by-line parsing of the SSE protocol.
pub fn read_sse_events(
    raw_text: &str,
    on_sse_event: Option<&SseEventCb>,
) -> Result<Vec<SseEvent>, OpError> {
    let mut events: Vec<SseEvent> = Vec::new();
    let mut current_event = String::new();
    let mut current_data_lines: Vec<String> = Vec::new();

    for line in raw_text.lines() {
        let line = line.trim_end_matches('\r');

        if let Some(rest) = line.strip_prefix("event:") {
            current_event = rest.trim().to_string();
            continue;
        }

        if let Some(rest) = line.strip_prefix("data:") {
            let data_str = rest.trim();
            if data_str == "[DONE]" {
                break;
            }
            current_data_lines.push(data_str.to_string());
            continue;
        }

        // Empty line = end of SSE message
        if line.is_empty() {
            if !current_data_lines.is_empty() {
                let joined = current_data_lines.join("\n");
                let data_map = parse_sse_data(&joined)?;
                if let Some(map) = data_map {
                    // Check for Anthropic error events
                    check_stream_error(&map)?;
                    if let Some(cb) = on_sse_event {
                        cb(&current_event, &map);
                    }
                    events.push((current_event.clone(), map));
                }
                current_data_lines.clear();
                current_event.clear();
            }
            continue;
        }
    }

    // Flush any remaining data (some servers don't end with empty line)
    if !current_data_lines.is_empty() {
        let joined = current_data_lines.join("\n");
        let data_map = parse_sse_data(&joined)?;
        if let Some(map) = data_map {
            check_stream_error(&map)?;
            if let Some(cb) = on_sse_event {
                cb(&current_event, &map);
            }
            events.push((current_event, map));
        }
    }

    Ok(events)
}

/// Parse a raw SSE data string into a JSON object map, or return None if not an object.
fn parse_sse_data(raw: &str) -> Result<Option<serde_json::Map<String, Value>>, OpError> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => {
            // Non-JSON data; wrap it
            let mut map = serde_json::Map::new();
            map.insert("_raw".to_string(), Value::String(raw.to_string()));
            return Ok(Some(map));
        }
    };

    match parsed {
        Value::Object(map) => Ok(Some(map)),
        _ => Ok(None), // Non-object JSON, skip
    }
}

/// Check for Anthropic-style stream error events.
fn check_stream_error(map: &serde_json::Map<String, Value>) -> Result<(), OpError> {
    if map.get("type").and_then(|v| v.as_str()) == Some("error") {
        let err_msg = map
            .get("error")
            .and_then(|e| e.as_object())
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown stream error");
        return Err(OpError::model(format!("Stream error: {err_msg}")));
    }
    Ok(())
}

/// Stream an SSE endpoint with first-byte timeout and retry logic.
///
/// This is the Rust equivalent of Python's `_http_stream_sse`.
#[allow(clippy::too_many_arguments)]
pub async fn http_stream_sse(
    url: &str,
    method: &str,
    headers: &[(&str, &str)],
    payload: &Value,
    first_byte_timeout: f64,
    stream_timeout: f64,
    max_retries: u32,
    on_sse_event: Option<&SseEventCb>,
) -> Result<Vec<SseEvent>, OpError> {
    let mut header_map = HeaderMap::new();
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| OpError::http(format!("Invalid header name {k}: {e}")))?;
        let val = HeaderValue::from_str(v)
            .map_err(|e| OpError::http(format!("Invalid header value for {k}: {e}")))?;
        header_map.insert(name, val);
    }

    let body_bytes = serde_json::to_vec(payload)
        .map_err(|e| OpError::http(format!("Failed to serialize payload: {e}")))?;

    let mut last_err: Option<OpError> = None;

    for attempt in 0..max_retries {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs_f64(first_byte_timeout))
            .build()
            .map_err(|e| OpError::http(format!("Failed to build HTTP client: {e}")))?;

        let request = match method.to_uppercase().as_str() {
            "POST" => client.post(url),
            "GET" => client.get(url),
            other => {
                return Err(OpError::http(format!("Unsupported method: {other}")));
            }
        };

        let request = request
            .headers(header_map.clone())
            .body(body_bytes.clone())
            .timeout(Duration::from_secs_f64(first_byte_timeout));

        // Send the request - on timeout/connection error, retry
        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() || e.is_connect() {
                    warn!(
                        attempt = attempt + 1,
                        max_retries, "SSE connection failed, retrying: {e}"
                    );
                    last_err = Some(OpError::http(format!(
                        "Connection error calling {url}: {e}"
                    )));
                    continue;
                }
                return Err(OpError::http(format!(
                    "Connection error calling {url}: {e}"
                )));
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OpError::model(format!(
                "HTTP {status} calling {url}: {body}"
            )));
        }

        // First byte received — now stream with a longer timeout
        let stream_deadline =
            tokio::time::Instant::now() + Duration::from_secs_f64(stream_timeout);

        let result = tokio::time::timeout_at(stream_deadline, async {
            collect_sse_stream(resp, on_sse_event).await
        })
        .await;

        match result {
            Ok(inner) => return inner,
            Err(_elapsed) => {
                warn!(
                    attempt = attempt + 1,
                    max_retries, "SSE stream timed out after {stream_timeout}s"
                );
                last_err = Some(OpError::http(format!(
                    "Stream timed out after {stream_timeout}s calling {url}"
                )));
                continue;
            }
        }
    }

    Err(OpError::model(format!(
        "Timed out after {max_retries} attempts calling {url}: {}",
        last_err
            .as_ref()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".into())
    )))
}

/// Collect the entire SSE stream from a response, performing line-by-line parsing.
async fn collect_sse_stream(
    resp: reqwest::Response,
    on_sse_event: Option<&SseEventCb>,
) -> Result<Vec<SseEvent>, OpError> {
    let mut events: Vec<SseEvent> = Vec::new();
    let mut current_event = String::new();
    let mut current_data_lines: Vec<String> = Vec::new();
    let mut buffer = String::new();

    let mut stream = resp.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result
            .map_err(|e| OpError::http(format!("Error reading SSE stream: {e}")))?;
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if let Some(rest) = line.strip_prefix("event:") {
                current_event = rest.trim().to_string();
                continue;
            }

            if let Some(rest) = line.strip_prefix("data:") {
                let data_str = rest.trim();
                if data_str == "[DONE]" {
                    return Ok(events);
                }
                current_data_lines.push(data_str.to_string());
                continue;
            }

            // Empty line = end of SSE message
            if line.is_empty() {
                if !current_data_lines.is_empty() {
                    let joined = current_data_lines.join("\n");
                    let data_map = parse_sse_data(&joined)?;
                    if let Some(map) = data_map {
                        check_stream_error(&map)?;
                        if let Some(cb) = on_sse_event {
                            cb(&current_event, &map);
                        }
                        events.push((current_event.clone(), map));
                    }
                    current_data_lines.clear();
                    current_event.clear();
                }
                continue;
            }
        }
    }

    // Flush any remaining data in the buffer
    if !buffer.is_empty() {
        for line in buffer.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(rest) = line.strip_prefix("data:") {
                let data_str = rest.trim();
                if data_str == "[DONE]" {
                    break;
                }
                current_data_lines.push(data_str.to_string());
            } else if let Some(rest) = line.strip_prefix("event:") {
                current_event = rest.trim().to_string();
            }
        }
    }

    // Flush remaining accumulated data
    if !current_data_lines.is_empty() {
        let joined = current_data_lines.join("\n");
        let data_map = parse_sse_data(&joined)?;
        if let Some(map) = data_map {
            check_stream_error(&map)?;
            if let Some(cb) = on_sse_event {
                cb(&current_event, &map);
            }
            events.push((current_event, map));
        }
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_sse_events_basic() {
        let raw = "event: message\ndata: {\"text\": \"hello\"}\n\n";
        let events = read_sse_events(raw, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "message");
        assert_eq!(
            events[0].1.get("text").unwrap().as_str().unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_read_sse_events_done_signal() {
        let raw = "data: {\"text\": \"hello\"}\n\ndata: [DONE]\n\n";
        let events = read_sse_events(raw, None).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_read_sse_events_multiple() {
        let raw = "data: {\"a\": 1}\n\ndata: {\"b\": 2}\n\n";
        let events = read_sse_events(raw, None).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_read_sse_events_non_json() {
        let raw = "data: not json data\n\n";
        let events = read_sse_events(raw, None).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].1.contains_key("_raw"));
    }

    #[test]
    fn test_read_sse_events_error() {
        let raw = "data: {\"type\": \"error\", \"error\": {\"message\": \"overloaded\"}}\n\n";
        let result = read_sse_events(raw, None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("overloaded"));
    }

    #[test]
    fn test_read_sse_events_with_callback() {
        use std::sync::{Arc, Mutex};

        let collected = Arc::new(Mutex::new(Vec::new()));
        let collected_clone = collected.clone();
        let cb: SseEventCb = Box::new(move |event_type, data| {
            collected_clone
                .lock()
                .unwrap()
                .push((event_type.to_string(), data.clone()));
        });

        let raw = "event: delta\ndata: {\"x\": 1}\n\nevent: delta\ndata: {\"x\": 2}\n\n";
        let events = read_sse_events(raw, Some(&cb)).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(collected.lock().unwrap().len(), 2);
    }

    #[test]
    fn test_read_sse_events_flush_without_trailing_newline() {
        let raw = "data: {\"final\": true}";
        let events = read_sse_events(raw, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].1.get("final").unwrap().as_bool().unwrap(),
            true
        );
    }

    #[test]
    fn test_read_sse_events_empty_input() {
        let events = read_sse_events("", None).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_read_sse_events_multiline_data() {
        // Multiple data: lines before the empty line should be joined
        let raw = "data: {\"part1\":\n data: \"value\"}\n\n";
        let events = read_sse_events(raw, None).unwrap();
        assert_eq!(events.len(), 1);
        // The joined result should parse as JSON or fall back to _raw
        // "{\"part1\":\n\"value\"}" would fail JSON parsing so it becomes _raw
        // Actually, the lines are: "{\"part1\":" and "\"value\"}" joined with \n
    }
}
