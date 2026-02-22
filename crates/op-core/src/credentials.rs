use crate::error::OpResult;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Bundle of API keys for all supported providers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialBundle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openrouter_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cerebras_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exa_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voyage_api_key: Option<String>,
}

impl CredentialBundle {
    pub fn has_any(&self) -> bool {
        self.openai_api_key.is_some()
            || self.anthropic_api_key.is_some()
            || self.openrouter_api_key.is_some()
            || self.cerebras_api_key.is_some()
            || self.exa_api_key.is_some()
            || self.voyage_api_key.is_some()
    }

    /// Fill in missing fields from another bundle.
    pub fn merge_missing(&mut self, other: &CredentialBundle) {
        if self.openai_api_key.is_none() {
            self.openai_api_key = other.openai_api_key.clone();
        }
        if self.anthropic_api_key.is_none() {
            self.anthropic_api_key = other.anthropic_api_key.clone();
        }
        if self.openrouter_api_key.is_none() {
            self.openrouter_api_key = other.openrouter_api_key.clone();
        }
        if self.cerebras_api_key.is_none() {
            self.cerebras_api_key = other.cerebras_api_key.clone();
        }
        if self.exa_api_key.is_none() {
            self.exa_api_key = other.exa_api_key.clone();
        }
        if self.voyage_api_key.is_none() {
            self.voyage_api_key = other.voyage_api_key.clone();
        }
    }
}

/// Workspace-local credential store (.openplanter/credentials.json).
pub struct CredentialStore {
    credentials_path: PathBuf,
}

impl CredentialStore {
    pub fn new(workspace: &Path, session_root_dir: &str) -> Self {
        Self {
            credentials_path: workspace
                .join(session_root_dir)
                .join("credentials.json"),
        }
    }

    pub fn load(&self) -> OpResult<CredentialBundle> {
        if !self.credentials_path.exists() {
            return Ok(CredentialBundle::default());
        }
        let data = fs::read_to_string(&self.credentials_path)?;
        let bundle: CredentialBundle = serde_json::from_str(&data)?;
        Ok(bundle)
    }

    pub fn save(&self, creds: &CredentialBundle) -> OpResult<()> {
        if let Some(parent) = self.credentials_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(creds)?;
        fs::write(&self.credentials_path, data)?;
        // Set file permissions to 0o600 on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&self.credentials_path, perms)?;
        }
        Ok(())
    }
}

/// User-global credential store (~/.openplanter/credentials.json).
pub struct UserCredentialStore {
    credentials_path: PathBuf,
}

impl UserCredentialStore {
    pub fn new() -> Self {
        let home = dirs_path();
        Self {
            credentials_path: home.join(".openplanter").join("credentials.json"),
        }
    }

    pub fn load(&self) -> OpResult<CredentialBundle> {
        if !self.credentials_path.exists() {
            return Ok(CredentialBundle::default());
        }
        let data = fs::read_to_string(&self.credentials_path)?;
        let bundle: CredentialBundle = serde_json::from_str(&data)?;
        Ok(bundle)
    }

    pub fn save(&self, creds: &CredentialBundle) -> OpResult<()> {
        if let Some(parent) = self.credentials_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(creds)?;
        fs::write(&self.credentials_path, data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&self.credentials_path, perms)?;
        }
        Ok(())
    }
}

