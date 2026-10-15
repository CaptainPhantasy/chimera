//! chimera-tui::skills — In-chat reusable procedures.
//!
//! Lists saved procedures for this project, captures the current conversation
//! as a new one after an explicit name and description step, and runs the
//! chosen procedure as a normal request in this conversation through the
//! ordinary agent path, so it appears as a regular turn with the usual
//! approvals.

use crate::dialog::{Choice, Dialog, Field};
use anyhow::{Context, Result};
use chimera_core::skill::{CapturedSkill, FileSkillCapture};
use std::path::PathBuf;

const PREVIEW_CHARS: usize = 80;

/// One listed procedure: what it is called, what it does and what a run needs.
struct Procedure {
    name: String,
    description: String,
    version: String,
    captured_at: chrono::DateTime<chrono::Utc>,
    tools: Vec<String>,
    objective: String,
}

impl Procedure {
    fn new(captured: CapturedSkill) -> Self {
        Self {
            name: captured.spec.name,
            description: captured.spec.description,
            version: captured.spec.version,
            captured_at: captured.captured_at,
            tools: captured.spec.required_tools,
            objective: captured.spec.objective_template,
        }
    }

    fn date_text(&self) -> String {
        self.captured_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    fn tools_text(&self) -> String {
        match self.tools.len() {
            0 => "no recorded tools".to_string(),
            1 => "1 tool".to_string(),
            count => format!("{count} tools"),
        }
    }

    fn choice(&self) -> Choice {
        Choice::new(
            self.name.clone(),
            format!(
                "v{} · {} · {} · {}",
                self.version,
                self.date_text(),
                self.description,
                self.tools_text()
            ),
            format!("skills:choose:{}", self.name),
        )
    }

    /// What selecting this procedure shows before anything is sent.
    fn confirmation(&self) -> Dialog {
        let mut chars = self.objective.lines().next().unwrap_or_default().chars();
        let mut preview: String = chars.by_ref().take(PREVIEW_CHARS).collect();
        if chars.next().is_some() {
            preview.push('…');
        }
        Dialog::choices(
            &format!("Run '{}'", self.name),
            "The procedure's objective is sent as your next request in this conversation. It runs as a normal turn through the usual approvals; no historic tool calls are replayed.",
            vec![
                Choice::new(
                    format!("Run '{}' now", self.name),
                    preview,
                    format!("skills:invoke:{}", self.name),
                ),
                Choice::new(
                    "Back to the procedure list",
                    "Return without sending anything.",
                    "skills:list",
                ),
            ],
        )
    }
}

/// What a finished background job hands back to the overlay.
enum Finished {
    Listed(Vec<Procedure>),
    Captured(String, Vec<Procedure>),
}

/// A procedure listing or capture request that runs while chat keeps drawing.
#[derive(Default)]
pub struct Skills {
    root: Option<PathBuf>,
    session: Option<uuid::Uuid>,
    procedures: Vec<Procedure>,
    pending: Option<(String, String)>,
    job: Option<tokio::task::JoinHandle<Result<Finished>>>,
}

impl Skills {
    /// Start listing saved procedures and show a waiting panel.
    pub fn open(&mut self, root: PathBuf, session: Option<uuid::Uuid>) -> Dialog {
        self.begin(root, session, "Loading saved procedures for this project.")
    }

    fn begin(&mut self, root: PathBuf, session: Option<uuid::Uuid>, note: &str) -> Dialog {
        self.cancel();
        self.root = Some(root.clone());
        self.session = session;
        self.job = Some(tokio::spawn(async move {
            Ok(Finished::Listed(Self::list(root).await?))
        }));
        Dialog::busy("Saved procedures", note)
    }

    /// Every saved procedure for this project, newest capture first.
    async fn list(root: PathBuf) -> Result<Vec<Procedure>> {
        let captured = FileSkillCapture::new(root).list_skills().await?;
        Ok(captured.into_iter().map(Procedure::new).collect())
    }

