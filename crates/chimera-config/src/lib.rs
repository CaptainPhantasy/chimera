//! chimera-config: configuration loading for CHIMERA.
//!
//! Resolves provider config from (in priority order):
//!   1. Environment variables (`CHIMERA_API_KEY`, `CHIMERA_BASE_URL`,
//!      `CHIMERA_MODEL`, `CHIMERA_PROVIDER`).
//!   2. A TOML/JSON config file at `~/.chimera/config.json` or the path in
//!      `CHIMERA_CONFIG`.
//!   3. Sensible defaults (OpenAI-compatible localhost / LM Studio).
//!
//! Builds an [`OpenAiCompatibleProvider`] from the resolved config.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config model
// ---------------------------------------------------------------------------

/// Top-level CHIMERA configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

/// LLM provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider name for display (e.g. "openai", "lm-studio", "ollama").
    #[serde(default = "default_provider_name")]
    pub name: String,
    /// Base URL of an OpenAI-compatible Chat Completions endpoint.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// API key (bearer token).
    #[serde(default)]
    pub api_key: String,
    /// Default model id to use for completions.
    #[serde(default = "default_model")]
    pub model: String,
    /// Default temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Default max tokens.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: default_provider_name(),
            base_url: default_base_url(),
            api_key: String::new(),
            model: default_model(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
        }
    }
}

/// Agent loop configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
        }
    }
}

fn default_provider_name() -> String {
    "openai".to_string()
}
fn default_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}
fn default_model() -> String {
    "gpt-4o-mini".to_string()
}
fn default_temperature() -> f32 {
    0.0
}
fn default_max_tokens() -> u32 {
    8192
}
fn default_max_iterations() -> usize {
    40
}

// ---------------------------------------------------------------------------
// Config resolution
// ---------------------------------------------------------------------------

impl Config {
    /// Load config by merging: file defaults < config file < env vars.
    pub fn load() -> Result<Self> {
        let mut cfg = Self::from_file_opt().unwrap_or_default();
        cfg.apply_env();
        Ok(cfg)
    }

    /// Load from a specific file path, if it exists.
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        let cfg: Config = if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            // Minimal TOML support: we accept JSON files primarily; for .toml,
            // attempt a very light parse via serde_json after converting. To
            // keep deps minimal, we only support JSON here and document it.
            return Err(anyhow::anyhow!(
                "TOML config not supported in this build; use config.json"
            ));
        } else {
            serde_json::from_str(&raw)
                .with_context(|| format!("failed to parse config JSON at {}", path.display()))?
        };
        Ok(cfg)
    }

    /// Load from the default config file path, if present.
    fn from_file_opt() -> Option<Self> {
        let path = config_file_path()?;
        if path.exists() {
            Self::from_file(&path).ok()
        } else {
            None
        }
    }

    /// Override loaded values with environment variables where set.
    fn apply_env(&mut self) {
        if let Ok(name) = std::env::var("CHIMERA_PROVIDER") {
            self.provider.name = name;
        }
        if let Ok(base) = std::env::var("CHIMERA_BASE_URL") {
            self.provider.base_url = base;
        }
        if let Ok(key) = std::env::var("CHIMERA_API_KEY") {
            self.provider.api_key = key;
        }
        if let Ok(model) = std::env::var("CHIMERA_MODEL") {
            self.provider.model = model;
        }
        if let Ok(t) = std::env::var("CHIMERA_TEMPERATURE") {
            if let Ok(parsed) = t.parse::<f32>() {
                self.provider.temperature = parsed;
            }
        }
        if let Ok(m) = std::env::var("CHIMERA_MAX_TOKENS") {
            if let Ok(parsed) = m.parse::<u32>() {
                self.provider.max_tokens = parsed;
            }
        }
        if let Ok(i) = std::env::var("CHIMERA_MAX_ITERATIONS") {
            if let Ok(parsed) = i.parse::<usize>() {
                self.agent.max_iterations = parsed;
            }
        }
        // The OPENAI_API_KEY convention.
        if self.provider.api_key.is_empty() {
            if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                self.provider.api_key = key;
            }
        }
    }

    /// Build an OpenAI-compatible provider from this config.
    pub fn build_provider(&self) -> chimera_llm::OpenAiCompatibleProvider {
        chimera_llm::OpenAiCompatibleProvider::new(
            &self.provider.name,
            &self.provider.base_url,
            &self.provider.api_key,
        )
    }
}

