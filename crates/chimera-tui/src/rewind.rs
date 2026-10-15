//! chimera-tui::rewind — In-chat restore of files edited by earlier turns.
//!
//! Each turn that edits files leaves one automatic snapshot. This overlay lists
//! them newest-first, previews an exact restore, and confirms before touching
//! anything; the current file versions are preserved in a private archive.

use crate::dialog::{Choice, Dialog};
use anyhow::{Context, Result};
use chimera_llm::Message;
use chimera_session::{
    CheckpointSummary, FileSessionStore, RestorePreview, SessionEvent, SessionState,
};
use std::path::PathBuf;
use uuid::Uuid;

/// One restorable snapshot of a turn's edited files.
struct Snapshot {
    id: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    files: Vec<PathBuf>,
}

impl Snapshot {
    fn from_summary(summary: CheckpointSummary) -> Self {
        Self {
            id: summary.id,
            created_at: summary.created_at,
            files: summary.files,
        }
    }

    fn date_text(&self) -> String {
        self.created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    fn choice(&self) -> Choice {
        let files = self
            .files
            .iter()
            .map(|file| file.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Choice::new(
            format!("Turn of {}", self.date_text()),
            format!("{} file(s) edited: {}", self.files.len(), files),
            format!("rewind:select:{}", self.id),
        )
    }
}

/// A rewind request that runs while the chat screen keeps drawing.
#[derive(Default)]
pub struct Rewind {
    root: Option<PathBuf>,
    session: Option<Uuid>,
    read_only: bool,
    snapshots: Vec<Snapshot>,
    preview: Option<RestorePreview>,
    job: Option<Job>,
}

enum Job {
    List(tokio::task::JoinHandle<Result<Vec<Snapshot>>>),
    Preview(tokio::task::JoinHandle<Result<RestorePreview>>),
}

impl Job {
    fn is_finished(&self) -> bool {
        match self {
            Job::List(job) => job.is_finished(),
            Job::Preview(job) => job.is_finished(),
        }
    }
}

impl Rewind {
    /// Start listing this conversation's snapshots and show a waiting panel.
    pub fn open(&mut self, root: PathBuf, session: Uuid, read_only: bool) -> Dialog {
        self.begin(
            root,
            session,
            read_only,
            "Loading snapshots of the files this conversation edited.",
        )
    }

    fn begin(&mut self, root: PathBuf, session: Uuid, read_only: bool, note: &str) -> Dialog {
        self.cancel();
        self.root = Some(root.clone());
        self.session = Some(session);
        self.read_only = read_only;
        self.preview = None;
        self.job = Some(Job::List(tokio::spawn(async move {
            FileSessionStore::new(root)
                .list_checkpoints(session)
                .await
                .map(|summaries| summaries.into_iter().map(Snapshot::from_summary).collect())
        })));
        Dialog::busy("Rewind files", note)
    }

    fn targets(&self) -> Result<(PathBuf, Uuid)> {
        let root = self
            .root
            .clone()
            .context("The rewind list is not open. Use /rewind.")?;
        let session = self
            .session
            .context("The rewind list is not open. Use /rewind.")?;
        Ok((root, session))
    }

    /// Abandon a running request. Safe when nothing is running.
    pub fn cancel(&mut self) {
        match self.job.take() {
            Some(Job::List(job)) => job.abort(),
            Some(Job::Preview(job)) => job.abort(),
            None => {}
        }
        self.preview = None;
    }

    /// Handle one confirmed overlay choice. `Ok(Some(messages))` means the
    /// restore happened and the conversation should show the restored history.
    pub async fn action(
        &mut self,
        action: &str,
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<Vec<Message>>> {
        if action == "rewind:retry" {
            let (root, session) = self.targets()?;
            *dialog = Some(self.begin(
                root,
                session,
                self.read_only,
                "Loading the snapshot list again.",
            ));
        } else if let Some(id_text) = action.strip_prefix("rewind:select:") {
            let id = Uuid::parse_str(id_text)
                .context("That snapshot could not be identified. Choose it from the list again.")?;
            let (root, session) = self.targets()?;
            self.snapshots
                .iter()
                .find(|snapshot| snapshot.id == id)
                .context("That snapshot is no longer listed. Reload the list.")?;
            self.cancel();
            self.job = Some(Job::Preview(tokio::spawn(async move {
                FileSessionStore::new(root)
                    .preview_restore(session, id)
                    .await
            })));
            *dialog = Some(Dialog::busy(
                "Rewind files",
                "Comparing the snapshot with the current files.",
            ));
        } else if let Some(id_text) = action.strip_prefix("rewind:confirm:") {
            let id = Uuid::parse_str(id_text)
                .context("That snapshot could not be identified. Choose it from the list again.")?;
            anyhow::ensure!(
                !self.read_only,
                "read-only policy does not allow restoring files; leave plan mode to rewind"
            );
            let preview = self
                .preview
                .as_ref()
                .filter(|preview| preview.checkpoint_id == id)
                .context(
                    "Preview this snapshot again before restoring; the files may have changed.",
                )?;
            let (root, session) = self.targets()?;
            let expected = preview.current_files.clone();
            let state = FileSessionStore::new(root)
                .restore_checkpoint(session, id, &expected)
                .await?;
            let messages = restored_messages(&state)?;
            self.cancel();
            self.root = None;
            self.session = None;
            self.snapshots.clear();
            *dialog = None;
            return Ok(Some(messages));
        }
        Ok(None)
    }

    /// Move a finished request into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        let Some(job) = self.job.as_ref() else {
            return;
        };
        if !job.is_finished() {
            return;
        }
        match self.job.take().expect("the finished job is still held") {
            Job::List(job) => {
                let result = job.await.unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "The snapshot list stopped loading. Try again."
                    ))
                });
                match result {
                    Ok(snapshots) => {
                        let note = if snapshots.is_empty() {
                            "No snapshots exist yet. A snapshot of the edited files is taken automatically after any turn that edits files, so rewind becomes available once a turn has edited a file."
                        } else if self.read_only {
                            "Snapshots are newest first. Search by file or date. Restoring is refused while plan (read-only) mode is active."
                        } else {
                            "Snapshots are newest first. Search by file or date. Enter previews exactly what would change."
                        };
                        self.snapshots = snapshots;
                        *dialog = Some(Dialog::choices(
                            "Rewind files",
                            note,
                            Self::rows(&self.snapshots),
                        ));
                    }
                    Err(error) => {
                        *dialog = Some(Dialog::choices(
                            "Rewind files",
                            &error.to_string(),
                            vec![Choice::new(
                                "Try again",
                                "Reload the snapshot list.",
                                "rewind:retry",
                            )],
                        ));
                    }
                }
            }
            Job::Preview(job) => {
                let result = job.await.unwrap_or_else(|_| {
                    Err(anyhow::anyhow!("The restore preview stopped. Try again."))
                });
                match result {
                    Ok(preview) => {
                        let note = confirm_note(&preview);
                        let id = preview.checkpoint_id;
                        self.preview = Some(preview);
                        *dialog = Some(Dialog::choices(
                            "Confirm restore",
                            &note,
                            vec![Choice::new(
                                "Restore these files",
                                "Rewind now; Esc leaves every file unchanged.",
                                format!("rewind:confirm:{id}"),
                            )],
                        ));
                    }
                    Err(error) => {
                        *dialog = Some(Dialog::choices(
                            "Rewind files",
                            &error.to_string(),
                            vec![Choice::new(
                                "Back to snapshots",
                                "Reload the snapshot list.",
                                "rewind:retry",
                            )],
                        ));
                    }
                }
            }
        }
    }

    fn rows(snapshots: &[Snapshot]) -> Vec<Choice> {
        snapshots.iter().map(Snapshot::choice).collect()
    }
}