    /// The exact request text for one saved procedure, from the real store.
    pub async fn objective(root: PathBuf, name: &str) -> Result<String> {
        FileSkillCapture::new(root)
            .execution_objective(name)
            .await
            .with_context(|| format!("'{name}' is not an available procedure in this project."))
    }

    fn listing(procedures: &[Procedure], can_capture: bool) -> Dialog {
        let mut rows: Vec<Choice> = procedures.iter().map(Procedure::choice).collect();
        if can_capture {
            rows.push(Choice::new(
                "Capture this conversation as a procedure",
                "Records this conversation's objective and tools under a name you choose. Nothing is sent or run.",
                "skills:capture",
            ));
        }
        let note = if procedures.is_empty() {
            "No saved procedures yet. After a request finishes here, choose 'Capture this conversation as a procedure' to save its objective and tools for reuse."
        } else {
            "Search by name, description or date. Enter shows what will run; running a procedure sends a normal request with the usual approvals. Esc returns to chat."
        };
        Dialog::choices("Saved procedures", note, rows)
    }

    fn capture_form(name: &str, description: &str, note: &str) -> Dialog {
        Dialog::form(
            "Capture this conversation",
            note,
            vec![
                Field::new("Procedure name (letters, digits, hyphens)", name, false),
                Field::new("Description", description, false),
            ],
            "skills:capture:save",
        )
    }

    /// Abandon a running listing or capture request. Safe when nothing runs.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.root = None;
        self.session = None;
        self.procedures.clear();
        self.pending = None;
    }

    /// Handle one confirmed overlay choice. `Ok(Some(objective))` means the
    /// caller should send that text as the current request in this chat.
    pub async fn action(
        &mut self,
        action: &str,
        values: &[String],
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<String>> {
        if action == "skills:retry" || action == "skills:list" {
            let root = self
                .root
                .clone()
                .context("The procedure list is not open. Use /skills.")?;
            let session = self.session;
            *dialog = Some(self.begin(root, session, "Reloading saved procedures."));
        } else if action == "skills:capture" {
            anyhow::ensure!(
                self.session.is_some(),
                "The current conversation is not available for capture. Use /skills from an open conversation."
            );
            *dialog = Some(Self::capture_form(
                "",
                "",
                "Name the procedure. Saving records the objective and tools of this conversation; nothing is sent or run.",
            ));
        } else if action == "skills:capture:save" {
            anyhow::ensure!(values.len() == 2, "The capture form is incomplete.");
            let name = values[0].trim().to_string();
            let description = values[1].trim().to_string();
            anyhow::ensure!(
                !name.is_empty(),
                "Enter a name for this procedure. Nothing was saved."
            );
            anyhow::ensure!(
                name.len() <= 128
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
                "Use only letters, digits, hyphens or underscores in the name; nothing was saved."
            );
            let root = self
                .root
                .clone()
                .context("The procedure list is not open. Use /skills.")?;
            let session = self
                .session
                .context("The current conversation is not available for capture.")?;
            self.cancel();
            self.root = Some(root.clone());
            self.session = Some(session);
            self.pending = Some((name.clone(), description.clone()));
            let note = format!("Recording this conversation's objective and tools as '{name}'.");
            self.job = Some(tokio::spawn(async move {
                let store = FileSkillCapture::new(root.clone());
                store
                    .capture_from_session(&name, &description, session)
                    .await?;
                Ok(Finished::Captured(name, Self::list(root).await?))
            }));
            *dialog = Some(Dialog::busy("Saving procedure", &note));
        } else if let Some(name) = action.strip_prefix("skills:choose:") {
            let procedure = self
                .procedures
                .iter()
                .find(|procedure| procedure.name == name)
                .context("That procedure is not in the saved list. Reload with /skills.")?;
            *dialog = Some(procedure.confirmation());
        } else if let Some(name) = action.strip_prefix("skills:invoke:") {
            let root = self
                .root
                .clone()
                .context("The procedure list is not open. Use /skills.")?;
            let objective = Self::objective(root, name).await?;
            self.cancel();
            *dialog = None;
            return Ok(Some(objective));
        }
        Ok(None)
    }

    /// Move a finished request into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let result = job
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("The procedure request stopped. Try again.")));
        match result {
            Ok(Finished::Listed(procedures)) => {
                let can_capture = self.session.is_some();
                let listing = Self::listing(&procedures, can_capture);
                self.procedures = procedures;
                *dialog = Some(listing);
            }
            Ok(Finished::Captured(name, procedures)) => {
                let can_capture = self.session.is_some();
                let mut saved = Self::listing(&procedures, can_capture);
                saved.note = format!(
                    "Saved '{name}'. Enter runs it in this conversation as a normal request; Esc returns to chat."
                );
                self.procedures = procedures;
                *dialog = Some(saved);
            }
            Err(error) => {
                if let Some((name, description)) = self.pending.take() {
                    if self.session.is_some() {
                        *dialog = Some(Self::capture_form(&name, &description, &error.to_string()));
                        return;
                    }
                }
                *dialog = Some(Dialog::choices(
                    "Saved procedures",
                    &error.to_string(),
                    vec![Choice::new(
                        "Try again",
                        "Reload the saved procedure list.",
                        "skills:retry",
                    )],
                ));
            }
        }
    }
}

