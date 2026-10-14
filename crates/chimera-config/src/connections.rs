//! Saved connections and bounded, read-only discovery of service models.
use crate::{Config, ProviderConfig};
use anyhow::{Context, Result};
use std::{io::Write, path::Path, time::Duration};

pub struct Preset {
    pub name: &'static str,
    pub address: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "OpenAI",
        address: "https://api.openai.com/v1",
    },
    Preset {
        name: "OpenRouter",
        address: "https://openrouter.ai/api/v1",
    },
    Preset {
        name: "Groq",
        address: "https://api.groq.com/openai/v1",
    },
    Preset {
        name: "LM Studio",
        address: "http://127.0.0.1:1234/v1",
    },
    Preset {
        name: "Ollama",
        address: "http://127.0.0.1:11434/v1",
    },
    Preset {
        name: "Custom",
        address: "",
    },
];

/// Never follow redirects with a user's key or include response bodies in errors.
pub async fn models(provider: &ProviderConfig) -> Result<Vec<String>> {
    let local = provider.is_local()?;
    anyhow::ensure!(
        local || !provider.api_key.trim().is_empty(),
        "Enter an access key for this service."
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    let mut request = client.get(format!(
        "{}/models",
        provider.base_url.trim_end_matches('/')
    ));
    if !provider.api_key.is_empty() {
        request = request.bearer_auth(&provider.api_key);
    }
    let mut response = request.send().await.map_err(|_| {
        anyhow::anyhow!("Could not reach the service. Check its address and whether it is running.")
    })?;
    let status = response.status();
    anyhow::ensure!(
        status.is_success(),
        "{}",
        match status.as_u16() {
            401 | 403 =>
                "The service rejected this key. Check the key and account access.".to_string(),
            404 =>
                "The service has no model list at this address. Check the address, including /v1."
                    .to_string(),
            429 => "The service is busy or has reached its limit. Try again shortly.".to_string(),
            _ => format!(
                "The service returned HTTP {}. Check the address or try again.",
                status.as_u16()
            ),
        }
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("The connection ended while reading models. Try again."))?
    {
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= 4 * 1024 * 1024,
            "The service returned a model list larger than 4 MB."
        );
        bytes.extend_from_slice(&chunk);
    }
    parse_models(&bytes)
}

pub fn parse_models(bytes: &[u8]) -> Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("The service did not return a readable model list."))?;
    let rows = value["data"]
        .as_array()
        .context("The service did not return a models data list.")?;
    let mut result = rows
        .iter()
        .filter_map(|row| row["id"].as_str())
        .filter(|id| !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    anyhow::ensure!(!result.is_empty(), "No models are available. Load a model in the local service or check account access, then retry.");
    Ok(result)
}

/// Save only the selected connection change, preserving other settings and profiles.
/// Keep an owner-readable copy of the previous file before an atomic replacement.
pub fn save_provider(path: &Path, provider: &ProviderConfig) -> Result<Config> {
    let mut config = if path.try_exists()? {
        Config::from_file(path)?
    } else {
        Config::default()
    };
    config
        .providers
        .insert(config.provider.name.clone(), config.provider.clone());
    config
        .providers
        .insert(provider.name.clone(), provider.clone());
    config.provider = provider.clone();
    config.validate_credentials()?;
    save_private(path, &serde_json::to_vec_pretty(&config)?)?;
    Ok(config)
}

pub fn save_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        anyhow::ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Settings must be a regular file, not a link."
        );
        let backup = parent.join(format!(
            ".chimera-settings-{}.previous",
            uuid::Uuid::new_v4()
        ));
        private_file(&backup, &std::fs::read(path)?)?;
    }
    let staged = parent.join(format!(
        ".chimera-settings-{}.pending",
        uuid::Uuid::new_v4()
    ));
    private_file(&staged, bytes)?;
    std::fs::rename(&staged, path)
        .context("Could not save settings; the previous file remains available.")?;
    Ok(())
}

fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_list_is_sorted_unique_and_rejects_empty_or_bad_responses() {
        assert_eq!(
            parse_models(br#"{"data":[{"id":"z"},{"id":"a"},{"id":"z"},{"id":"\u001b"}]}"#)
                .unwrap(),
            ["a", "z"]
        );
        assert!(parse_models(br#"{"data":[]}"#).is_err());
        assert!(parse_models(b"secret response body")
            .unwrap_err()
            .to_string()
            .find("secret")
            .is_none());
    }
    #[test]
    fn save_preserves_settings_connections_and_private_previous_copy() {
        let root = std::env::temp_dir().join(format!("chimera-connect-{}", uuid::Uuid::new_v4()));
        let path = root.join("config.json");
        let mut cfg = Config::default();
        cfg.agent.max_iterations = 17;
        save_private(&path, &serde_json::to_vec(&cfg).unwrap()).unwrap();
        let provider = ProviderConfig {
            name: "Local".into(),
            base_url: "http://127.0.0.1:12434/v1".into(),
            model: "model-a".into(),
            ..Default::default()
        };
        let saved = save_provider(&path, &provider).unwrap();
        assert_eq!(saved.agent.max_iterations, 17);
        assert_eq!(saved.providers["Local"].model, "model-a");
        assert!(saved.providers.contains_key("openai"));
        assert_eq!(Config::from_file(&path).unwrap().provider.name, "Local");
        assert!(std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .any(|e| e.path().extension().is_some_and(|v| v == "previous")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
