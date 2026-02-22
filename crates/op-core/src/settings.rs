use crate::error::OpResult;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const VALID_REASONING_EFFORTS: &[&str] = &["low", "medium", "high"];

/// Persistent workspace settings (.openplanter/settings.json).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistentSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model_openai: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model_anthropic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model_openrouter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model_cerebras: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model_ollama: Option<String>,
}

impl PersistentSettings {
    /// Get default model for a specific provider, falling back to the general default.
    pub fn default_model_for_provider(&self, provider: &str) -> Option<&str> {
        let specific = match provider {
            "openai" => self.default_model_openai.as_deref(),
            "anthropic" => self.default_model_anthropic.as_deref(),
            "openrouter" => self.default_model_openrouter.as_deref(),
            "cerebras" => self.default_model_cerebras.as_deref(),
            "ollama" => self.default_model_ollama.as_deref(),
            _ => None,
        };
        specific.or(self.default_model.as_deref())
    }

    /// Return a cleaned/validated copy.
    pub fn normalized(&self) -> Self {
        Self {
            default_model: self.default_model.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            default_reasoning_effort: normalize_reasoning_effort(self.default_reasoning_effort.as_deref()),
            default_model_openai: self.default_model_openai.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            default_model_anthropic: self.default_model_anthropic.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            default_model_openrouter: self.default_model_openrouter.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            default_model_cerebras: self.default_model_cerebras.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            default_model_ollama: self.default_model_ollama.as_deref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        }
    }
}

/// Validate and normalize a reasoning effort value.
pub fn normalize_reasoning_effort(value: Option<&str>) -> Option<String> {
    let valid: HashSet<&str> = VALID_REASONING_EFFORTS.iter().copied().collect();
    value
        .map(|v| v.trim().to_lowercase())
        .filter(|v| valid.contains(v.as_str()))
}

/// Persistent settings store.
pub struct SettingsStore {
    settings_path: PathBuf,
}

impl SettingsStore {
    pub fn new(workspace: &Path, session_root_dir: &str) -> Self {
        Self {
            settings_path: workspace.join(session_root_dir).join("settings.json"),
        }
    }

    pub fn load(&self) -> OpResult<PersistentSettings> {
        if !self.settings_path.exists() {
            return Ok(PersistentSettings::default());
        }
        let data = fs::read_to_string(&self.settings_path)?;
        let settings: PersistentSettings = serde_json::from_str(&data)?;
        Ok(settings.normalized())
    }

    pub fn save(&self, settings: &PersistentSettings) -> OpResult<()> {
        if let Some(parent) = self.settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&settings.normalized())?;
        fs::write(&self.settings_path, data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_model_for_provider() {
        let settings = PersistentSettings {
            default_model: Some("gpt-5.2".into()),
            default_model_anthropic: Some("claude-opus-4-6".into()),
            ..Default::default()
        };
        assert_eq!(
            settings.default_model_for_provider("anthropic"),
            Some("claude-opus-4-6")
        );
        assert_eq!(
            settings.default_model_for_provider("openai"),
            Some("gpt-5.2")
        );
        assert_eq!(
            settings.default_model_for_provider("cerebras"),
            Some("gpt-5.2")
        );
    }

    #[test]
    fn test_normalize_reasoning_effort() {
        assert_eq!(
            normalize_reasoning_effort(Some("high")),
            Some("high".into())
        );
        assert_eq!(
            normalize_reasoning_effort(Some("LOW")),
            Some("low".into())
        );
        assert_eq!(normalize_reasoning_effort(Some("invalid")), None);
        assert_eq!(normalize_reasoning_effort(None), None);
    }

    #[test]
    fn test_normalized() {
        let s = PersistentSettings {
            default_model: Some("  gpt-5.2  ".into()),
            default_reasoning_effort: Some("HIGH".into()),
            default_model_openai: Some("".into()),
            ..Default::default()
        };
        let n = s.normalized();
        assert_eq!(n.default_model.as_deref(), Some("gpt-5.2"));
        assert_eq!(n.default_reasoning_effort.as_deref(), Some("high"));
        assert!(n.default_model_openai.is_none()); // empty string removed
    }

    #[test]
    fn test_settings_store_round_trip() {
        let dir = std::env::temp_dir().join("op_test_settings");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let store = SettingsStore::new(&dir, ".openplanter");
        let settings = PersistentSettings {
            default_model: Some("test-model".into()),
            ..Default::default()
        };
        store.save(&settings).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.default_model.as_deref(), Some("test-model"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
