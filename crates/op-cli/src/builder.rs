//! Engine construction and model listing helpers.
//!
//! Port of Python `agent/builder.py`. Builds [`RLMEngine`] instances from
//! [`AgentConfig`], infers providers from model names, and lists available
//! models per-provider.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use op_core::{AgentConfig, OpError, OpResult};
use op_engine::engine::ModelFactory;
use op_engine::RLMEngine;
use op_model::{AnthropicModel, EchoFallbackModel, LlmModel, OpenAiModel};
use op_tools::WorkspaceTools;
use regex::Regex;

// ---------------------------------------------------------------------------
// Provider inference
// ---------------------------------------------------------------------------

/// Return the likely provider name for the given model string, or `None` if
/// the model name is ambiguous.
///
/// Matches Python's `infer_provider_for_model`.
pub fn infer_provider_for_model(model: &str) -> Option<&'static str> {
    // OpenRouter: contains a slash (org/model format)
    if model.contains('/') {
        return Some("openrouter");
    }

    // Anthropic: starts with "claude"
    let anthropic_re = Regex::new(r"(?i)^claude").expect("valid regex");
    if anthropic_re.is_match(model) {
        return Some("anthropic");
    }

    // Cerebras: starts with llama.*cerebras, qwen-3, gpt-oss, or zai-glm
    let cerebras_re =
        Regex::new(r"(?i)^(llama.*cerebras|qwen-3|gpt-oss|zai-glm)").expect("valid regex");
    if cerebras_re.is_match(model) {
        return Some("cerebras");
    }

    // OpenAI: starts with gpt, o1/o2/o3/o4 prefix, chatgpt, dall-e, tts-, whisper
    let openai_re =
        Regex::new(r"(?i)^(gpt|o[1-4]-|o[1-4]$|chatgpt|dall-e|tts-|whisper)").expect("valid regex");
    if openai_re.is_match(model) {
        return Some("openai");
    }

    // Ollama: common local model families (but not qwen-3 which is cerebras above)
    let ollama_re = Regex::new(
        r"(?i)^(llama|mistral|gemma|phi|codellama|deepseek|vicuna|tinyllama|neural-chat|dolphin|wizardlm|orca|nous-hermes|command-r|qwen)",
    )
    .expect("valid regex");
    if ollama_re.is_match(model) && !model.to_lowercase().starts_with("qwen-3") {
        return Some("ollama");
    }

    None
}

// ---------------------------------------------------------------------------
// Model-provider validation
// ---------------------------------------------------------------------------

/// Return an error if the model name clearly belongs to a different provider.
fn validate_model_provider(model_name: &str, provider: &str) -> OpResult<()> {
    if provider == "openrouter" {
        return Ok(());
    }
    let inferred = infer_provider_for_model(model_name);
    match inferred {
        None | Some("openrouter") => Ok(()),
        Some(p) if p == provider => Ok(()),
        Some(p) => Err(OpError::model(format!(
            "Model '{}' belongs to provider '{}', not '{}'. \
             Use --provider {} or pick a model that matches the current provider.",
            model_name, p, provider, p
        ))),
    }
}

// ---------------------------------------------------------------------------
// Model listing
// ---------------------------------------------------------------------------

/// Metadata for a single model returned by a provider listing endpoint.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub created_ts: i64,
}

/// Fetch the list of models from a given provider.
///
/// Matches Python's `_fetch_models_for_provider`.
pub async fn fetch_models_for_provider(
    cfg: &AgentConfig,
    provider: &str,
) -> OpResult<Vec<ModelInfo>> {
    match provider {
        "openai" => {
            let key = cfg
                .openai_api_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or_else(|| OpError::model("OpenAI key not configured."))?;
            fetch_openai_compatible_models(key, &cfg.openai_base_url).await
        }
        "anthropic" => {
            let key = cfg
                .anthropic_api_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or_else(|| OpError::model("Anthropic key not configured."))?;
            fetch_anthropic_models(key, &cfg.anthropic_base_url).await
        }
        "openrouter" => {
            let key = cfg
                .openrouter_api_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or_else(|| OpError::model("OpenRouter key not configured."))?;
            fetch_openai_compatible_models(key, &cfg.openrouter_base_url).await
        }
        "cerebras" => {
            let key = cfg
                .cerebras_api_key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or_else(|| OpError::model("Cerebras key not configured."))?;
            fetch_openai_compatible_models(key, &cfg.cerebras_base_url).await
        }
        "ollama" => fetch_ollama_models(&cfg.ollama_base_url).await,
        other => Err(OpError::model(format!("Unknown provider: {}", other))),
    }
}