fn confirm_note(preview: &RestorePreview) -> String {
    if preview.restored_files.is_empty() {
        return "This snapshot captured no file contents, so restoring it only returns the conversation to the point right after that turn.".into();
    }
    let files = preview
        .restored_files
        .iter()
        .map(|file| file.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Restoring rewrites {} file(s) exactly as they were at that turn: {}. The current versions are preserved in a private archive first, and the conversation returns to the point right after that turn.",
        preview.restored_files.len(),
        files
    )
}

/// The restored conversation replaces the in-memory history, matching resume.
fn restored_messages(state: &SessionState) -> Result<Vec<Message>> {
    Ok(state
        .effective_events()?
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::Custom { key, payload } if key == "messages" => Some(payload.clone()),
            _ => None,
        })
        .map(serde_json::from_value)
        .transpose()
        .context("the restored conversation is damaged")?
        .unwrap_or_default())
}

impl Drop for Rewind {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use chimera_session::SessionStore;
    use chrono::TimeZone;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "chimera-rewind-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
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

    async fn wait_for_body(
        rewind: &mut Rewind,
        dialog: &mut Option<Dialog>,
        wanted: &dyn Fn(&Dialog) -> bool,
    ) {
        for _ in 0..500 {
            rewind.poll(dialog).await;
            if dialog.as_ref().is_some_and(wanted) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the rewind request never finished");
    }

    fn is_choices(dialog: &Dialog) -> bool {
        matches!(dialog.body, Body::Choices { .. })
    }

    /// A session with one checkpoint that captured `name`, plus `newer_first`
    /// seconds of clock separation so the newest-first order is deterministic.
    async fn seeded_session(
        root: &std::path::Path,
        older: &str,
        newer: Option<&str>,
    ) -> (Uuid, Uuid, Option<Uuid>) {
        let store = FileSessionStore::new(root.to_path_buf());
        let session = store
            .create(chimera_session::SessionMeta {
                label: "rewind test".into(),
                objective: "rewind test objective".into(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        std::fs::write(root.join(older), b"older captured bytes").unwrap();
        let first = store
            .checkpoint_with_files(session, &[PathBuf::from(older)])
            .await
            .unwrap();
        let second = match newer {
            None => None,
            Some(name) => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                std::fs::write(root.join(name), b"newer captured bytes").unwrap();
                Some(
                    store
                        .checkpoint_with_files(session, &[PathBuf::from(name)])
                        .await
                        .unwrap(),
                )
            }
        };
        (session, first, second)
    }

    #[tokio::test]
    async fn snapshots_list_newest_first_with_paths() {
        let root = temp_root();
        let (session, first, second) = seeded_session(&root, "note.txt", Some("other.txt")).await;
        let second = second.expect("the newer checkpoint exists");
        let mut rewind = Rewind::default();
        let mut dialog = Some(rewind.open(root.clone(), session, false));
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        wait_for_body(&mut rewind, &mut dialog, &is_choices).await;
        let rows = rows(dialog.as_ref().unwrap());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].2, format!("rewind:select:{second}"));
        assert_eq!(rows[1].2, format!("rewind:select:{first}"));
        assert!(rows[0].1.contains("other.txt"), "{}", rows[0].1);
        assert!(rows[1].1.contains("note.txt"), "{}", rows[1].1);
        assert!(rows[0].1.starts_with("1 file(s) edited"));
        // The label is a readable local date and time; the year-month prefix is
        // stable under any timezone shift.
        assert!(rows[0].0.starts_with("Turn of "));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn typing_filters_and_enter_targets_the_chosen_snapshot() {
        let root = temp_root();
        let (session, _first, _second) = seeded_session(&root, "note.txt", Some("other.txt")).await;
        let mut rewind = Rewind::default();
        let mut dialog = Some(rewind.open(root.clone(), session, false));
        wait_for_body(&mut rewind, &mut dialog, &is_choices).await;
        let mut dialog = dialog.unwrap();
        dialog.paste("note");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action.starts_with("rewind:select:")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_history_explains_the_automatic_snapshots() {
        let root = temp_root();
        let store = FileSessionStore::new(root.clone());
        let empty = store
            .create(chimera_session::SessionMeta {
                label: "empty".into(),
                objective: "empty objective".into(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        let mut rewind = Rewind::default();
        let mut dialog = Some(rewind.open(root.clone(), empty, false));
        wait_for_body(&mut rewind, &mut dialog, &is_choices).await;
        let dialog = dialog.unwrap();
        assert!(dialog.note.contains("No snapshots exist yet"));
        assert!(dialog
            .note
            .contains("automatically after any turn that edits files"));
        assert!(rows(&dialog).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn confirm_uses_the_preview_files_and_refuses_a_stale_one() {
        let root = temp_root();
        let (session, _first, _second) = seeded_session(&root, "note.txt", None).await;
        let mut rewind = Rewind::default();
        let mut dialog = Some(rewind.open(root.clone(), session, false));
        wait_for_body(&mut rewind, &mut dialog, &is_choices).await;
        // Select the snapshot, then the preview names the exact file.
        let error = match rewind.action("rewind:select:not-a-uuid", &mut dialog).await {
            Err(error) => error,
            Ok(_) => panic!("a garbage id must be refused"),
        };
        assert!(error.to_string().contains("could not be identified"));
        assert!(dialog.is_some());
        let id_text = rows(dialog.as_ref().unwrap())[0].2.clone();
        rewind.action(&id_text, &mut dialog).await.unwrap();
        wait_for_body(&mut rewind, &mut dialog, &|dialog| {
            dialog.title == "Confirm restore"
        })
        .await;
        let confirm_note_text = dialog.as_ref().unwrap().note.clone();
        assert!(confirm_note_text.contains("note.txt"));
        assert!(confirm_note_text.contains("preserved in a private archive"));
        // An edit after the preview makes this exact preview's current files
        // stale, so the restore refuses instead of overwriting new work.
        std::fs::write(root.join("note.txt"), b"edited after the preview").unwrap();
        let confirm = format!("rewind:confirm:{}", &id_text["rewind:select:".len()..]);
        let error = rewind.action(&confirm, &mut dialog).await.unwrap_err();
        assert!(error.to_string().contains("changed since restore preview"));
        assert_eq!(
            std::fs::read(root.join("note.txt")).unwrap(),
            b"edited after the preview"
        );
        assert!(dialog.is_some());
        // Confirming against the still-current preview restores exactly.
        assert!(rewind
            .action(&id_text, &mut dialog)
            .await
            .unwrap()
            .is_none());
        wait_for_body(&mut rewind, &mut dialog, &|dialog| {
            dialog.title == "Confirm restore"
        })
        .await;
        let messages = rewind.action(&confirm, &mut dialog).await.unwrap().unwrap();
        assert_eq!(
            std::fs::read(root.join("note.txt")).unwrap(),
            b"older captured bytes"
        );
        // This fixture session never ran, so the restored history is empty;
        // the extraction path still completed without inventing messages.
        assert!(messages.is_empty());
        // The overlay closes on success; the module keeps no stale state.
        assert!(dialog.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn read_only_mode_refuses_the_restore_and_changes_nothing() {
        let root = temp_root();
        let (session, _first, _second) = seeded_session(&root, "note.txt", None).await;
        let mut rewind = Rewind::default();
        let mut dialog = Some(rewind.open(root.clone(), session, true));
        wait_for_body(&mut rewind, &mut dialog, &is_choices).await;
        let id_text = rows(dialog.as_ref().unwrap())[0].2.clone();
        rewind.action(&id_text, &mut dialog).await.unwrap();
        wait_for_body(&mut rewind, &mut dialog, &|dialog| {
            dialog.title == "Confirm restore"
        })
        .await;
        let confirm = format!("rewind:confirm:{}", &id_text["rewind:select:".len()..]);
        let error = rewind.action(&confirm, &mut dialog).await.unwrap_err();
        assert!(error
            .to_string()
            .contains("read-only policy does not allow restoring files"));
        assert_eq!(
            std::fs::read(root.join("note.txt")).unwrap(),
            b"older captured bytes"
        );
        assert!(dialog.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn confirming_without_a_preview_is_refused() {
        let root = temp_root();
        let (session, _first, _second) = seeded_session(&root, "note.txt", None).await;
        let mut rewind = Rewind::default();
        rewind.root = Some(root.clone());
        rewind.session = Some(session);
        let mut dialog = Some(Dialog::busy("Rewind files", "starting"));
        let confirm = format!("rewind:confirm:{}", uuid::Uuid::new_v4());
        let error = rewind.action(&confirm, &mut dialog).await.unwrap_err();
        assert!(error.to_string().contains("Preview this snapshot again"));
        assert!(dialog.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unknown_snapshot_reports_an_error_and_keeps_the_overlay() {
        let root = temp_root();
        let (session, _first, _second) = seeded_session(&root, "note.txt", None).await;
        let mut rewind = Rewind::default();
        let mut dialog = Some(rewind.open(root.clone(), session, false));
        wait_for_body(&mut rewind, &mut dialog, &is_choices).await;
        let unknown = uuid::Uuid::new_v4();
        let error = match rewind
            .action(&format!("rewind:select:{unknown}"), &mut dialog)
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("an unknown snapshot must be refused"),
        };
        assert!(error.to_string().contains("no longer listed"));
        assert!(dialog.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_clears_an_in_flight_request() {
        let root = temp_root();
        let (session, _first, _second) = seeded_session(&root, "note.txt", None).await;
        let mut rewind = Rewind::default();
        let _busy = rewind.open(root.clone(), session, false);
        assert!(rewind.job.is_some());
        rewind.cancel();
        assert!(rewind.job.is_none());
        assert!(rewind.preview.is_none());
        let mut dialog = None;
        rewind.poll(&mut dialog).await;
        assert!(dialog.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn local_dates_render_as_readable_turn_labels() {
        let created = chrono::Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap();
        let snapshot = Snapshot {
            id: uuid::Uuid::new_v4(),
            created_at: created,
            files: vec![PathBuf::from("note.txt")],
        };
        let choice = snapshot.choice();
        assert!(choice.label.starts_with("Turn of 2026-06"));
        assert!(choice.detail.contains("note.txt"));
        assert_eq!(choice.action, format!("rewind:select:{}", snapshot.id));
        assert_eq!(
            Snapshot::from_summary(CheckpointSummary {
                id: snapshot.id,
                created_at: created,
                files: snapshot.files.clone(),
            })
            .choice()
            .action,
            choice.action
        );
    }
}