fn dirs_path() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Strip surrounding quotes from a value (matching Python's _strip_quotes).
pub fn strip_quotes(value: &str) -> &str {
    let v = value.trim();
    if (v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')) {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// Parse a .env file into a CredentialBundle.
pub fn parse_env_file(path: &Path) -> OpResult<CredentialBundle> {
    let content = fs::read_to_string(path)?;
    let mut bundle = CredentialBundle::default();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = strip_quotes(value.trim());
            match key {
                "OPENAI_API_KEY" | "OPENPLANTER_OPENAI_API_KEY" => {
                    bundle.openai_api_key = Some(value.to_string());
                }
                "ANTHROPIC_API_KEY" | "OPENPLANTER_ANTHROPIC_API_KEY" => {
                    bundle.anthropic_api_key = Some(value.to_string());
                }
                "OPENROUTER_API_KEY" | "OPENPLANTER_OPENROUTER_API_KEY" => {
                    bundle.openrouter_api_key = Some(value.to_string());
                }
                "CEREBRAS_API_KEY" | "OPENPLANTER_CEREBRAS_API_KEY" => {
                    bundle.cerebras_api_key = Some(value.to_string());
                }
                "EXA_API_KEY" | "OPENPLANTER_EXA_API_KEY" => {
                    bundle.exa_api_key = Some(value.to_string());
                }
                "VOYAGE_API_KEY" | "OPENPLANTER_VOYAGE_API_KEY" => {
                    bundle.voyage_api_key = Some(value.to_string());
                }
                _ => {}
            }
        }
    }
    Ok(bundle)
}

/// Load credentials from environment variables.
pub fn credentials_from_env() -> CredentialBundle {
    CredentialBundle {
        openai_api_key: std::env::var("OPENAI_API_KEY").ok().or_else(|| {
            std::env::var("OPENPLANTER_OPENAI_API_KEY").ok()
        }),
        anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok().or_else(|| {
            std::env::var("OPENPLANTER_ANTHROPIC_API_KEY").ok()
        }),
        openrouter_api_key: std::env::var("OPENROUTER_API_KEY").ok().or_else(|| {
            std::env::var("OPENPLANTER_OPENROUTER_API_KEY").ok()
        }),
        cerebras_api_key: std::env::var("CEREBRAS_API_KEY").ok().or_else(|| {
            std::env::var("OPENPLANTER_CEREBRAS_API_KEY").ok()
        }),
        exa_api_key: std::env::var("EXA_API_KEY").ok().or_else(|| {
            std::env::var("OPENPLANTER_EXA_API_KEY").ok()
        }),
        voyage_api_key: std::env::var("VOYAGE_API_KEY").ok().or_else(|| {
            std::env::var("OPENPLANTER_VOYAGE_API_KEY").ok()
        }),
    }
}

/// Discover .env file candidates in workspace and parent directories.
pub fn discover_env_candidates(workspace: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut current = Some(workspace.to_path_buf());
    while let Some(dir) = current {
        let env_path = dir.join(".env");
        if env_path.exists() {
            candidates.push(env_path);
        }
        current = dir.parent().map(|p| p.to_path_buf());
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_credential_bundle_default() {
        let bundle = CredentialBundle::default();
        assert!(!bundle.has_any());
    }

    #[test]
    fn test_credential_bundle_has_any() {
        let mut bundle = CredentialBundle::default();
        bundle.openai_api_key = Some("sk-test".into());
        assert!(bundle.has_any());
    }

    #[test]
    fn test_merge_missing() {
        let mut a = CredentialBundle::default();
        a.openai_api_key = Some("existing".into());

        let b = CredentialBundle {
            openai_api_key: Some("should_not_override".into()),
            anthropic_api_key: Some("new_key".into()),
            ..Default::default()
        };

        a.merge_missing(&b);
        assert_eq!(a.openai_api_key.as_deref(), Some("existing"));
        assert_eq!(a.anthropic_api_key.as_deref(), Some("new_key"));
    }

    #[test]
    fn test_strip_quotes() {
        assert_eq!(strip_quotes("\"hello\""), "hello");
        assert_eq!(strip_quotes("'hello'"), "hello");
        assert_eq!(strip_quotes("hello"), "hello");
        assert_eq!(strip_quotes("  \"spaced\"  "), "spaced");
    }

    #[test]
    fn test_parse_env_file() {
        let dir = std::env::temp_dir().join("op_test_env");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(".env");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "OPENAI_API_KEY=sk-test123").unwrap();
        writeln!(f, "# comment").unwrap();
        writeln!(f, "ANTHROPIC_API_KEY=\"ant-key\"").unwrap();
        writeln!(f, "").unwrap();
        drop(f);

        let bundle = parse_env_file(&path).unwrap();
        assert_eq!(bundle.openai_api_key.as_deref(), Some("sk-test123"));
        assert_eq!(bundle.anthropic_api_key.as_deref(), Some("ant-key"));
        assert!(bundle.cerebras_api_key.is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
