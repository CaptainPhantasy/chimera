//! chimera-config: configuration loading for CHIMERA.
//!
//! Resolves provider config from (in priority order):
//!   1. Environment variables (`CHIMERA_API_KEY`, `CHIMERA_BASE_URL`,
//!      `CHIMERA_MODEL`, `CHIMERA_PROVIDER`).
//!   2. A JSON config file at `~/.chimera/config.json` or the path in
//!      `CHIMERA_CONFIG`.
//!   3. OpenAI defaults. Provider names are labels, not endpoint presets.
//!
//! Builds an [`OpenAiCompatibleProvider`] from the resolved config.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub mod connections;

// ---------------------------------------------------------------------------
// Config model
// ---------------------------------------------------------------------------

/// Top-level CHIMERA configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    /// Saved connections; the selected one remains in `provider` for compatibility.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub providers: std::collections::BTreeMap<String, ProviderConfig>,
    /// External editor used by the chat screen to compose long prompts.
    /// Empty falls back to `$VISUAL`, then `$EDITOR`, when the editor runs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub editor: String,
    /// Interface preferences.
    #[serde(default, skip_serializing_if = "UiConfig::is_default")]
    pub ui: UiConfig,
}

/// Interface preferences stored alongside provider settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    /// Preferred terminal palette: "dark" or "light".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub theme: String,
}

impl UiConfig {
    fn is_default(&self) -> bool {
        self.theme.is_empty()
    }
}

/// Persist the preferred palette ("dark" or "light") to the config file.
///
/// A missing config file is created from defaults; an unreadable file is an
/// error rather than a silent overwrite that could drop provider secrets.
pub fn set_theme(theme: &str) -> Result<()> {
    let path = config_file_path().context("no config file path is available")?;
    let mut config = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Config>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(error) => return Err(error).context("could not read the config file"),
    };
    config.ui.theme = theme.to_string();
    let bytes = serde_json::to_vec_pretty(&config)?;
    connections::save_private(&path, &bytes)
}

/// LLM provider configuration.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Maximum total duration of one model request.
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    /// Maximum wait between response fragments.
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_response_bytes")]
    pub max_response_bytes: usize,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .field("max_response_bytes", &self.max_response_bytes)
            .finish()
    }
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
            request_timeout_secs: default_request_timeout(),
            idle_timeout_secs: default_idle_timeout(),
            max_response_bytes: default_response_bytes(),
        }
    }
}

/// Agent loop configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

