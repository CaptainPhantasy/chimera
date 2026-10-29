//! chimera-tui::sessions — In-chat conversation chooser.
//!
//! Lists every saved conversation newest-first with title, date and preview,
//! and resumes the chosen one in place; the unsent draft stays in the input.

use crate::dialog::{Choice, Dialog};
use anyhow::{Context, Result};
use chimera_core::runtime::SavedSession;
use chimera_session::{FileSessionStore, SessionEvent, SessionMeta, SessionState, SessionStore};
use std::path::PathBuf;

const PREVIEW_CHARS: usize = 80;

/// One listed conversation: title, created date and a one-line preview.
struct Conversation {
    id: uuid::Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    last_activity: chrono::DateTime<chrono::Utc>,
    search_text: String,
    archived: bool,
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
            last_activity: meta.created_at,
            search_text: meta.objective.clone(),
            archived: false,
            label: meta.label.clone(),
            preview,
        }
    }

    fn from_state(state: &SessionState, root: &std::path::Path, archived: bool) -> Self {
        let mut conversation = Self::new(state.id, &state.meta);
        conversation.archived = archived;
        let parent = root.join(".chimera").join("sessions");
        let path = if archived {
            parent.join("archived")
        } else {
            parent
        }
        .join(format!("{}.json", state.id));
        if let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) {
            conversation.last_activity = conversation.last_activity.max(modified.into());
        }
        for event in &state.events {
            if let SessionEvent::Custom { payload, .. } = event {
                if let Some(at) = payload
                    .get("at")
                    .and_then(|v| v.as_str())
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                {
                    conversation.last_activity = conversation
                        .last_activity
                        .max(at.with_timezone(&chrono::Utc));
                }
            }
        }
        conversation
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
            format!(
                "{} · used {} · {}",
                self.date_text(),
                self.last_activity
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M"),
                self.preview
            ),
            format!(
                "sessions:{}:{}",
                if self.archived { "restore" } else { "open" },
                self.id
            ),
        )
        .with_search_text(self.search_text.clone())
    }
}

/// A conversation listing request that runs while the chat screen keeps drawing.
#[derive(Default)]
pub struct Sessions {
    root: Option<PathBuf>,
    archived: bool,
    query: String,
    job: Option<tokio::task::JoinHandle<Result<Vec<Conversation>>>>,
}

impl Sessions {
    /// Start listing saved conversations and show a waiting panel.
    pub fn open(&mut self, root: PathBuf) -> Dialog {
        self.archived = false;
        self.begin(root, "Loading saved conversations from this project.")
    }

    fn begin(&mut self, root: PathBuf, note: &str) -> Dialog {
        let archived = self.archived;
        self.cancel();
        self.root = Some(root.clone());
        self.query.clear();
        self.job = Some(tokio::spawn(async move {
            Self::list_matching(root, archived, "").await
        }));
        Dialog::busy("Switch conversation", note)
    }

    /// Every stored conversation, newest first. One unreadable record is
    /// skipped so a single damaged file cannot hide the rest of the list.
    async fn list_matching(
        root: PathBuf,
        archived: bool,
        query: &str,
    ) -> Result<Vec<Conversation>> {
        let store = FileSessionStore::new(root.clone());
        let ids = if archived {
            store.archived_session_ids().await?
        } else {
            store.list().await?
        };
        let query = query.to_lowercase();
        let mut conversations = Vec::new();
        for id in ids {
            tokio::task::yield_now().await;
            let result = if archived {
                store.load_archived(id).await
            } else {
                store.load(id).await
            };
            if let Ok(state) = result {
                let mut conversation = Conversation::from_state(&state, &root, archived);
                let metadata = format!(
                    "{} {} {}",
                    conversation.label,
                    conversation.preview,
                    conversation.date_text()
                )
                .to_lowercase();
                let content_matches = query.is_empty()
                    || metadata.contains(&query)
                    || conversation.search_text.to_lowercase().contains(&query)
                    || state.events.iter().any(|event| match event {
                        SessionEvent::Custom { key, payload } if key == "messages" => {
                            payload.as_array().is_some_and(|messages| {
                                messages.iter().any(|message| {
                                    message.get("role").and_then(|r| r.as_str()) != Some("system")
                                        && message
                                            .get("content")
                                            .and_then(|v| v.as_str())
                                            .is_some_and(|text| {
                                                text.to_lowercase().contains(&query)
                                            })
                                })
                            })
                        }
                        _ => false,
                    });
                if content_matches {
                    // The result is already full-text filtered. Retain only the
                    // current query, never all private bodies across all sessions.
                    conversation.search_text = query.clone();
                    conversations.push(conversation);
                }
            }
        }
        conversations.sort_by_key(|a| std::cmp::Reverse((a.last_activity, a.id)));
        Ok(conversations)
    }

