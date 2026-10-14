//! chimera-tui::models — In-chat model chooser.
//!
//! Lists the models a saved service offers, saves the chosen one as the
//! default, and reports when an explicit process override keeps winning.

use crate::dialog::{Choice, Dialog};
use anyhow::{Context, Result};
use chimera_config::{connections, Config, ProviderConfig};

/// A model list request that runs while the chat screen keeps drawing.
#[derive(Default)]
pub struct Models {
    provider: Option<ProviderConfig>,
    models: Vec<String>,
    job: Option<tokio::task::JoinHandle<Result<Vec<String>>>>,
}

impl Models {
    /// Choices for every saved service the user can list models from.
    pub fn open(&mut self, config: &Config) -> Dialog {
        self.cancel();
        Dialog::choices(
            "Choose a model",
            "Pick a service to list its models. The model you choose is saved as your default.",
            Self::provider_rows(config),
        )
    }

    fn provider_rows(config: &Config) -> Vec<Choice> {
        let mut rows = vec![Self::row(&config.provider, true)];
        for provider in config.providers.values() {
            if provider.name != config.provider.name
                && (provider.is_local().unwrap_or(false) || !provider.api_key.is_empty())
            {
                rows.push(Self::row(provider, false));
            }
        }
        rows
    }

    fn row(provider: &ProviderConfig, current: bool) -> Choice {
        let label = if current {
            format!("{} — current", provider.name)
        } else {
            provider.name.clone()
        };
        let detail = if provider.base_url.is_empty() {
            "No address saved".to_string()
        } else {
            provider.base_url.clone()
        };
        Choice::new(label, detail, format!("models:provider:{}", provider.name))
    }

    fn model_rows(models: &[String]) -> Vec<Choice> {
        models
            .iter()
            .enumerate()
            .map(|(index, name)| {
                Choice::new(
                    name,
                    "Available from this service",
                    format!("models:select:{index}"),
                )
            })
            .collect()
    }

    fn find<'a>(config: &'a Config, name: &str) -> Option<&'a ProviderConfig> {
        if config.provider.name == name {
            Some(&config.provider)
        } else {
            config.providers.get(name)
        }
    }

    fn start(&mut self, provider: ProviderConfig, dialog: &mut Option<Dialog>, note: &str) {
        self.cancel();
        self.provider = Some(provider.clone());
        self.job = Some(tokio::spawn(
            async move { connections::models(&provider).await },
        ));
        *dialog = Some(Dialog::busy("Choose a model", note));
    }

    /// Abandon a running list request. Safe when nothing is running.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.provider = None;
        self.models.clear();
    }

    /// Handle one confirmed overlay choice. `Ok(Some(config))` means the
    /// chosen model is saved and the caller should reload settings.
    pub async fn action(
        &mut self,
        action: &str,
        config: &Config,
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<Config>> {
        if let Some(name) = action.strip_prefix("models:provider:") {
            let provider = Self::find(config, name)
                .context("That service is no longer available.")?
                .clone();
            self.start(
                provider,
                dialog,
                "Fetching the model list. This does not send a chat request.",
            );
        } else if action == "models:retry" {
            let provider = self.provider.clone().context("Choose a service first.")?;
            self.start(
                provider,
                dialog,
                "Fetching the model list again. This does not send a chat request.",
            );
        } else if let Some(index) = action.strip_prefix("models:select:") {
            let provider = self.provider.clone().context("Choose a service first.")?;
            let index = index
                .parse::<usize>()
                .context("Choose a model from this list.")?;
            let model = self
                .models
                .get(index)
                .context("Choose a model from this list.")?
                .clone();
            let path =
                chimera_config::config_file_path().context("Cannot locate your settings file.")?;
            let saved = connections::save_model(&path, &provider, &model).map_err(|_| {
                anyhow::anyhow!(
                    "Could not save this model choice. Check settings-file access; your previous settings are preserved."
                )
            })?;
            self.cancel();
            *dialog = None;
            return Ok(Some(saved));
        }
        Ok(None)
    }

    /// Move a finished list request into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let Some(provider) = self.provider.clone() else {
            return;
        };
        let result = job.await.unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "The model list request stopped. Try again."
            ))
        });
        match result {
            Ok(models) => {
                self.models = models;
                *dialog = Some(Dialog::choices(
                    "Choose a model",
                    &format!(
                        "Available from {}. The model you choose is saved as your default.",
                        provider.name
                    ),
                    Self::model_rows(&self.models),
                ));
            }
            Err(error) => {
                *dialog = Some(Dialog::choices(
                    "Choose a model",
                    &error.to_string(),
                    vec![Choice::new(
                        "Try again",
                        provider.base_url.clone(),
                        "models:retry",
                    )],
                ));
            }
        }
    }
}

impl Drop for Models {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn config() -> Config {
        let mut config = Config {
            provider: ProviderConfig {
                name: "Local".into(),
                base_url: "http://127.0.0.1:12434/v1".into(),
                model: "first".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        config
            .providers
            .insert("Local".into(), config.provider.clone());
        for (name, key) in [("Hosted", "stored-key"), ("Keyless", "")] {
            config.providers.insert(
                name.into(),
                ProviderConfig {
                    name: name.into(),
                    base_url: "https://api.example.com/v1".into(),
                    api_key: key.into(),
                    ..Default::default()
                },
            );
        }
        config
    }

    fn rows(dialog: &Dialog) -> Vec<(String, String)> {
        match &dialog.body {
            Body::Choices { rows, .. } => rows
                .iter()
                .map(|row| (row.label.clone(), row.action.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn chooser_marks_the_current_service_and_hides_keyless_ones() {
        let dialog = Models::default().open(&config());
        let rows = rows(&dialog);
        assert_eq!(
            rows.first(),
            Some(&(
                "Local — current".to_string(),
                "models:provider:Local".to_string()
            ))
        );
        assert!(rows.iter().any(|(_, a)| a == "models:provider:Hosted"));
        assert!(!rows.iter().any(|(_, a)| a == "models:provider:Keyless"));
        // The chooser is searchable by label or address.
        let mut dialog = dialog;
        dialog.paste("hosted");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == "models:provider:Hosted"
        ));
    }

    #[test]
    fn model_choices_select_by_position_and_search() {
        let mut dialog = Dialog::choices(
            "Choose a model",
            "",
            Models::model_rows(&["alpha".into(), "beta".into()]),
        );
        dialog.paste("beta");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == "models:select:1"
        ));
    }

    #[tokio::test]
    async fn an_unknown_service_reports_an_error_without_changing_the_overlay() {
        let mut models = Models::default();
        let mut dialog = Some(Dialog::busy("Choose a model", "starting"));
        let error = models
            .action("models:provider:Missing", &config(), &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer available"));
        assert!(matches!(models.models.as_slice(), []));
        assert!(dialog.is_some());
    }

    #[tokio::test]
    async fn selecting_a_model_before_listing_anything_is_refused() {
        let mut models = Models::default();
        let mut dialog = Some(Dialog::busy("Choose a model", "starting"));
        let error = models
            .action("models:select:0", &config(), &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Choose a service first"));
        assert!(dialog.is_some());
    }

    #[tokio::test]
    async fn cancelling_clears_a_finished_list_request() {
        let mut models = Models::default();
        models.models = vec!["alpha".into()];
        models.cancel();
        assert!(models.models.is_empty());
        let mut dialog = None;
        models.poll(&mut dialog).await;
        assert!(dialog.is_none());
    }
}