impl Drop for Skills {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use chimera_core::skill::{SkillCapture, SkillSpec};
    use chimera_session::{SessionEvent, SessionMeta, SessionStore};
    use chrono::TimeZone;

    fn temp_root() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "chimera-skills-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    /// A saved, completed conversation with one recorded tool call, so
    /// `capture_from_session` accepts it as a source.
    async fn completed_session(
        root: &std::path::Path,
        objective: &str,
    ) -> anyhow::Result<uuid::Uuid> {
        let store = chimera_session::FileSessionStore::new(root.to_path_buf());
        let id = store
            .create(SessionMeta {
                label: objective.chars().take(60).collect(),
                objective: objective.into(),
                created_at: chrono::Utc::now(),
            })
            .await?;
        store
            .append_event_sync(
                id,
                SessionEvent::ToolCalled {
                    tool: "read_file".into(),
                    args: serde_json::json!({"path":"sample.txt"}),
                },
            )
            .unwrap();
        store.close(id, "completed").await?;
        Ok(id)
    }

    fn captured(
        name: &str,
        source: uuid::Uuid,
        captured_at: chrono::DateTime<chrono::Utc>,
    ) -> CapturedSkill {
        CapturedSkill {
            id: uuid::Uuid::new_v4(),
            spec: SkillSpec {
                name: name.into(),
                description: format!("the {name} procedure"),
                version: "1".into(),
                objective_template: format!("carry out {name}"),
                required_creatures: vec![],
                required_tools: vec!["read_file".into()],
                pre_hooks: vec![],
                post_hooks: vec![],
            },
            source_session: source,
            captured_at,
            confidence: 0.0,
        }
    }

    /// One completed source conversation plus two captured procedures with
    /// stable capture times, newest last.
    async fn seeded(root: &std::path::Path) -> anyhow::Result<uuid::Uuid> {
        let session = completed_session(root, "inspect the sample file").await?;
        let store = FileSkillCapture::new(root);
        store
            .capture(captured(
                "older-skill",
                session,
                chrono::Utc.with_ymd_and_hms(2026, 3, 10, 8, 30, 0).unwrap(),
            ))
            .await?;
        store
            .capture(captured(
                "newer-skill",
                session,
                chrono::Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap(),
            ))
            .await?;
        Ok(session)
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

    async fn wait_for_choices(
        skills: &mut Skills,
        dialog: &mut Option<Dialog>,
    ) -> Vec<(String, String, String)> {
        for _ in 0..500 {
            skills.poll(dialog).await;
            if matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. })) {
                return rows(dialog.as_ref().unwrap());
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the procedure listing never finished");
    }