/// Fetch models from an OpenAI-compatible /models endpoint.
async fn fetch_openai_compatible_models(
    api_key: &str,
    base_url: &str,
) -> OpResult<Vec<ModelInfo>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .map_err(|e| OpError::http(format!("Failed to list models: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(OpError::http(format!(
            "Model listing failed ({}): {}",
            status, body
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| OpError::http(format!("Invalid JSON from models endpoint: {}", e)))?;

    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    let mut models: Vec<ModelInfo> = data
        .iter()
        .map(|entry| {
            let id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_ts = entry
                .get("created")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            ModelInfo { id, created_ts }
        })
        .collect();

    // Sort newest first (descending by created_ts).
    models.sort_by(|a, b| b.created_ts.cmp(&a.created_ts));
    Ok(models)
}

/// Fetch models from an Anthropic-compatible endpoint.
async fn fetch_anthropic_models(api_key: &str, base_url: &str) -> OpResult<Vec<ModelInfo>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| OpError::http(format!("Failed to list Anthropic models: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(OpError::http(format!(
            "Anthropic model listing failed ({}): {}",
            status, body
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| OpError::http(format!("Invalid JSON from Anthropic: {}", e)))?;

    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    let mut models: Vec<ModelInfo> = data
        .iter()
        .map(|entry| {
            let id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_ts = entry
                .get("created_at")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            ModelInfo { id, created_ts }
        })
        .collect();

    models.sort_by(|a, b| b.created_ts.cmp(&a.created_ts));
    Ok(models)
}

/// Fetch models from an Ollama endpoint.
async fn fetch_ollama_models(base_url: &str) -> OpResult<Vec<ModelInfo>> {
    // Ollama uses /api/tags (or /v1/models for the OpenAI-compat layer).
    // Try the /v1/models first since base_url likely ends in /v1.
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| OpError::http(format!("Failed to list Ollama models: {}", e)))?;

    if !resp.status().is_success() {
        // Fall back to the native Ollama /api/tags endpoint.
        let alt_url = base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .to_string()
            + "/api/tags";
        let resp2 = client
            .get(&alt_url)
            .send()
            .await
            .map_err(|e| OpError::http(format!("Failed to list Ollama models (fallback): {}", e)))?;

        if !resp2.status().is_success() {
            let status = resp2.status();
            let body = resp2.text().await.unwrap_or_default();
            return Err(OpError::http(format!(
                "Ollama model listing failed ({}): {}",
                status, body
            )));
        }

        let body: serde_json::Value = resp2
            .json()
            .await
            .map_err(|e| OpError::http(format!("Invalid JSON from Ollama: {}", e)))?;

        let data = body
            .get("models")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let mut models: Vec<ModelInfo> = data
            .iter()
            .map(|entry| {
                let id = entry
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                ModelInfo { id, created_ts: 0 }
            })
            .collect();

        models.sort_by(|a, b| a.id.cmp(&b.id));
        return Ok(models);
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| OpError::http(format!("Invalid JSON from Ollama: {}", e)))?;

    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    let mut models: Vec<ModelInfo> = data
        .iter()
        .map(|entry| {
            let id = entry
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_ts = entry
                .get("created")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            ModelInfo { id, created_ts }
        })
        .collect();

    models.sort_by(|a, b| b.created_ts.cmp(&a.created_ts));
    Ok(models)
}

// ---------------------------------------------------------------------------
// Resolve model name
// ---------------------------------------------------------------------------

/// Resolve the final model name, handling "newest" auto-selection and
/// falling back to the provider default.
async fn resolve_model_name(cfg: &AgentConfig) -> OpResult<String> {
    let selected = cfg.model.trim();
    if !selected.is_empty() && selected.to_lowercase() != "newest" {
        return Ok(selected.to_string());
    }
    if selected.to_lowercase() == "newest" {
        let models = fetch_models_for_provider(cfg, &cfg.provider).await.map_err(|e| {
            OpError::model(format!(
                "Failed to resolve newest model for provider '{}': {}",
                cfg.provider, e
            ))
        })?;
        if models.is_empty() {
            return Err(OpError::model(format!(
                "No models returned for provider '{}'.",
                cfg.provider
            )));
        }
        return Ok(models[0].id.clone());
    }
    // No model specified — use provider default.
    let defaults = op_core::config::provider_default_models();
    let default = defaults
        .get(cfg.provider.as_str())
        .copied()
        .unwrap_or("claude-opus-4-6");
    Ok(default.to_string())
}