fn default_request_timeout() -> u64 {
    120
}
fn default_idle_timeout() -> u64 {
    30
}
fn default_response_bytes() -> usize {
    8 * 1024 * 1024
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

impl ProviderConfig {
    pub fn is_local(&self) -> Result<bool> {
        Ok(chimera_llm::base_url_is_local(&self.base_url)?)
    }
}

impl Config {
    /// The editor used to compose long prompts: the saved `editor` value,
    /// else `$VISUAL`, else `$EDITOR`. `None` when nothing is configured.
    pub fn editor(&self) -> Option<String> {
        self.editor_with(|key| std::env::var(key).ok())
    }

    /// [`Config::editor`] with an injectable environment for tests.
    pub fn editor_with(&self, env: impl Fn(&str) -> Option<String>) -> Option<String> {
        let configured = self.editor.trim();
        if !configured.is_empty() {
            return Some(configured.to_string());
        }
        for key in ["VISUAL", "EDITOR"] {
            match env(key) {
                Some(value) if !value.trim().is_empty() => return Some(value),
                _ => {}
            }
        }
        None
    }

    /// Priority: CHIMERA_* environment, file values, defaults. OPENAI_API_KEY
    /// is a fallback only when neither CHIMERA_API_KEY nor a file key is set.
    pub fn load() -> Result<Self> {
        let mut env = std::collections::HashMap::new();
        for key in [
            "CHIMERA_CONFIG",
            "CHIMERA_PROVIDER",
            "CHIMERA_BASE_URL",
            "CHIMERA_API_KEY",
            "CHIMERA_MODEL",
            "CHIMERA_TEMPERATURE",
            "CHIMERA_MAX_TOKENS",
            "CHIMERA_MAX_ITERATIONS",
            "CHIMERA_REQUEST_TIMEOUT_SECS",
            "CHIMERA_IDLE_TIMEOUT_SECS",
            "CHIMERA_MAX_RESPONSE_BYTES",
            "OPENAI_API_KEY",
        ] {
            match std::env::var(key) {
                Ok(value) => {
                    env.insert(key.to_string(), value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(_) => anyhow::bail!("{key} must be valid UTF-8"),
            }
        }
        let required = env.contains_key("CHIMERA_CONFIG");
        if required && env["CHIMERA_CONFIG"].trim().is_empty() {
            anyhow::bail!("CHIMERA_CONFIG must not be empty");
        }
        let path = config_file_path();
        Self::load_with_environment(path.as_deref(), required, |key| env.get(key).cloned())
    }

    fn load_with_environment(
        path: Option<&Path>,
        required: bool,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let mut cfg = match path {
            Some(path)
                if required
                    || path
                        .try_exists()
                        .with_context(|| format!("cannot inspect config {}", path.display()))? =>
            {
                Self::from_file(path)?
            }
            _ => Self::default(),
        };
        cfg.apply_environment(env)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config at {}", path.display()))?;
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            anyhow::bail!("TOML config not supported; use config.json");
        }
        serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse config JSON at {}", path.display()))
    }

    fn apply_environment(&mut self, env: impl Fn(&str) -> Option<String>) -> Result<()> {
        if let Some(value) = env("CHIMERA_PROVIDER") {
            self.provider.name = value;
        }
        if let Some(value) = env("CHIMERA_BASE_URL") {
            self.provider.base_url = value;
        }
        let explicit_key = env("CHIMERA_API_KEY");
        if let Some(value) = &explicit_key {
            self.provider.api_key = value.clone();
        }
        if let Some(value) = env("CHIMERA_MODEL") {
            self.provider.model = value;
        }
        macro_rules! parse {
            ($name:literal, $field:expr) => {
                if let Some(value) = env($name) {
                    $field = value
                        .parse()
                        .with_context(|| format!("{} has an invalid numeric value", $name))?;
                }
            };
        }
        parse!("CHIMERA_TEMPERATURE", self.provider.temperature);
        parse!("CHIMERA_MAX_TOKENS", self.provider.max_tokens);
        parse!("CHIMERA_MAX_ITERATIONS", self.agent.max_iterations);
        parse!(
            "CHIMERA_REQUEST_TIMEOUT_SECS",
            self.provider.request_timeout_secs
        );
        parse!("CHIMERA_IDLE_TIMEOUT_SECS", self.provider.idle_timeout_secs);
        parse!(
            "CHIMERA_MAX_RESPONSE_BYTES",
            self.provider.max_response_bytes
        );
        if explicit_key.is_none() && self.provider.api_key.is_empty() {
            if let Some(key) = env("OPENAI_API_KEY") {
                self.provider.api_key = key;
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        self.provider.is_local()?;
        anyhow::ensure!(
            !self.provider.name.trim().is_empty(),
            "provider.name must not be empty"
        );
        anyhow::ensure!(
            !self.provider.model.trim().is_empty(),
            "provider.model must not be empty"
        );
        anyhow::ensure!(
            self.provider.temperature.is_finite()
                && (0.0..=2.0).contains(&self.provider.temperature),
            "temperature must be finite and between 0 and 2"
        );
        anyhow::ensure!(self.provider.max_tokens > 0, "max_tokens must be positive");
        anyhow::ensure!(
            self.agent.max_iterations <= 1000,
            "max_iterations must be at most 1000"
        );
        anyhow::ensure!(
            (1..=3600).contains(&self.provider.request_timeout_secs),
            "request_timeout_secs must be between 1 and 3600"
        );
        anyhow::ensure!(
            self.provider.idle_timeout_secs > 0
                && self.provider.idle_timeout_secs <= self.provider.request_timeout_secs,
            "idle_timeout_secs must be positive and no greater than request_timeout_secs"
        );
        anyhow::ensure!(
            (1..=64 * 1024 * 1024).contains(&self.provider.max_response_bytes),
            "max_response_bytes must be between 1 and 67108864"
        );
        Ok(())
    }

    /// Diagnostics may load a config without credentials; execution must call this.
    pub fn validate_credentials(&self) -> Result<()> {
        self.validate()?;
        anyhow::ensure!(
            self.provider.is_local()? || !self.provider.api_key.trim().is_empty(),
            "no API key configured for the remote provider; set CHIMERA_API_KEY or OPENAI_API_KEY"
        );
        Ok(())
    }

    pub fn build_provider(&self) -> chimera_llm::OpenAiCompatibleProvider {
        chimera_llm::OpenAiCompatibleProvider::new(
            &self.provider.name,
            &self.provider.base_url,
            &self.provider.api_key,
        )
        .with_limits(chimera_llm::RequestLimits {
            total: std::time::Duration::from_secs(self.provider.request_timeout_secs),
            idle: std::time::Duration::from_secs(self.provider.idle_timeout_secs),
            max_response_bytes: self.provider.max_response_bytes,
        })
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
    use std::io::Write;
    let path = config_file_path().context("could not resolve config path")?;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(serde_json::to_string_pretty(&Config::default())?.as_bytes())?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Config::from_file(&path)?.validate()?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to create {}", path.display()))
        }
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
        c.apply_environment(|key| match key {
            "CHIMERA_MODEL" => Some("test-model-xyz".into()),
            "CHIMERA_MAX_TOKENS" => Some("1234".into()),
            "CHIMERA_TEMPERATURE" => Some("0.7".into()),
            _ => None,
        })
        .unwrap();
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
        // Keep the uniquely named temporary fixture available as test evidence.
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

    #[test]
    fn config_errors_never_fall_back_to_defaults() {
        let path = std::env::temp_dir().join(format!("chimera-invalid-{}.json", uuid_for_test()));
        assert!(Config::load_with_environment(Some(&path), true, |_| None).is_err());
        assert!(Config::load_with_environment(Some(&path), false, |_| None).is_ok());
        std::fs::write(&path, "{broken").unwrap();
        assert!(Config::load_with_environment(Some(&path), false, |_| None).is_err());
        std::fs::write(&path, r#"{"provider":{"modle":"typo"}}"#).unwrap();
        assert!(Config::load_with_environment(Some(&path), false, |_| None).is_err());
    }

    #[test]
    fn environment_precedence_and_invalid_values_are_explicit() {
        let path =
            std::env::temp_dir().join(format!("chimera-precedence-{}.json", uuid_for_test()));
        std::fs::write(
            &path,
            r#"{"provider":{"model":"file-model","api_key":"file-key"}}"#,
        )
        .unwrap();
        let cfg = Config::load_with_environment(Some(&path), true, |key| match key {
            "CHIMERA_MODEL" => Some("env-model".into()),
            "OPENAI_API_KEY" => Some("fallback-key".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.provider.model, "env-model");
        assert_eq!(cfg.provider.api_key, "file-key");
        let cfg = Config::load_with_environment(Some(&path), true, |key| match key {
            "CHIMERA_API_KEY" => Some("".into()),
            "OPENAI_API_KEY" => Some("fallback-key".into()),
            _ => None,
        })
        .unwrap();
        assert!(cfg.provider.api_key.is_empty());
        for (key, value) in [
            ("CHIMERA_MAX_TOKENS", "bad"),
            ("CHIMERA_MAX_TOKENS", "0"),
            ("CHIMERA_TEMPERATURE", "NaN"),
            ("CHIMERA_TEMPERATURE", "3"),
            ("CHIMERA_MODEL", ""),
            ("CHIMERA_BASE_URL", "bad"),
            ("CHIMERA_MAX_ITERATIONS", "1001"),
            ("CHIMERA_REQUEST_TIMEOUT_SECS", "0"),
            ("CHIMERA_MAX_RESPONSE_BYTES", "0"),
        ] {
            assert!(
                Config::load_with_environment(None, false, |name| (name == key)
                    .then(|| value.into()))
                .is_err(),
                "{key}"
            );
        }
        let zero = Config::load_with_environment(None, false, |key| {
            (key == "CHIMERA_MAX_ITERATIONS").then(|| "0".into())
        })
        .unwrap();
        assert_eq!(zero.agent.max_iterations, 0);
    }

    #[test]
    fn remote_lookalike_requires_credentials() {
        let mut cfg = Config::default();
        cfg.provider.base_url = "https://localhost.example.test/v1".into();
        assert!(cfg.validate_credentials().is_err());
        cfg.provider.base_url = "http://[::1]:1234/v1".into();
        assert!(cfg.validate_credentials().is_ok());
    }

    #[test]
    fn debug_output_hides_api_key() {
        let mut cfg = Config::default();
        cfg.provider.api_key = "private-test-token".into();
        assert!(!format!("{cfg:?}").contains("private-test-token"));
    }

    #[test]
    fn editor_resolution_prefers_visual_before_editor() {
        let cfg = Config::default();
        assert_eq!(cfg.editor_with(|_| None), None);
        assert_eq!(
            cfg.editor_with(|key| (key == "VISUAL").then(|| "visual-bin".into())),
            Some("visual-bin".to_string())
        );
        // $EDITOR is used only when $VISUAL is unset or blank.
        assert_eq!(
            cfg.editor_with(|key| match key {
                "EDITOR" => Some("editor-bin".into()),
                _ => None,
            }),
            Some("editor-bin".to_string())
        );
        assert_eq!(
            cfg.editor_with(|key| match key {
                "VISUAL" => Some("   ".into()),
                "EDITOR" => Some("editor-bin".into()),
                _ => None,
            }),
            Some("editor-bin".to_string())
        );
    }

    #[test]
    fn saved_editor_setting_beats_the_environment() {
        let cfg = Config {
            editor: "  saved-bin  ".into(),
            ..Default::default()
        };
        assert_eq!(
            cfg.editor_with(|key| (key == "VISUAL").then(|| "visual-bin".into())),
            Some("saved-bin".to_string())
        );
    }

    #[test]
    fn editor_setting_round_trips_through_the_settings_file() {
        let path = std::env::temp_dir().join(format!("chimera-editor-{}.json", uuid_for_test()));
        std::fs::write(&path, r#"{"provider":{"model":"m"},"editor":"code -w"}"#).unwrap();
        let cfg = Config::from_file(&path).unwrap();
        assert_eq!(cfg.editor, "code -w");
        let written = serde_json::to_string(&cfg).unwrap();
        assert!(written.contains(r#""editor":"code -w""#));
        let reloaded = serde_json::from_str::<Config>(&written).unwrap();
        assert_eq!(reloaded.editor, "code -w");
        // Without a configured editor the key stays out of the file, so the
        // settings of a CHIMERA without this feature remain byte-identical.
        let plain = serde_json::to_string(&Config::default()).unwrap();
        assert!(!plain.contains("editor"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ui_theme_stays_out_of_the_file_until_it_is_set() {
        let plain = serde_json::to_string(&Config::default()).unwrap();
        assert!(!plain.contains("ui"));
        let themed = Config {
            ui: UiConfig {
                theme: "light".into(),
            },
            ..Default::default()
        };
        let written = serde_json::to_string(&themed).unwrap();
        assert!(written.contains(r#""theme":"light""#));
        let reloaded: Config = serde_json::from_str(&written).unwrap();
        assert_eq!(reloaded.ui.theme, "light");
    }
}
