//! chimera-tui::sessions — In-chat conversation chooser.
//!
//! Lists every saved conversation newest-first with title, date and preview,
//! and resumes the chosen one in place; the unsent draft stays in the input.

use crate::dialog::{Choice, Dialog};
use anyhow::{Context, Result};
use chimera_core::runtime::SavedSession;
use chimera_session::{FileSessionStore, SessionMeta, SessionStore};
use std::path::PathBuf;

const PREVIEW_CHARS: usize = 80;

/// One listed conversation: title, created date and a one-line preview.
struct Conversation {
    id: uuid::Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    label: String,
    preview: String,
}

impl Conversation {
    fn new(id: uuid::Uuid, meta: &SessionMeta) -> Self {
        let first_line = meta.objective.lines().next().unwrap_or_default();
        let mut chars = first_line.chars();
        let mut preview: String = chars.by_ref().take(PREVIEW_CHARS).collect();
        if chars.next().is_some() {
            preview.push('…');
        }
        Self {
            id,
            created_at: meta.created_at,
            label: meta.label.clone(),
            preview,
        }
    }

    fn date_text(&self) -> String {
        self.created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    fn choice(&self) -> Choice {
        Choice::new(
            self.label.clone(),
            format!("{} · {}", self.date_text(), self.preview),
            format!("sessions:open:{}", self.id),
        )
    }
}

/// A conversation listing request that runs while the chat screen keeps drawing.
#[derive(Default)]
pub struct Sessions {
    root: Option<PathBuf>,
    job: Option<tokio::task::JoinHandle<Result<Vec<Conversation>>>>,
}

impl Sessions {
    /// Start listing saved conversations and show a waiting panel.
    pub fn open(&mut self, root: PathBuf) -> Dialog {
        self.begin(root, "Loading saved conversations from this project.")
    }

    fn begin(&mut self, root: PathBuf, note: &str) -> Dialog {
        self.cancel();
        self.root = Some(root.clone());
        self.job = Some(tokio::spawn(
            async move { Self::list_conversations(root).await },
        ));
        Dialog::busy("Switch conversation", note)
    }

    /// Every stored conversation, newest first. One unreadable record is
    /// skipped so a single damaged file cannot hide the rest of the list.
    async fn list_conversations(root: PathBuf) -> Result<Vec<Conversation>> {
        let store = FileSessionStore::new(root);
        let mut conversations = Vec::new();
        for id in store.list().await? {
            if let Ok(state) = store.load(id).await {
                conversations.push(Conversation::new(id, &state.meta));
            }
        }
        conversations.sort_by_key(|a| std::cmp::Reverse((a.created_at, a.id)));
        Ok(conversations)
    }

    fn rows(conversations: &[Conversation]) -> Vec<Choice> {
        conversations.iter().map(Conversation::choice).collect()
    }

    /// Abandon a running listing request. Safe when nothing is running.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.root = None;
    }