    fn rows(conversations: &[Conversation]) -> Vec<Choice> {
        conversations
            .iter()
            .map(|c| {
                if c.archived {
                    c.choice()
                } else {
                    c.choice()
                        .with_alt_action(format!("sessions:delete:{}", c.id))
                }
            })
            .collect()
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
        if action == "sessions:archives" || action == "sessions:active" {
            let root = self.root.clone().context("Open /sessions first.")?;
            self.archived = action == "sessions:archives";
            *dialog = Some(self.begin(root, "Loading conversations."));
        } else if let Some(id) = action.strip_prefix("sessions:restore:") {
            let id = uuid::Uuid::parse_str(id)
                .context("Choose an archived conversation from the list.")?;
            let root = self.root.clone().context("Open /sessions first.")?;
            FileSessionStore::new(root.clone())
                .restore_archived(id)
                .await?;
            let resumed = SavedSession::resume(root, id).await.context(
                "The archived record was restored to the active list, but its conversation could not be opened. Reopen /sessions to inspect it; the restored record is preserved",
            )?;
            self.cancel();
            *dialog = None;
            return Ok(Some(resumed));
        } else if action == "sessions:retry" {
            let root = self
                .root
                .clone()
                .context("The conversation list is not open. Use /sessions.")?;
            *dialog = Some(self.begin(root, "Loading saved conversations again."));
        } else if let Some(id) = action.strip_prefix("sessions:delete-confirm:") {
            let id = uuid::Uuid::parse_str(id).context(
                "That conversation could not be identified. Choose it from the list again.",
            )?;
            let root = self
                .root
                .clone()
                .context("The conversation list is not open. Use /sessions.")?;
            FileSessionStore::new(root.clone())
                .delete(id)
                .await
                .map_err(|error| anyhow::anyhow!("Could not archive that conversation. {error}"))?;
            *dialog = Some(self.begin(
                root,
                "Conversation archived. Restore it from Archived conversations.",
            ));
        } else if let Some(id) = action.strip_prefix("sessions:delete:") {
            if let Some(job) = self.job.take() {
                job.abort();
            }
            let id = uuid::Uuid::parse_str(id).context(
                "That conversation could not be identified. Choose it from the list again.",
            )?;
            *dialog = Some(Dialog::choices(
                "Archive conversation",
                "Remove this conversation from the active list. Its messages remain private and recoverable under Archived conversations. Enter keeps it by default.",
                vec![
                    Choice::new("Keep it", "Return to the list without changing anything", "sessions:retry"),
                    Choice::new(
                        "Archive conversation",
                        "Hide it now; restore it later from Archived conversations",
                        format!("sessions:delete-confirm:{id}"),
                    ),
                ],
            ));
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
        let title = if self.archived {
            "Archived conversations"
        } else {
            "Switch conversation"
        };
        let edited_query =
            dialog
                .as_ref()
                .filter(|d| d.title == title)
                .and_then(|d| match &d.body {
                    crate::dialog::Body::Choices { search, .. } => Some(search.clone()),
                    _ => None,
                });
        if let Some(query) = edited_query.filter(|query| *query != self.query) {
            if let Some(root) = self.root.clone() {
                if let Some(job) = self.job.take() {
                    job.abort();
                }
                self.query = query.clone();
                let archived = self.archived;
                self.job = Some(tokio::spawn(async move {
                    Self::list_matching(root, archived, &query).await
                }));
                if let Some(dialog) = dialog.as_mut() {
                    dialog.note = "Searching saved conversation text… You can keep typing.".into();
                }
            }
        }
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
                    if self.archived {
                        "No archived conversations. Escape returns to chat."
                    } else {
                        "No active conversations. Send a request, or open Archived conversations to restore one."
                    }
                } else if self.archived {
                    "Search all saved conversation text. Enter restores and opens it without replacing another conversation."
                } else {
                    "Recently used first. Search all conversation text. Enter opens it; your draft is kept. Delete archives the selected conversation."
                };
                let mut rows = Self::rows(&conversations);
                if self.archived {
                    rows.push(Choice::new(
                        "Active conversations",
                        "Return to your current conversation list",
                        "sessions:active",
                    ));
                } else {
                    rows.push(Choice::new(
                        "Archived conversations",
                        "Restore conversations removed from the active list",
                        "sessions:archives",
                    ));
                }
                let mut next = Dialog::choices(
                    if self.archived {
                        "Archived conversations"
                    } else {
                        "Switch conversation"
                    },
                    note,
                    rows,
                );
                next.paste(&self.query);
                *dialog = Some(next);
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
                .filter(|row| {
                    row.action.starts_with("sessions:open:")
                        || row.action.starts_with("sessions:restore:")
                })
                .map(|row| (row.label.clone(), row.detail.clone(), row.action.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    async fn wait_for_choices(sessions: &mut Sessions, dialog: &mut Option<Dialog>) {
        for _ in 0..500 {
            sessions.poll(dialog).await;
            if sessions.job.is_none()
                && matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. }))
            {
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
    async fn delete_key_archives_a_conversation_after_confirmation() {
        let root = temp_root();
        let doomed = seed(
            &root,
            "Doomed conversation",
            chrono::Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap(),
        )
        .await;
        let keeper = seed(
            &root,
            "Keeper conversation",
            chrono::Utc.with_ymd_and_hms(2026, 3, 10, 8, 30, 0).unwrap(),
        )
        .await;
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root.clone()));
        wait_for_choices(&mut sessions, &mut dialog).await;
        let mut list = dialog.take().unwrap();
        // Delete on the newest row raises a safe archive confirmation.
        let confirmed = match list.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE)) {
            Outcome::Action(action, _) => action,
            other => panic!("Delete must raise the archive action, got {other:?}"),
        };
        assert_eq!(confirmed, format!("sessions:delete:{doomed}"));
        sessions.action(&confirmed, &mut dialog).await.unwrap();
        let confirm = dialog.as_ref().unwrap();
        assert_eq!(confirm.title, "Archive conversation");
        // Keep is the default; explicitly select Archive before confirming.
        let mut confirm = dialog.take().unwrap();
        confirm.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let performed = match confirm.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
            Outcome::Action(action, _) => action,
            other => panic!("Enter must confirm the delete, got {other:?}"),
        };
        sessions.action(&performed, &mut dialog).await.unwrap();
        wait_for_choices(&mut sessions, &mut dialog).await;
        let remaining = rows(dialog.as_ref().unwrap());
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].2, format!("sessions:open:{keeper}"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_store_shows_a_helpful_empty_state() {
        let root = temp_root();
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root.clone()));
        wait_for_choices(&mut sessions, &mut dialog).await;
        let dialog = dialog.unwrap();
        assert!(dialog.note.contains("No active conversations"));
        assert!(rows(&dialog).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
    /// WHAT: an older conversation used most recently is first in the chooser.
    /// WHY: the developer can return to active work without hunting by creation date.
    #[tokio::test]
    async fn ux_12_recent_work_is_the_first_conversation() {
        let root = temp_root();
        let old = seed(
            &root,
            "Older active work",
            chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        )
        .await;
        seed(
            &root,
            "Newer idle work",
            chrono::Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap(),
        )
        .await;
        FileSessionStore::new(root.clone())
            .append_event(
                old,
                SessionEvent::Custom {
                    key: "run_resumed".into(),
                    payload: serde_json::json!({"at":"2099-01-01T00:00:00Z"}),
                },
            )
            .await
            .unwrap();
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root));
        wait_for_choices(&mut sessions, &mut dialog).await;
        assert!(
            matches!(dialog.as_mut().unwrap().key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == format!("sessions:open:{old}")),
            "PASS only when Enter on the initial row opens the most recently used work"
        );
    }

    /// WHAT: a phrase from a later answer finds the correct saved conversation.
    /// WHY: developers remember discussed errors, not necessarily opening titles.
    #[tokio::test]
    async fn ux_13_search_finds_text_beyond_the_opening_preview() {
        let root = temp_root();
        let id = seed(&root, "Unrelated opening title", chrono::Utc::now()).await;
        FileSessionStore::new(root.clone()).append_event(id, SessionEvent::Custom {
            key: "messages".into(), payload: serde_json::json!([
                {"role":"user","content":"Investigate a build"},
                {"role":"assistant","content":"The later failure was missing symbol frobnicate_42"}
            ]),
        }).await.unwrap();
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root));
        wait_for_choices(&mut sessions, &mut dialog).await;
        dialog.as_mut().unwrap().paste("frobnicate_42");
        wait_for_choices(&mut sessions, &mut dialog).await;
        let dialog = dialog.as_mut().unwrap();
        assert!(
            matches!(dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == format!("sessions:open:{id}")),
            "PASS only when the later-answer search selects its actual conversation"
        );
    }

    /// WHAT: deletion confirmation defaults to Keep and explicit archiving can be undone.
    /// WHY: a routine chooser action must not irreversibly erase engineering history.
    #[tokio::test]
    async fn ux_14_keep_is_default_and_archived_conversation_is_recoverable() {
        let root = temp_root();
        let id = seed(&root, "Work to preserve", chrono::Utc::now()).await;
        FileSessionStore::new(root.clone()).append_event(id, SessionEvent::Custom {
            key: "messages".into(), payload: serde_json::json!([{"role":"user","content":"Original engineering decision"}]),
        }).await.unwrap();
        let record = root.join(".chimera/sessions").join(format!("{id}.json"));
        let bytes = std::fs::read(&record).unwrap();
        let mut sessions = Sessions::default();
        let mut dialog = Some(sessions.open(root.clone()));
        wait_for_choices(&mut sessions, &mut dialog).await;
        sessions
            .action(&format!("sessions:delete:{id}"), &mut dialog)
            .await
            .unwrap();
        let keep = match dialog
            .as_mut()
            .unwrap()
            .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        {
            Outcome::Action(action, _) => action,
            _ => panic!("Keep must be selected initially"),
        };
        assert_eq!(keep, "sessions:retry", "Enter must keep the conversation");
        sessions.action(&keep, &mut dialog).await.unwrap();
        assert_eq!(std::fs::read(&record).unwrap(), bytes);
        sessions
            .action(&format!("sessions:delete:{id}"), &mut dialog)
            .await
            .unwrap();
        let confirm = dialog.as_mut().unwrap();
        confirm.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let archive = match confirm.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
            Outcome::Action(action, _) => action,
            _ => panic!("Archive must be explicitly selected"),
        };
        sessions.action(&archive, &mut dialog).await.unwrap();
        assert_eq!(
            std::fs::read(
                root.join(".chimera/sessions/archived")
                    .join(format!("{id}.json"))
            )
            .unwrap(),
            bytes
        );
        sessions
            .action("sessions:archives", &mut dialog)
            .await
            .unwrap();
        wait_for_choices(&mut sessions, &mut dialog).await;
        let restore = match dialog
            .as_mut()
            .unwrap()
            .key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        {
            Outcome::Action(action, _) => action,
            _ => panic!("Archived work must be selectable"),
        };
        let restored = sessions
            .action(&restore, &mut dialog)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            restored.messages[0].content.as_deref(),
            Some("Original engineering decision"),
            "PASS only when the exact saved conversation opens again after archive/restore"
        );
    }

    /// WHAT: typing the ordinary letter d filters conversations instead of deleting.
    /// WHY: search input must never double as an unexpected destructive shortcut.
    #[test]
    fn ux_15_typing_debug_searches_without_triggering_archive() {
        let id = uuid::Uuid::new_v4();
        let mut dialog = Dialog::choices(
            "Switch conversation",
            "",
            vec![Choice::new(
                "Debug build",
                "Compiler investigation",
                format!("sessions:open:{id}"),
            )
            .with_alt_action(format!("sessions:delete:{id}"))],
        );
        for character in "debug".chars() {
            assert!(matches!(
                dialog.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
                Outcome::Stay
            ));
        }
        assert!(
            matches!(dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == format!("sessions:open:{id}")),
            "PASS only when ordinary search text opens the matching conversation"
        );
    }
}