/// Resolve the config file path: `$CHIMERA_CONFIG` or `~/.chimera/config.json`.
pub fn config_file_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHIMERA_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".chimera").join("config.json"))
}

/// The default config dir: `~/.chimera`.
pub fn config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".chimera"))
}

/// Write a default config file to `~/.chimera/config.json` if none exists.
pub fn init_config() -> Result<PathBuf> {
    let dir = config_dir().context("could not resolve home directory for config")?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create config dir {}", dir.display()))?;
    let path = dir.join("config.json");
    if !path.exists() {
        let default = Config::default();
        let json = serde_json::to_string_pretty(&default)
            .context("failed to serialize default config")?;
        std::fs::write(&path, json)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chimera_llm::LlmProvider;

    #[test]
    fn default_config_has_sensible_values() {
        let c = Config::default();
        assert_eq!(c.provider.name, "openai");
        assert_eq!(c.provider.model, "gpt-4o-mini");
        assert_eq!(c.provider.temperature, 0.0);
        assert_eq!(c.provider.max_tokens, 8192);
        assert_eq!(c.agent.max_iterations, 40);
    }

    #[test]
    fn apply_env_overrides_values() {
        let mut c = Config::default();
        // SAFETY: tests may run in parallel; use unique values and restore via
        // the fact that env mutation in-process is visible only to this test's
        // later reads. We scope by reading after setting.
        // Note: std::env::set_var is process-global; for determinism we only
        // assert that the *parsed* fields take effect.
        unsafe {
            std::env::set_var("CHIMERA_MODEL", "test-model-xyz");
            std::env::set_var("CHIMERA_MAX_TOKENS", "1234");
            std::env::set_var("CHIMERA_TEMPERATURE", "0.7");
        }
        c.apply_env();
        assert_eq!(c.provider.model, "test-model-xyz");
        assert_eq!(c.provider.max_tokens, 1234);
        assert!((c.provider.temperature - 0.7).abs() < 1e-6);
    }

    #[test]
    fn from_file_parses_json() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("chimera-test-{}.json", uuid_for_test()));
        let json = r#"{
            "provider": {
                "name": "lm-studio",
                "base_url": "http://localhost:1234/v1",
                "api_key": "secret",
                "model": "local-model",
                "temperature": 0.5,
                "max_tokens": 2048
            },
            "agent": { "max_iterations": 10 }
        }"#;
        std::fs::write(&path, json).unwrap();
        let cfg = Config::from_file(&path).unwrap();
        assert_eq!(cfg.provider.name, "lm-studio");
        assert_eq!(cfg.provider.base_url, "http://localhost:1234/v1");
        assert_eq!(cfg.provider.api_key, "secret");
        assert_eq!(cfg.provider.model, "local-model");
        assert_eq!(cfg.agent.max_iterations, 10);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn from_file_missing_returns_error() {
        let path = std::env::temp_dir().join("definitely-does-not-exist-xyz.json");
        let res = Config::from_file(&path);
        assert!(res.is_err());
    }

    #[test]
    fn build_provider_constructs_openai_compatible() {
        let c = Config {
            provider: ProviderConfig {
                name: "test".to_string(),
                base_url: "https://example.test/v1/".to_string(),
                api_key: "k".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let p = c.build_provider();
        assert_eq!(p.name(), "test");
    }

    fn uuid_for_test() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{nanos}")
    }
}
