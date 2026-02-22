use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Default models per provider, matching Python's PROVIDER_DEFAULT_MODELS.
pub fn provider_default_models() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("openai", "gpt-5.2");
    m.insert("anthropic", "claude-opus-4-6");
    m.insert("openrouter", "anthropic/claude-sonnet-4-5");
    m.insert("cerebras", "qwen-3-235b-a22b-instruct-2507");
    m.insert("ollama", "llama3.2");
    m
}

/// Agent configuration, loaded from environment variables with OPENPLANTER_ prefix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub workspace: PathBuf,
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
    // Legacy aliases
    pub base_url: String,
    pub api_key: Option<String>,
    // Per-provider base URLs
    pub openai_base_url: String,
    pub anthropic_base_url: String,
    pub openrouter_base_url: String,
    pub cerebras_base_url: String,
    pub ollama_base_url: String,
    pub exa_base_url: String,
    // Per-provider API keys
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub cerebras_api_key: Option<String>,
    pub exa_api_key: Option<String>,
    pub voyage_api_key: Option<String>,
    // Limits
    pub max_depth: u32,
    pub max_steps_per_call: u32,
    pub max_observation_chars: usize,
    pub command_timeout_sec: u64,
    pub shell: String,
    pub max_files_listed: usize,
    pub max_file_chars: usize,
    pub max_search_hits: usize,
    pub max_shell_output_chars: usize,
    pub session_root_dir: String,
    pub max_persisted_observations: usize,
    pub max_solve_seconds: u64,
    // Feature flags
    pub recursive: bool,
    pub min_subtask_depth: u32,
    pub acceptance_criteria: bool,
    pub max_plan_chars: usize,
    pub demo: bool,
}

impl AgentConfig {
    /// Load config from environment variables, matching Python's `from_env`.
    pub fn from_env(workspace: &Path) -> Self {
        let ws = workspace
            .to_path_buf()
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());

        let openai_api_key = env_or("OPENPLANTER_OPENAI_API_KEY", None)
            .or_else(|| env_or("OPENAI_API_KEY", None));
        let anthropic_api_key = env_or("OPENPLANTER_ANTHROPIC_API_KEY", None)
            .or_else(|| env_or("ANTHROPIC_API_KEY", None));
        let openrouter_api_key = env_or("OPENPLANTER_OPENROUTER_API_KEY", None)
            .or_else(|| env_or("OPENROUTER_API_KEY", None));
        let cerebras_api_key = env_or("OPENPLANTER_CEREBRAS_API_KEY", None)
            .or_else(|| env_or("CEREBRAS_API_KEY", None));
        let exa_api_key = env_or("OPENPLANTER_EXA_API_KEY", None)
            .or_else(|| env_or("EXA_API_KEY", None));
        let voyage_api_key = env_or("OPENPLANTER_VOYAGE_API_KEY", None)
            .or_else(|| env_or("VOYAGE_API_KEY", None));

        let openai_base_url = env_or(
            "OPENPLANTER_OPENAI_BASE_URL",
            Some("https://api.openai.com/v1"),
        )
        .or_else(|| env_or("OPENPLANTER_BASE_URL", Some("https://api.openai.com/v1")))
        .unwrap_or_else(|| "https://api.openai.com/v1".into());

        Self {
            workspace: ws,
            provider: env_str("OPENPLANTER_PROVIDER", "auto")
                .trim()
                .to_lowercase(),
            model: env_str("OPENPLANTER_MODEL", "claude-opus-4-6"),
            reasoning_effort: {
                let v = env_str("OPENPLANTER_REASONING_EFFORT", "high")
                    .trim()
                    .to_lowercase();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            base_url: openai_base_url.clone(),
            api_key: openai_api_key.clone(),
            openai_base_url,
            anthropic_base_url: env_str(
                "OPENPLANTER_ANTHROPIC_BASE_URL",
                "https://api.anthropic.com/v1",
            ),
            openrouter_base_url: env_str(
                "OPENPLANTER_OPENROUTER_BASE_URL",
                "https://openrouter.ai/api/v1",
            ),
            cerebras_base_url: env_str(
                "OPENPLANTER_CEREBRAS_BASE_URL",
                "https://api.cerebras.ai/v1",
            ),
            ollama_base_url: env_str(
                "OPENPLANTER_OLLAMA_BASE_URL",
                "http://localhost:11434/v1",
            ),
            exa_base_url: env_str("OPENPLANTER_EXA_BASE_URL", "https://api.exa.ai"),
            openai_api_key,
            anthropic_api_key,
            openrouter_api_key,
            cerebras_api_key,
            exa_api_key,
            voyage_api_key,
            max_depth: env_u32("OPENPLANTER_MAX_DEPTH", 4),
            max_steps_per_call: env_u32("OPENPLANTER_MAX_STEPS", 100),
            max_observation_chars: env_usize("OPENPLANTER_MAX_OBS_CHARS", 6000),
            command_timeout_sec: env_u64("OPENPLANTER_CMD_TIMEOUT", 45),
            shell: env_str("OPENPLANTER_SHELL", "/bin/sh"),
            max_files_listed: env_usize("OPENPLANTER_MAX_FILES", 400),
            max_file_chars: env_usize("OPENPLANTER_MAX_FILE_CHARS", 20000),
            max_search_hits: env_usize("OPENPLANTER_MAX_SEARCH_HITS", 200),
            max_shell_output_chars: env_usize("OPENPLANTER_MAX_SHELL_CHARS", 16000),
            session_root_dir: env_str("OPENPLANTER_SESSION_DIR", ".openplanter"),
            max_persisted_observations: env_usize("OPENPLANTER_MAX_PERSISTED_OBS", 400),
            max_solve_seconds: env_u64("OPENPLANTER_MAX_SOLVE_SECONDS", 0),
            recursive: env_bool("OPENPLANTER_RECURSIVE", true),
            min_subtask_depth: env_u32("OPENPLANTER_MIN_SUBTASK_DEPTH", 0),
            acceptance_criteria: env_bool("OPENPLANTER_ACCEPTANCE_CRITERIA", true),
            max_plan_chars: env_usize("OPENPLANTER_MAX_PLAN_CHARS", 40000),
            demo: env_bool("OPENPLANTER_DEMO", false),
        }
    }
}

fn env_or(key: &str, default: Option<&str>) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => default.map(|s| s.to_string()),
    }
}

fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_from_env_defaults() {
        // Clear any env vars that might interfere
        let cfg = AgentConfig::from_env(Path::new("/tmp/test_workspace"));
        assert_eq!(cfg.provider, "auto");
        assert_eq!(cfg.max_depth, 4);
        assert_eq!(cfg.max_steps_per_call, 100);
        assert!(cfg.recursive);
        assert!(cfg.acceptance_criteria);
        assert!(!cfg.demo);
        assert_eq!(cfg.command_timeout_sec, 45);
        assert_eq!(cfg.shell, "/bin/sh");
        assert_eq!(cfg.session_root_dir, ".openplanter");
    }

    #[test]
    fn test_provider_default_models() {
        let models = provider_default_models();
        assert_eq!(models["openai"], "gpt-5.2");
        assert_eq!(models["anthropic"], "claude-opus-4-6");
        assert_eq!(models["ollama"], "llama3.2");
    }
}
