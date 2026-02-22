//! Web operations: web_search and fetch_url using the Exa API.
//!
//! Ports the `_exa_request`, `web_search`, and `fetch_url` methods from Python.

use crate::file_ops::clip;
use op_core::OpError;

/// Exa API client for web search and URL fetching.
#[derive(Debug, Clone)]
pub struct ExaClient {
    pub api_key: Option<String>,
    pub base_url: String,
    pub timeout_sec: u64,
    http: reqwest::Client,
}

impl ExaClient {
    pub fn new(api_key: Option<String>, base_url: &str, timeout_sec: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_sec))
            .build()
            .unwrap_or_default();
        Self {
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            timeout_sec,
            http,
        }
    }

    /// Send a POST request to an Exa API endpoint.
    async fn exa_request(
        &self,
        endpoint: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, OpError> {
        let api_key = self
            .api_key
            .as_deref()
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| OpError::tool("EXA_API_KEY not configured"))?;

        let url = format!("{}{}", self.base_url, endpoint);
        let response = self
            .http
            .post(&url)
            .header("x-api-key", api_key)
            .header("Content-Type", "application/json")
            .header("User-Agent", "exa-py 1.0.18")
            .json(payload)
            .send()
            .await
            .map_err(|e| OpError::http(format!("Exa API connection error: {}", e)))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| OpError::http(format!("Exa API read error: {}", e)))?;

        if !status.is_success() {
            return Err(OpError::http(format!(
                "Exa API HTTP {}: {}",
                status.as_u16(),
                body
            )));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
            OpError::http(format!(
                "Exa API returned non-JSON payload: {}",
                &body[..body.len().min(500)]
            ))
        })?;

        if !parsed.is_object() {
            return Err(OpError::http(format!(
                "Exa API returned non-object response: {}",
                parsed
            )));
        }
        Ok(parsed)
    }
}

/// Perform a web search using the Exa API.
pub async fn web_search(
    client: &ExaClient,
    query: &str,
    num_results: Option<u32>,
    include_text: bool,
    max_file_chars: usize,
) -> String {
    let query = query.trim();
    if query.is_empty() {
        return "web_search requires non-empty query".to_string();
    }

    let clamped_results = num_results
        .unwrap_or(10)
        .max(1)
        .min(20);

    let mut payload = serde_json::json!({
        "query": query,
        "numResults": clamped_results,
    });

    if include_text {
        payload["contents"] = serde_json::json!({
            "text": { "maxCharacters": 4000 }
        });
    }

    let parsed = match client.exa_request("/search", &payload).await {
        Ok(p) => p,
        Err(e) => return format!("Web search failed: {}", e),
    };

    let mut out_results: Vec<serde_json::Value> = Vec::new();
    if let Some(results) = parsed.get("results").and_then(|r| r.as_array()) {
        for row in results {
            if !row.is_object() {
                continue;
            }
            let mut item = serde_json::json!({
                "url": row.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                "title": row.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "snippet": row.get("highlight")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| row.get("snippet").and_then(|v| v.as_str()))
                    .unwrap_or(""),
            });
            if include_text {
                if let Some(text) = row.get("text").and_then(|v| v.as_str()) {
                    item["text"] = serde_json::Value::String(clip(text, 4000));
                }
            }
            out_results.push(item);
        }
    }

    let output = serde_json::json!({
        "query": query,
        "results": out_results,
        "total": out_results.len(),
    });

    let json_str = serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string());
    clip(&json_str, max_file_chars)
}

/// Fetch the text content of one or more URLs using the Exa API.
pub async fn fetch_url(
    client: &ExaClient,
    urls: &[String],
    max_file_chars: usize,
) -> String {
    if urls.is_empty() {
        return "fetch_url requires at least one valid URL".to_string();
    }

    let normalized: Vec<String> = urls
        .iter()
        .filter_map(|u| {
            let t = u.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
        .take(10)
        .collect();

    if normalized.is_empty() {
        return "fetch_url requires at least one valid URL".to_string();
    }

    let payload = serde_json::json!({
        "ids": normalized,
        "text": { "maxCharacters": 8000 },
    });

    let parsed = match client.exa_request("/contents", &payload).await {
        Ok(p) => p,
        Err(e) => return format!("Fetch URL failed: {}", e),
    };

    let mut pages: Vec<serde_json::Value> = Vec::new();
    if let Some(results) = parsed.get("results").and_then(|r| r.as_array()) {
        for row in results {
            if !row.is_object() {
                continue;
            }
            let text = row
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            pages.push(serde_json::json!({
                "url": row.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                "title": row.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "text": clip(text, 8000),
            }));
        }
    }

    let output = serde_json::json!({
        "pages": pages,
        "total": pages.len(),
    });

    let json_str = serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string());
    clip(&json_str, max_file_chars)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exa_client_new() {
        let client = ExaClient::new(Some("test-key".to_string()), "https://api.exa.ai", 30);
        assert_eq!(client.base_url, "https://api.exa.ai");
        assert_eq!(client.api_key.as_deref(), Some("test-key"));
    }

    #[test]
    fn test_exa_client_strip_trailing_slash() {
        let client = ExaClient::new(None, "https://api.exa.ai/", 30);
        assert_eq!(client.base_url, "https://api.exa.ai");
    }

    #[tokio::test]
    async fn test_web_search_empty_query() {
        let client = ExaClient::new(Some("key".to_string()), "https://api.exa.ai", 30);
        let result = web_search(&client, "", None, false, 20000).await;
        assert_eq!(result, "web_search requires non-empty query");
    }

    #[tokio::test]
    async fn test_web_search_whitespace_query() {
        let client = ExaClient::new(Some("key".to_string()), "https://api.exa.ai", 30);
        let result = web_search(&client, "   ", None, false, 20000).await;
        assert_eq!(result, "web_search requires non-empty query");
    }

    #[tokio::test]
    async fn test_web_search_no_api_key() {
        let client = ExaClient::new(None, "https://api.exa.ai", 30);
        let result = web_search(&client, "test query", None, false, 20000).await;
        assert!(result.contains("failed") || result.contains("EXA_API_KEY"));
    }

    #[tokio::test]
    async fn test_fetch_url_empty() {
        let client = ExaClient::new(Some("key".to_string()), "https://api.exa.ai", 30);
        let result = fetch_url(&client, &[], 20000).await;
        assert!(result.contains("requires at least one valid URL"));
    }

    #[tokio::test]
    async fn test_fetch_url_whitespace_only() {
        let client = ExaClient::new(Some("key".to_string()), "https://api.exa.ai", 30);
        let urls = vec!["  ".to_string(), "".to_string()];
        let result = fetch_url(&client, &urls, 20000).await;
        assert!(result.contains("requires at least one valid URL"));
    }

    #[tokio::test]
    async fn test_fetch_url_no_api_key() {
        let client = ExaClient::new(None, "https://api.exa.ai", 30);
        let urls = vec!["https://example.com".to_string()];
        let result = fetch_url(&client, &urls, 20000).await;
        assert!(result.contains("failed") || result.contains("EXA_API_KEY"));
    }
}