    /// Handle one confirmed overlay choice. `Ok(Some(session))` means the
    /// conversation was resumed and the caller should show it in place.
    pub async fn action(
        &mut self,
        action: &str,
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<SavedSession>> {
        if action == "sessions:retry" {
            let root = self
                .root
                .clone()
                .context("The conversation list is not open. Use /sessions.")?;
            *dialog = Some(self.begin(root, "Loading saved conversations again."));
        } else if let Some(id) = action.strip_prefix("sessions:open:") {
            let id = uuid::Uuid::parse_str(id).context(
                "That conversation could not be identified. Choose it from the list again.",
            )?;
            let root = self
                .root
                .clone()
                .context("The conversation list is not open. Use /sessions.")?;
            let resumed = SavedSession::resume(root, id).await.map_err(|error| {
                anyhow::anyhow!(
                    "Could not open that conversation. It may be damaged or from another project. {error}"
                )
            })?;
            self.cancel();
            *dialog = None;
            return Ok(Some(resumed));
        }
        Ok(None)
    }

    /// Move a finished listing request into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let result = job.await.unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "The conversation list stopped loading. Try again."
            ))
        });
        match result {
            Ok(conversations) => {
                let note = if conversations.is_empty() {
                    "No saved conversations yet. Send your first request and it will appear here."
                } else {
                    "Search by title, date or preview. Enter opens the conversation here; your unsent text is kept."
                };
                *dialog = Some(Dialog::choices(
                    "Switch conversation",
                    note,
                    Self::rows(&conversations),
                ));
            }
            Err(error) => {
                *dialog = Some(Dialog::choices(
                    "Switch conversation",
                    &error.to_string(),
                    vec![Choice::new(
                        "Try again",
                        "Reload the saved conversation list.",
                        "sessions:retry",
                    )],
                ));
            }
        }
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use chrono::TimeZone;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "chimera-sessions-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    async fn seed(
        root: &std::path::Path,
        label: &str,
        created: chrono::DateTime<chrono::Utc>,
    ) -> uuid::Uuid {
        let store = FileSessionStore::new(root.to_path_buf());
        store
            .create(SessionMeta {
                label: label.into(),
                objective: format!("{label} objective"),
                created_at: created,
            })
            .await
            .unwrap()
    }

    fn rows(dialog: &Dialog) -> Vec<(String, String, String)> {
        match &dialog.body {
            Body::Choices { rows, .. } => rows
                .iter()
                .map(|row| (row.label.clone(), row.detail.clone(), row.action.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    async fn wait_for_choices(sessions: &mut Sessions, dialog: &mut Option<Dialog>) {
        for _ in 0..500 {
            sessions.poll(dialog).await;
            if matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. })) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the conversation listing never finished");
    }

    #[test]
    fn preview_truncates_to_one_line() {
        let long = SessionMeta {
            label: "Title".into(),
            objective: "x".repeat(200) + "\nsecond line",
            created_at: chrono::Utc::now(),
        };
        let conversation = Conversation::new(uuid::Uuid::new_v4(), &long);
        assert_eq!(conversation.preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(conversation.preview.ends_with('…'));
        let multiline = SessionMeta {
            label: "Title".into(),
            objective: "first line\nsecond line".into(),
            created_at: chrono::Utc::now(),
        };
        let conversation = Conversation::new(uuid::Uuid::new_v4(), &multiline);
        assert_eq!(conversation.preview, "first line");
    }

    #[tokio::test]
    async fn rows_are_newest_first_with_title_date_and_preview() {
        let root = temp_root();
        let older = seed(
            &root,
            "Older conversation",
            chrono::Utc.with_ymd_and_hms(2026, 3, 10, 8, 30, 0).unwrap(),
        )
        .await;
        let newer = seed(
            &root,
            "Newer conversation",
            chrono::Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap(),
        )
        .await;
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root.clone()));
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        wait_for_choices(&mut sessions, &mut dialog).await;
        let rows = rows(dialog.as_ref().unwrap());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Newer conversation");
        assert_eq!(rows[1].0, "Older conversation");
        assert_eq!(rows[0].2, format!("sessions:open:{newer}"));
        assert_eq!(rows[1].2, format!("sessions:open:{older}"));
        // The detail shows a readable date and the objective preview; the
        // year-month prefix is stable under any timezone shift.
        assert!(rows[0].1.starts_with("2026-06"));
        assert!(rows[0].1.contains("Newer conversation objective"));
        assert!(rows[1].1.starts_with("2026-03"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn typing_filters_and_enter_targets_the_chosen_conversation() {
        let root = temp_root();
        let payments = seed(
            &root,
            "Payments refactor",
            chrono::Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap(),
        )
        .await;
        seed(
            &root,
            "Login outage",
            chrono::Utc.with_ymd_and_hms(2026, 3, 10, 8, 30, 0).unwrap(),
        )
        .await;
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root.clone()));
        wait_for_choices(&mut sessions, &mut dialog).await;
        let mut dialog = dialog.unwrap();
        dialog.paste("refactor");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == format!("sessions:open:{payments}")
        ));
        // The objective preview is searchable too.
        let mut dialog = Some(sessions.open(root.clone()));
        wait_for_choices(&mut sessions, &mut dialog).await;
        let mut dialog = dialog.unwrap();
        dialog.paste("outage objective");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action.starts_with("sessions:open:") && action != format!("sessions:open:{payments}")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_garbage_or_unknown_id_reports_an_error_and_keeps_the_overlay() {
        let root = temp_root();
        seed(&root, "Only conversation", chrono::Utc::now()).await;
        let mut sessions = Sessions::default();
        sessions.root = Some(root.clone());
        let mut dialog = Some(Dialog::choices("Switch conversation", "", Vec::new()));
        let error = match sessions
            .action("sessions:open:not-a-uuid", &mut dialog)
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("a garbage id must be refused"),
        };
        assert!(error.to_string().contains("could not be identified"));
        assert!(dialog.is_some());
        let unknown = uuid::Uuid::new_v4();
        let error = match sessions
            .action(&format!("sessions:open:{unknown}"), &mut dialog)
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("an unknown session must be refused"),
        };
        assert!(error
            .to_string()
            .contains("Could not open that conversation"));
        assert!(dialog.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_clears_an_in_flight_listing_job() {
        let root = temp_root();
        seed(&root, "Only conversation", chrono::Utc::now()).await;
        let mut sessions = Sessions::default();
        let _busy = sessions.open(root.clone());
        assert!(sessions.job.is_some());
        sessions.cancel();
        assert!(sessions.job.is_none());
        assert!(sessions.root.is_none());
        let mut dialog = None;
        sessions.poll(&mut dialog).await;
        assert!(dialog.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_store_shows_a_helpful_empty_state() {
        let root = temp_root();
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root.clone()));
        wait_for_choices(&mut sessions, &mut dialog).await;
        let dialog = dialog.unwrap();
        assert!(dialog.note.contains("No saved conversations"));
        assert!(rows(&dialog).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