    #[tokio::test]
    async fn listing_reflects_saved_procedures_newest_first_with_detail() {
        let root = temp_root();
        seeded(&root).await.unwrap();
        let mut skills = Skills::default();
        let session = Some(uuid::Uuid::new_v4());
        let mut dialog = Some(skills.open(root.clone(), session));
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        let rows = wait_for_choices(&mut skills, &mut dialog).await;
        assert_eq!(rows.len(), 3, "two procedures plus the capture row");
        assert_eq!(rows[0].0, "newer-skill");
        assert_eq!(rows[1].0, "older-skill");
        assert_eq!(rows[0].2, "skills:choose:newer-skill");
        // The detail names the version, capture month, description and tools.
        assert!(rows[0].1.starts_with("v1 · 2026-06"));
        assert!(rows[0].1.contains("the newer-skill procedure"));
        assert!(rows[0].1.contains("1 tool"));
        // The capture row invites saving the current conversation.
        assert_eq!(rows[2].2, "skills:capture");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_project_shows_a_helpful_empty_state_and_capture_row() {
        let root = temp_root();
        std::fs::create_dir_all(&root).unwrap();
        let mut skills = Skills::default();
        let mut dialog = Some(skills.open(root.clone(), Some(uuid::Uuid::new_v4())));
        let rows = wait_for_choices(&mut skills, &mut dialog).await;
        let dialog = dialog.unwrap();
        assert!(dialog.note.contains("No saved procedures yet"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2, "skills:capture");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn type_to_search_and_enter_targets_the_chosen_procedure() {
        let root = temp_root();
        seeded(&root).await.unwrap();
        let mut skills = Skills::default();
        let mut dialog = Some(skills.open(root.clone(), None));
        wait_for_choices(&mut skills, &mut dialog).await;
        let mut dialog = dialog.unwrap();
        dialog.paste("older");
        assert!(matches!(
            dialog.key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE
            )),
            Outcome::Action(action, _) if action == "skills:choose:older-skill"
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn capture_requires_an_explicit_non_empty_name_and_changes_nothing() {
        let root = temp_root();
        let session = seeded(&root).await.unwrap();
        let mut skills = Skills::default();
        skills.root = Some(root.clone());
        skills.session = Some(session);
        let saved_before = FileSkillCapture::new(&root)
            .list_skills()
            .await
            .unwrap()
            .len();
        let mut dialog = Some(Dialog::busy("Saved procedures", "starting"));
        let error = skills
            .action(
                "skills:capture:save",
                &["".into(), "desc".into()],
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Enter a name"));
        assert!(dialog.is_some());
        assert!(
            skills.job.is_none(),
            "no capture may start for an empty name"
        );
        let error = skills
            .action(
                "skills:capture:save",
                &["bad name!".into(), "desc".into()],
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("letters, digits, hyphens"));
        assert!(dialog.is_some());
        let saved_after = FileSkillCapture::new(&root)
            .list_skills()
            .await
            .unwrap()
            .len();
        assert_eq!(
            saved_after, saved_before,
            "a refusal must not save anything"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn capture_saves_and_the_procedure_then_appears_in_the_list() {
        let root = temp_root();
        let session = completed_session(&root, "inspect the sample file")
            .await
            .unwrap();
        let mut skills = Skills::default();
        let mut dialog = Some(skills.open(root.clone(), Some(session)));
        wait_for_choices(&mut skills, &mut dialog).await;
        skills
            .action(
                "skills:capture:save",
                &["deploy-check".into(), "Deploy checklist".into()],
                &mut dialog,
            )
            .await
            .unwrap();
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        let rows = wait_for_choices(&mut skills, &mut dialog).await;
        assert!(
            dialog
                .as_ref()
                .unwrap()
                .note
                .contains("Saved 'deploy-check'"),
            "the save must be confirmed by name"
        );
        assert!(rows.iter().any(|(label, _, _)| label == "deploy-check"));
        // The procedure is really persisted, so a fresh list shows it too.
        let reopened = FileSkillCapture::new(&root);
        let listed = reopened.list_skills().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].spec.description, "Deploy checklist");
        assert_eq!(listed[0].spec.required_tools, vec!["read_file"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn capturing_without_a_completed_conversation_reports_the_real_error() {
        let root = temp_root();
        let store = chimera_session::FileSessionStore::new(root.clone());
        let session = store
            .create(SessionMeta {
                label: "open".into(),
                objective: "never finished".into(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        let mut skills = Skills::default();
        skills.root = Some(root.clone());
        skills.session = Some(session);
        let mut dialog = Some(Dialog::busy("Saved procedures", "starting"));
        skills
            .action(
                "skills:capture:save",
                &["too-early".into(), "desc".into()],
                &mut dialog,
            )
            .await
            .unwrap();
        for _ in 0..500 {
            skills.poll(&mut dialog).await;
            if skills.job.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let dialog = dialog.unwrap();
        assert!(dialog.note.contains("completed saved source session"));
        assert!(matches!(
            FileSkillCapture::new(&root).list_skills().await,
            Ok(listed) if listed.is_empty()
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn invoking_a_chosen_procedure_yields_its_execution_objective() {
        let root = temp_root();
        seeded(&root).await.unwrap();
        let mut skills = Skills::default();
        skills.root = Some(root.clone());
        skills.session = Some(uuid::Uuid::new_v4());
        let mut dialog = Some(Dialog::busy("Saved procedures", "starting"));
        let error = skills
            .action("skills:choose:ghost", &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not in the saved list"));
        assert!(dialog.is_some());
        // List first so the chooser has the procedures, then confirm and run.
        let mut dialog = Some(skills.open(root.clone(), None));
        wait_for_choices(&mut skills, &mut dialog).await;
        skills
            .action("skills:choose:newer-skill", &[], &mut dialog)
            .await
            .unwrap();
        let confirmation = dialog.as_ref().unwrap();
        assert!(confirmation.title.contains("Run 'newer-skill'"));
        let confirmation_rows = rows(confirmation);
        assert_eq!(confirmation_rows[0].2, "skills:invoke:newer-skill");
        assert_eq!(confirmation_rows[1].2, "skills:list");
        let objective = skills
            .action("skills:invoke:newer-skill", &[], &mut dialog)
            .await
            .unwrap()
            .expect("invoking must yield the request text");
        assert!(objective.contains("Run the saved workflow 'newer-skill'"));
        assert!(objective.contains("carry out newer-skill"));
        assert!(objective.contains("current permissions"));
        assert!(dialog.is_none(), "the overlay closes when the run starts");
        assert!(skills.root.is_none(), "the in-flight state is cleared");
        // An unknown invocation is refused and changes nothing. The list is
        // open again here, so the refusal is about the name, not the overlay.
        skills.root = Some(root.clone());
        skills.session = Some(uuid::Uuid::new_v4());
        skills.procedures.clear();
        let mut dialog = Some(Dialog::busy("Saved procedures", "starting"));
        let error = skills
            .action("skills:invoke:ghost", &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not an available procedure"));
        assert!(dialog.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn back_to_the_list_reloads_without_sending_anything() {
        let root = temp_root();
        seeded(&root).await.unwrap();
        let mut skills = Skills::default();
        let mut dialog = Some(skills.open(root.clone(), None));
        wait_for_choices(&mut skills, &mut dialog).await;
        skills
            .action("skills:choose:newer-skill", &[], &mut dialog)
            .await
            .unwrap();
        skills
            .action("skills:list", &[], &mut dialog)
            .await
            .unwrap();
        let listed = wait_for_choices(&mut skills, &mut dialog).await;
        assert!(listed.iter().any(|(label, _, _)| label == "newer-skill"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_clears_an_in_flight_request() {
        let root = temp_root();
        seeded(&root).await.unwrap();
        let mut skills = Skills::default();
        let _busy = skills.open(root.clone(), Some(uuid::Uuid::new_v4()));
        assert!(skills.job.is_some());
        skills.cancel();
        assert!(skills.job.is_none());
        assert!(skills.root.is_none());
        assert!(skills.session.is_none());
        assert!(skills.procedures.is_empty());
        let mut dialog = None;
        skills.poll(&mut dialog).await;
        assert!(dialog.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