// ---------------------------------------------------------------------------
// Model factory
// ---------------------------------------------------------------------------

/// Build a model factory closure that can create models by name and optional
/// reasoning effort. Returns `None` if no API keys are configured.
///
/// Matches Python's `build_model_factory`.
pub fn build_model_factory(cfg: &AgentConfig) -> Option<ModelFactory> {
    let has_any_key = cfg.openai_api_key.is_some()
        || cfg.anthropic_api_key.is_some()
        || cfg.openrouter_api_key.is_some()
        || cfg.cerebras_api_key.is_some()
        || !cfg.ollama_base_url.is_empty();

    if !has_any_key {
        return None;
    }

    let cfg = cfg.clone();
    Some(Arc::new(move |model_name: &str, reasoning_effort: Option<&str>| -> Box<dyn LlmModel> {
        let provider = infer_provider_for_model(model_name);
        let effort = reasoning_effort
            .map(String::from)
            .or_else(|| cfg.reasoning_effort.clone());

        match provider {
            Some("anthropic") if cfg.anthropic_api_key.is_some() => {
                let mut m = AnthropicModel::new(
                    model_name.to_string(),
                    cfg.anthropic_api_key.clone().unwrap(),
                );
                m.base_url = cfg.anthropic_base_url.clone();
                m.reasoning_effort = effort;
                Box::new(m)
            }
            Some("openrouter") if cfg.openrouter_api_key.is_some() => {
                let mut m = OpenAiModel::new(
                    model_name.to_string(),
                    cfg.openrouter_api_key.clone().unwrap(),
                );
                m.base_url = cfg.openrouter_base_url.clone();
                m.reasoning_effort = effort;
                m.extra_headers = vec![
                    ("HTTP-Referer".to_string(), "https://github.com/openplanter".to_string()),
                    ("X-Title".to_string(), "OpenPlanter".to_string()),
                ];
                Box::new(m)
            }
            Some("cerebras") if cfg.cerebras_api_key.is_some() => {
                let mut m = OpenAiModel::new(
                    model_name.to_string(),
                    cfg.cerebras_api_key.clone().unwrap(),
                );
                m.base_url = cfg.cerebras_base_url.clone();
                m.reasoning_effort = effort;
                Box::new(m)
            }
            Some("ollama") => {
                let mut m = OpenAiModel::new(model_name.to_string(), "ollama".to_string());
                m.base_url = cfg.ollama_base_url.clone();
                m.reasoning_effort = effort;
                m.first_byte_timeout = 120.0;
                m.strict_tools = false;
                Box::new(m)
            }
            _ if cfg.openai_api_key.is_some() => {
                // Default to OpenAI for unknown providers when an OpenAI key exists.
                let mut m = OpenAiModel::new(
                    model_name.to_string(),
                    cfg.openai_api_key.clone().unwrap(),
                );
                m.base_url = cfg.openai_base_url.clone();
                m.reasoning_effort = effort;
                Box::new(m)
            }
            _ => {
                // Fallback echo model.
                Box::new(EchoFallbackModel::new(format!(
                    "No API key available for model '{}' (provider={:?})",
                    model_name, provider
                )))
            }
        }
    }))
}

// ---------------------------------------------------------------------------
// Engine construction
// ---------------------------------------------------------------------------

