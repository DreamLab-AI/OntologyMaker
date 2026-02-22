use crate::http::http_json;
use chrono::{DateTime, NaiveDateTime};
use op_core::OpResult;
use regex::Regex;
use serde_json::Value;

/// A model listing entry returned by the listing functions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelEntry {
    pub provider: String,
    pub id: String,
    pub created_ts: i64,
    pub raw: Value,
}

/// Parse a timestamp from various formats (integer, float, ISO 8601 string).
fn parse_timestamp(value: &Value) -> i64 {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return i;
            }
            if let Some(f) = n.as_f64() {
                return f as i64;
            }
            0
        }
        Value::String(s) => {
            let text = s.trim();
            if text.is_empty() {
                return 0;
            }
            // Try parsing as integer
            if let Ok(i) = text.parse::<i64>() {
                return i;
            }
            // Truncate nanoseconds for Ollama compatibility
            let iso = text.replace('Z', "+00:00");
            let iso = truncate_nanoseconds(&iso);
            // Try RFC 3339
            if let Ok(dt) = DateTime::parse_from_rfc3339(&iso) {
                return dt.timestamp();
            }
            // Try without timezone
            if let Ok(dt) = NaiveDateTime::parse_from_str(&iso, "%Y-%m-%dT%H:%M:%S%.f") {
                return dt.and_utc().timestamp();
            }
            if let Ok(dt) = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S") {
                return dt.and_utc().timestamp();
            }
            0
        }
        _ => 0,
    }
}

/// Sort models by (created_ts DESC, id ASC for tiebreaker).
fn sorted_models(mut models: Vec<ModelEntry>) -> Vec<ModelEntry> {
    models.sort_by(|a, b| {
        b.created_ts
            .cmp(&a.created_ts)
            .then_with(|| a.id.cmp(&b.id))
    });
    models
}

/// Truncate nanosecond-precision fractional seconds to microseconds.
///
/// Python 3.10's `fromisoformat` only handles up to 6 decimal places.
/// Ollama emits 9 (e.g. `2026-02-21T12:44:19.177147556-05:00`).
fn truncate_nanoseconds(ts: &str) -> String {
    let re = Regex::new(r"(\.\d{6})\d+").unwrap();
    re.replace_all(ts, "$1").to_string()
}

/// List available models from an OpenAI-compatible endpoint.
pub async fn list_openai_models(
    api_key: &str,
    base_url: &str,
    timeout_sec: u64,
) -> OpResult<Vec<ModelEntry>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let auth = format!("Bearer {api_key}");
    let headers = [
        ("Authorization", auth.as_str()),
        ("Content-Type", "application/json"),
    ];
    let parsed = http_json(&url, "GET", &headers, None, timeout_sec).await?;

    let mut rows: Vec<ModelEntry> = Vec::new();

    if let Some(Value::Array(data)) = parsed.get("data") {
        for row in data {
            let obj = match row.as_object() {
                Some(o) => o,
                None => continue,
            };
            let model_id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if model_id.is_empty() {
                continue;
            }
            let created = parse_timestamp(
                obj.get("created")
                    .or_else(|| obj.get("created_at"))
                    .unwrap_or(&Value::Null),
            );
            rows.push(ModelEntry {
                provider: "openai".to_string(),
                id: model_id,
                created_ts: created,
                raw: row.clone(),
            });
        }
    }

    Ok(sorted_models(rows))
}

/// List available models from the Anthropic API.
pub async fn list_anthropic_models(
    api_key: &str,
    base_url: &str,
    timeout_sec: u64,
) -> OpResult<Vec<ModelEntry>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let headers = [
        ("x-api-key", api_key),
        ("anthropic-version", "2023-06-01"),
        ("content-type", "application/json"),
    ];
    let parsed = http_json(&url, "GET", &headers, None, timeout_sec).await?;

    let mut rows: Vec<ModelEntry> = Vec::new();

    if let Some(Value::Array(data)) = parsed.get("data") {
        for row in data {
            let obj = match row.as_object() {
                Some(o) => o,
                None => continue,
            };
            let model_id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if model_id.is_empty() {
                continue;
            }
            let created = parse_timestamp(
                obj.get("created_at")
                    .or_else(|| obj.get("created"))
                    .or_else(|| obj.get("released_at"))
                    .unwrap_or(&Value::Null),
            );
            rows.push(ModelEntry {
                provider: "anthropic".to_string(),
                id: model_id,
                created_ts: created,
                raw: row.clone(),
            });
        }
    }

    Ok(sorted_models(rows))
}

/// List available models from OpenRouter.
pub async fn list_openrouter_models(
    api_key: &str,
    base_url: &str,
    timeout_sec: u64,
) -> OpResult<Vec<ModelEntry>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let auth = format!("Bearer {api_key}");
    let headers = [
        ("Authorization", auth.as_str()),
        ("Content-Type", "application/json"),
    ];
    let parsed = http_json(&url, "GET", &headers, None, timeout_sec).await?;

    let mut rows: Vec<ModelEntry> = Vec::new();

    if let Some(Value::Array(data)) = parsed.get("data") {
        for row in data {
            let obj = match row.as_object() {
                Some(o) => o,
                None => continue,
            };
            let model_id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if model_id.is_empty() {
                continue;
            }
            let top_provider_created = obj
                .get("top_provider")
                .and_then(|tp| tp.as_object())
                .and_then(|tp| tp.get("created"));

            let created = parse_timestamp(
                obj.get("created")
                    .or_else(|| obj.get("created_at"))
                    .or(top_provider_created)
                    .or_else(|| obj.get("updated_at"))
                    .unwrap_or(&Value::Null),
            );
            rows.push(ModelEntry {
                provider: "openrouter".to_string(),
                id: model_id,
                created_ts: created,
                raw: row.clone(),
            });
        }
    }

    Ok(sorted_models(rows))
}

