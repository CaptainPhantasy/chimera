use crate::dialog::{Choice, Dialog, Field};
use anyhow::{Context, Result};
use chimera_config::{
    connections::{self, PRESETS},
    Config, ProviderConfig,
};

#[derive(Default)]
pub struct Connections {
    pub pending: Option<ProviderConfig>,
    models: Vec<String>,
    job: Option<tokio::task::JoinHandle<Result<Vec<String>>>>,
}
impl Connections {
    pub fn open(&mut self, config: &Config) -> Dialog {
        self.cancel();
        let mut rows = config
            .providers
            .values()
            .filter(|p| p.is_local().unwrap_or(false) || !p.api_key.is_empty())
            .map(|p| {
                Choice::new(
                    format!("{} — saved", p.name),
                    &p.base_url,
                    format!("connect:saved:{}", p.name),
                )
            })
            .collect::<Vec<_>>();
        rows.extend(PRESETS.iter().enumerate().map(|(i, p)| {
            Choice::new(
                p.name,
                if p.address.is_empty() {
                    "Another compatible service"
                } else {
                    p.address
                },
                format!("connect:preset:{i}"),
            )
        }));
        Dialog::choices(
            "Connect a provider",
            "Choose a service. Your key stays private and is never sent as a chat message.",
            rows,
        )
    }
    fn form(provider: &ProviderConfig, note: &str) -> Dialog {
        Dialog::form(
            "Connect a provider",
            note,
            vec![
                Field::new("Connection name", &provider.name, false),
                Field::new("Service address", &provider.base_url, false),
                Field::new(
                    "Access key (local services may leave blank)",
                    &provider.api_key,
                    true,
                ),
            ],
            "connect:test",
        )
    }
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.pending = None;
        self.models.clear();
    }
    pub async fn action(
        &mut self,
        action: &str,
        values: &[String],
        config: &Config,
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<Config>> {
        if let Some(index) = action.strip_prefix("connect:preset:") {
            let preset = PRESETS
                .get(index.parse::<usize>()?)
                .context("Provider choice is no longer available.")?;
            let provider = ProviderConfig {
                name: preset.name.into(),
                base_url: preset.address.into(),
                ..Default::default()
            };
            *dialog = Some(Self::form(&provider, "Check the address, then paste your key. Enter on the key field checks the connection."));
        } else if let Some(name) = action.strip_prefix("connect:saved:") {
            let provider = config
                .providers
                .get(name)
                .context("Saved provider is no longer available.")?;
            *dialog = Some(Self::form(provider, "Your saved key is masked. Edit any field, then press Enter on the key field to reconnect."));
        } else if action == "connect:test" {
            anyhow::ensure!(values.len() == 3, "The connection form is incomplete.");
            let provider = ProviderConfig {
                name: values[0].trim().into(),
                base_url: values[1].trim().into(),
                api_key: values[2].trim().into(),
                ..Default::default()
            };
            anyhow::ensure!(
                !provider.name.is_empty() && !provider.name.chars().any(char::is_control),
                "Enter a connection name."
            );
            provider.is_local().map_err(|_| {
                anyhow::anyhow!("Enter a valid HTTPS address, or HTTP for a local service.")
            })?;
            self.cancel();
            self.pending = Some(provider.clone());
            self.job = Some(tokio::spawn(
                async move { connections::models(&provider).await },
            ));
            *dialog = Some(Dialog::busy(
                "Checking connection",
                "Fetching available models. This does not send a chat request.",
            ));
        } else if let Some(index) = action.strip_prefix("connect:model:") {
            let mut provider = self.pending.clone().context("Choose a provider first.")?;
            provider.model = self
                .models
                .get(index.parse::<usize>()?)
                .context("Choose a model from this list.")?
                .clone();
            let path =
                chimera_config::config_file_path().context("Cannot locate your settings file.")?;
            let saved = connections::save_provider(&path, &provider).map_err(|_| anyhow::anyhow!("Could not save this connection. Check settings-file access; your previous settings are preserved."))?;
            self.cancel();
            *dialog = None;
            return Ok(Some(saved));
        }
        Ok(None)
    }
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else {
            return;
        };
        let result = job
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("The connection check stopped. Try again.")));
        match result {
            Ok(models) => {
                self.models = models;
                *dialog = Some(Dialog::choices(
                    "Choose a model",
                    "Connection checked. Select a model to save this provider and start using it.",
                    self.models
                        .iter()
                        .enumerate()
                        .map(|(i, name)| {
                            Choice::new(
                                name,
                                "Available from this service",
                                format!("connect:model:{i}"),
                            )
                        })
                        .collect(),
                ));
            }
            Err(error) => {
                if let Some(provider) = &self.pending {
                    *dialog = Some(Self::form(provider, &error.to_string()));
                }
            }
        }
    }
}
impl Drop for Connections {
    fn drop(&mut self) {
        self.cancel();
    }
}