/// Build a complete [`RLMEngine`] from the given configuration.
///
/// Matches Python's `build_engine`.
pub async fn build_engine(cfg: &AgentConfig) -> RLMEngine {
    let tools = WorkspaceTools::new(Path::new(&cfg.workspace));

    let model_name = match resolve_model_name(cfg).await {
        Ok(name) => name,
        Err(e) => {
            let model = EchoFallbackModel::new(e.to_string());
            let mut engine = RLMEngine::new(Box::new(model), tools, cfg.clone());
            engine.model_factory = build_model_factory(cfg);
            return engine;
        }
    };

    if let Err(e) = validate_model_provider(&model_name, &cfg.provider) {
        let model = EchoFallbackModel::new(e.to_string());
        let mut engine = RLMEngine::new(Box::new(model), tools, cfg.clone());
        engine.model_factory = build_model_factory(cfg);
        return engine;
    }

    let model: Box<dyn LlmModel> = match cfg.provider.as_str() {
        "openai" if cfg.openai_api_key.is_some() => {
            let mut m = OpenAiModel::new(
                model_name.clone(),
                cfg.openai_api_key.clone().unwrap(),
            );
            m.base_url = cfg.openai_base_url.clone();
            m.reasoning_effort = cfg.reasoning_effort.clone();
            Box::new(m)
        }
        "openrouter" if cfg.openrouter_api_key.is_some() => {
            let mut m = OpenAiModel::new(
                model_name.clone(),
                cfg.openrouter_api_key.clone().unwrap(),
            );
            m.base_url = cfg.openrouter_base_url.clone();
            m.reasoning_effort = cfg.reasoning_effort.clone();
            m.extra_headers = vec![
                ("HTTP-Referer".to_string(), "https://github.com/openplanter".to_string()),
                ("X-Title".to_string(), "OpenPlanter".to_string()),
            ];
            Box::new(m)
        }
        "cerebras" if cfg.cerebras_api_key.is_some() => {
            let mut m = OpenAiModel::new(
                model_name.clone(),
                cfg.cerebras_api_key.clone().unwrap(),
            );
            m.base_url = cfg.cerebras_base_url.clone();
            m.reasoning_effort = cfg.reasoning_effort.clone();
            Box::new(m)
        }
        "ollama" => {
            let mut m = OpenAiModel::new(model_name.clone(), "ollama".to_string());
            m.base_url = cfg.ollama_base_url.clone();
            m.reasoning_effort = cfg.reasoning_effort.clone();
            m.first_byte_timeout = 120.0;
            m.strict_tools = false;
            Box::new(m)
        }
        "anthropic" if cfg.anthropic_api_key.is_some() => {
            let mut m = AnthropicModel::new(
                model_name.clone(),
                cfg.anthropic_api_key.clone().unwrap(),
            );
            m.base_url = cfg.anthropic_base_url.clone();
            m.reasoning_effort = cfg.reasoning_effort.clone();
            Box::new(m)
        }
        _ => Box::new(EchoFallbackModel::default()),
    };

    let mut engine = RLMEngine::new(model, tools, cfg.clone());
    engine.model_factory = build_model_factory(cfg);
    engine
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_provider_anthropic() {
        assert_eq!(infer_provider_for_model("claude-opus-4-6"), Some("anthropic"));
        assert_eq!(infer_provider_for_model("claude-sonnet-4-5"), Some("anthropic"));
        assert_eq!(infer_provider_for_model("Claude-3-haiku"), Some("anthropic"));
    }

    #[test]
    fn test_infer_provider_openai() {
        assert_eq!(infer_provider_for_model("gpt-5.2"), Some("openai"));
        assert_eq!(infer_provider_for_model("gpt-4o"), Some("openai"));
        assert_eq!(infer_provider_for_model("o1-preview"), Some("openai"));
        assert_eq!(infer_provider_for_model("o3-mini"), Some("openai"));
        assert_eq!(infer_provider_for_model("o4-mini"), Some("openai"));
    }

    #[test]
    fn test_infer_provider_cerebras() {
        assert_eq!(
            infer_provider_for_model("qwen-3-235b-a22b-instruct-2507"),
            Some("cerebras")
        );
    }

    #[test]
    fn test_infer_provider_ollama() {
        assert_eq!(infer_provider_for_model("llama3.2"), Some("ollama"));
        assert_eq!(infer_provider_for_model("mistral"), Some("ollama"));
        assert_eq!(infer_provider_for_model("deepseek-coder"), Some("ollama"));
    }

    #[test]
    fn test_infer_provider_openrouter() {
        assert_eq!(
            infer_provider_for_model("anthropic/claude-sonnet-4-5"),
            Some("openrouter")
        );
        assert_eq!(
            infer_provider_for_model("meta/llama-3-70b"),
            Some("openrouter")
        );
    }

    #[test]
    fn test_infer_provider_unknown() {
        assert_eq!(infer_provider_for_model("some-custom-model"), None);
    }

    #[test]
    fn test_validate_model_provider_ok() {
        assert!(validate_model_provider("gpt-5.2", "openai").is_ok());
        assert!(validate_model_provider("claude-opus-4-6", "anthropic").is_ok());
        // OpenRouter accepts anything.
        assert!(validate_model_provider("gpt-5.2", "openrouter").is_ok());
        // Unknown model => no opinion => ok.
        assert!(validate_model_provider("custom-model", "openai").is_ok());
    }

    #[test]
    fn test_validate_model_provider_mismatch() {
        assert!(validate_model_provider("gpt-5.2", "anthropic").is_err());
        assert!(validate_model_provider("claude-opus-4-6", "openai").is_err());
    }
}