/// List available models from a local Ollama instance.
pub async fn list_ollama_models(
    base_url: &str,
    timeout_sec: u64,
) -> OpResult<Vec<ModelEntry>> {
    // Ollama's native API endpoint is /api/tags; strip /v1 suffix to reach it.
    let mut native_url = base_url.trim_end_matches('/').to_string();
    if native_url.ends_with("/v1") {
        native_url = native_url[..native_url.len() - 3].to_string();
    }
    let url = format!("{}/api/tags", native_url.trim_end_matches('/'));
    let headers = [("Content-Type", "application/json")];
    let parsed = http_json(&url, "GET", &headers, None, timeout_sec).await?;

    let mut rows: Vec<ModelEntry> = Vec::new();

    if let Some(Value::Array(models)) = parsed.get("models") {
        for row in models {
            let obj = match row.as_object() {
                Some(o) => o,
                None => continue,
            };
            let model_id = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if model_id.is_empty() {
                continue;
            }

            let raw_ts = obj
                .get("modified_at")
                .or_else(|| obj.get("created_at"))
                .cloned()
                .unwrap_or(Value::Null);

            let ts_val = match raw_ts {
                Value::String(s) => Value::String(truncate_nanoseconds(&s)),
                other => other,
            };

            let created = parse_timestamp(&ts_val);

            rows.push(ModelEntry {
                provider: "ollama".to_string(),
                id: model_id,
                created_ts: created,
                raw: row.clone(),
            });
        }
    }

    Ok(sorted_models(rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_timestamp_integer() {
        assert_eq!(parse_timestamp(&json!(1700000000)), 1700000000);
    }

    #[test]
    fn test_parse_timestamp_float() {
        assert_eq!(parse_timestamp(&json!(1700000000.5)), 1700000000);
    }

    #[test]
    fn test_parse_timestamp_string_integer() {
        assert_eq!(parse_timestamp(&json!("1700000000")), 1700000000);
    }

    #[test]
    fn test_parse_timestamp_iso8601() {
        let ts = parse_timestamp(&json!("2024-01-15T10:30:00Z"));
        assert!(ts > 0);
    }

    #[test]
    fn test_parse_timestamp_iso8601_with_offset() {
        let ts = parse_timestamp(&json!("2024-01-15T10:30:00+00:00"));
        assert!(ts > 0);
    }

    #[test]
    fn test_parse_timestamp_empty() {
        assert_eq!(parse_timestamp(&json!("")), 0);
    }

    #[test]
    fn test_parse_timestamp_null() {
        assert_eq!(parse_timestamp(&json!(null)), 0);
    }

    #[test]
    fn test_parse_timestamp_invalid_string() {
        assert_eq!(parse_timestamp(&json!("not a date")), 0);
    }

    #[test]
    fn test_sorted_models() {
        let models = vec![
            ModelEntry {
                provider: "test".to_string(),
                id: "model-a".to_string(),
                created_ts: 100,
                raw: json!({}),
            },
            ModelEntry {
                provider: "test".to_string(),
                id: "model-c".to_string(),
                created_ts: 300,
                raw: json!({}),
            },
            ModelEntry {
                provider: "test".to_string(),
                id: "model-b".to_string(),
                created_ts: 200,
                raw: json!({}),
            },
        ];
        let sorted = sorted_models(models);
        assert_eq!(sorted[0].id, "model-c"); // highest timestamp first
        assert_eq!(sorted[1].id, "model-b");
        assert_eq!(sorted[2].id, "model-a");
    }

    #[test]
    fn test_sorted_models_same_timestamp() {
        let models = vec![
            ModelEntry {
                provider: "test".to_string(),
                id: "zzz".to_string(),
                created_ts: 100,
                raw: json!({}),
            },
            ModelEntry {
                provider: "test".to_string(),
                id: "aaa".to_string(),
                created_ts: 100,
                raw: json!({}),
            },
        ];
        let sorted = sorted_models(models);
        // Same ts: sorted by id ASC
        assert_eq!(sorted[0].id, "aaa");
        assert_eq!(sorted[1].id, "zzz");
    }

    #[test]
    fn test_truncate_nanoseconds() {
        let input = "2026-02-21T12:44:19.177147556-05:00";
        let result = truncate_nanoseconds(input);
        assert_eq!(result, "2026-02-21T12:44:19.177147-05:00");
    }

    #[test]
    fn test_truncate_nanoseconds_no_change() {
        let input = "2026-02-21T12:44:19.177147-05:00";
        let result = truncate_nanoseconds(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_truncate_nanoseconds_no_fractional() {
        let input = "2026-02-21T12:44:19-05:00";
        let result = truncate_nanoseconds(input);
        assert_eq!(result, input);
    }
}
