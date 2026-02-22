use op_core::OpError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::time::Duration;

/// Perform an HTTP request and parse the response as a JSON object.
///
/// This is the Rust equivalent of the Python `_http_json` helper.
pub async fn http_json(
    url: &str,
    method: &str,
    headers: &[(&str, &str)],
    payload: Option<&Value>,
    timeout_sec: u64,
) -> Result<serde_json::Map<String, Value>, OpError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_sec))
        .build()
        .map_err(|e| OpError::http(format!("Failed to build HTTP client: {e}")))?;

    let mut header_map = HeaderMap::new();
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| OpError::http(format!("Invalid header name {k}: {e}")))?;
        let val = HeaderValue::from_str(v)
            .map_err(|e| OpError::http(format!("Invalid header value for {k}: {e}")))?;
        header_map.insert(name, val);
    }

    let request = match method.to_uppercase().as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "PATCH" => client.patch(url),
        other => {
            return Err(OpError::http(format!("Unsupported HTTP method: {other}")));
        }
    };

    let mut request = request.headers(header_map);
    if let Some(body) = payload {
        let body_bytes = serde_json::to_vec(body)
            .map_err(|e| OpError::http(format!("Failed to serialize payload: {e}")))?;
        request = request.body(body_bytes);
    }

    let resp = request
        .send()
        .await
        .map_err(|e| OpError::http(format!("Connection error calling {url}: {e}")))?;

    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| OpError::http(format!("Failed to read response from {url}: {e}")))?;

    if !status.is_success() {
        return Err(OpError::model(format!(
            "HTTP {status} calling {url}: {body}"
        )));
    }

    let parsed: Value = serde_json::from_str(&body).map_err(|_| {
        let preview = if body.len() > 500 { &body[..500] } else { &body };
        OpError::model(format!("Non-JSON response from {url}: {preview}"))
    })?;

    match parsed {
        Value::Object(map) => Ok(map),
        other => Err(OpError::model(format!(
            "Unexpected non-object JSON from {url}: {:?}",
            other
        ))),
    }
}

/// Extract text content from a potentially nested content field.
///
/// Handles both plain strings and arrays of content blocks (OpenAI/Anthropic format).
pub fn extract_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let mut text_parts: Vec<String> = Vec::new();
            for part in arr {
                if let Value::Object(obj) = part {
                    if let Some(Value::String(t)) = obj.get("text") {
                        text_parts.push(t.clone());
                        continue;
                    }
                    if obj.get("type").and_then(|v| v.as_str()) == Some("text") {
                        if let Some(Value::String(t)) = obj.get("text") {
                            text_parts.push(t.clone());
                        }
                    }
                }
            }
            text_parts.join("\n")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_content_string() {
        let v = json!("hello world");
        assert_eq!(extract_content(&v), "hello world");
    }

    #[test]
    fn test_extract_content_array_of_text_blocks() {
        let v = json!([
            {"type": "text", "text": "line 1"},
            {"type": "text", "text": "line 2"}
        ]);
        assert_eq!(extract_content(&v), "line 1\nline 2");
    }

    #[test]
    fn test_extract_content_array_with_text_key() {
        let v = json!([
            {"text": "direct text"},
            {"type": "image", "source": {}}
        ]);
        assert_eq!(extract_content(&v), "direct text");
    }

    #[test]
    fn test_extract_content_null() {
        let v = json!(null);
        assert_eq!(extract_content(&v), "");
    }

    #[test]
    fn test_extract_content_empty_array() {
        let v = json!([]);
        assert_eq!(extract_content(&v), "");
    }

    #[test]
    fn test_extract_content_number() {
        let v = json!(42);
        assert_eq!(extract_content(&v), "");
    }
}
