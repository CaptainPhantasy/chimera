//! chimera-tui::memory — In-chat project facts: browse, edit, archive, remember.
//!
//! `/memory` lists the saved shared project facts the agent reads, with
//! search, editing and archiving that keeps history. `/remember` shows an
//! explicit confirmation form before anything is written; declining writes
//! nothing.

use crate::dialog::{Choice, Dialog, Field};
use anyhow::{Context, Result};
use chimera_memory::{
    FileMemoryStore, MemoryId, MemoryItem, MemoryLayer, MemoryQuery, MemoryStore,
};
use std::path::PathBuf;

const LIST_CHARS: usize = 80;
const MENU_CHARS: usize = 200;
const LIST_LIMIT: usize = 1000;

/// One saved fact: the shared project records the agent reads as context.
#[derive(Clone)]
struct Fact {
    id: MemoryId,
    key: String,
    content: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl Fact {
    fn from_item(item: MemoryItem) -> Option<Self> {
        // The agent's context only includes facts without a creature scope.
        if item.creature_id.is_some() {
            return None;
        }
        Some(Self {
            id: item.id,
            key: item.key,
            content: item.content,
            created_at: item.created_at,
        })
    }

    /// Cut by characters, not bytes, so CJK text cannot split mid-character.
    fn excerpt(text: &str, chars: usize) -> String {
        let mut cut: String = text.chars().take(chars).collect();
        if text.chars().count() > chars {
            cut.push('…');
        }
        cut
    }

    fn first_line(&self, chars: usize) -> String {
        Self::excerpt(self.content.lines().next().unwrap_or_default(), chars)
    }

    fn date_text(&self) -> String {
        self.created_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    fn choice(&self) -> Choice {
        Choice::new(
            self.key.clone(),
            format!("{} · {}", self.date_text(), self.first_line(LIST_CHARS)),
            format!("memory:fact:{}", self.id),
        )
    }

    fn menu(&self) -> Dialog {
        Dialog::choices(
            "Saved fact",
            &format!(
                "{}\nSaved {}. Choose an action; Esc returns to chat.",
                Self::excerpt(&self.content, MENU_CHARS),
                self.date_text()
            ),
            vec![
                Choice::new(
                    "Edit text",
                    "Change the key or the fact; the previous text stays in history.",
                    format!("memory:edit:{}", self.id),
                ),
                Choice::new(
                    "Archive",
                    "Hide it from the active list and the agent; its record is kept.",
                    format!("memory:archive:{}", self.id),
                ),
            ],
        )
    }
}

/// A fact browser plus remember flow that runs while the chat screen keeps drawing.
#[derive(Default)]
pub struct Memory {
    root: Option<PathBuf>,
    facts: Vec<Fact>,
    flash: Option<String>,
    job: Option<tokio::task::JoinHandle<Result<Vec<Fact>>>>,
}

impl Memory {
    /// Start listing saved project facts and show a waiting panel.
    pub fn open(&mut self, root: PathBuf) -> Dialog {
        self.begin(root, "Loading saved project facts.")
    }

    /// The explicit remember form. Nothing is written until it is confirmed.
    pub fn remember_form(&mut self, root: PathBuf, text: &str) -> Dialog {
        self.cancel();
        self.root = Some(root);
        Dialog::form(
            "Remember a fact",
            "Review before saving. Nothing is written until you choose Save; Esc discards it. Saved facts are shared project facts the agent reads in later requests.",
            vec![Field::new("Key", "", false), Field::new("Text", text, false)],
            "memory:remember",
        )
    }

    fn begin(&mut self, root: PathBuf, note: &str) -> Dialog {
        self.cancel();
        self.root = Some(root.clone());
        self.job = Some(tokio::spawn(async move { Self::list(root).await }));
        Dialog::busy("Project memory", note)
    }

    /// The shared project facts, matching the selection the agent reads.
    async fn list(root: PathBuf) -> Result<Vec<Fact>> {
        let store = FileMemoryStore::new(root);
        let items = store
            .retrieve(MemoryQuery {
                layer: Some(MemoryLayer::Semantic),
                limit: LIST_LIMIT,
                ..Default::default()
            })
            .await
            .context("Could not read the saved project facts.")?;
        Ok(items.into_iter().filter_map(Fact::from_item).collect())
    }

    fn rows(facts: &[Fact]) -> Vec<Choice> {
        facts.iter().map(Fact::choice).collect()
    }

    fn root(&self) -> Result<PathBuf> {
        self.root
            .clone()
            .context("The fact list is not open. Use /memory.")
    }

    fn find(&self, id: MemoryId) -> Result<Fact> {
        self.facts
            .iter()
            .find(|fact| fact.id == id)
            .cloned()
            .context("That fact is no longer in the saved list. Reopen /memory to refresh it.")
    }

    fn parse(raw: &str) -> Result<MemoryId> {
        uuid::Uuid::parse_str(raw)
            .context("That fact could not be identified. Choose it from the list again.")
    }

    /// The confirmed form values. Empty fields refuse the write.
    fn fields(values: &[String]) -> Result<(String, String)> {
        let key = values
            .first()
            .map(String::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let text = values
            .get(1)
            .map(String::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        anyhow::ensure!(
            !key.is_empty(),
            "Give the fact a short key. Nothing was changed."
        );
        anyhow::ensure!(
            !text.is_empty(),
            "The fact text is empty. Nothing was changed."
        );
        Ok((key, text))
    }

    fn confirmed_remember(&self, key: &str, text: &str) -> MemoryItem {
        let now = chrono::Utc::now();
        MemoryItem {
            id: uuid::Uuid::new_v4(),
            layer: MemoryLayer::Semantic,
            key: key.to_string(),
            content: text.to_string(),
            metadata: serde_json::json!({"source": "user"}),
            created_at: now,
            last_accessed: now,
            session_id: None,
            creature_id: None,
            relevance: 1.0,
        }
    }

    /// Abandon a running listing request. Safe when nothing is running.
    /// The pending flash note survives: it describes the last completed
    /// write and is shown by the next finished listing.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.root = None;
        self.facts.clear();
    }

    /// Handle one confirmed overlay choice. `Ok(Some(message))` means a write
    /// completed and the caller should record the message in chat.
    pub async fn action(
        &mut self,
        action: &str,
        values: &[String],
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<String>> {
        if action == "memory:retry" {
            *dialog = Some(self.begin(self.root()?, "Loading saved project facts again."));
        } else if action == "memory:back" {
            *dialog = Some(self.begin(self.root()?, "Loading saved project facts."));
        } else if action == "memory:new" {
            *dialog = Some(self.remember_form(self.root()?, ""));
        } else if let Some(raw) = action.strip_prefix("memory:fact:") {
            let id = Self::parse(raw)?;
            *dialog = Some(self.find(id)?.menu());
        } else if let Some(raw) = action.strip_prefix("memory:edit:") {
            let id = Self::parse(raw)?;
            let fact = self.find(id)?;
            *dialog = Some(Dialog::form(
                "Edit fact",
                "Saving keeps the fact's identity and creation date; the previous text stays in history.",
                vec![
                    Field::new("Key", &fact.key, false),
                    Field::new("Text", &fact.content, false),
                ],
                &format!("memory:save:{id}"),
            ));
        } else if let Some(raw) = action.strip_prefix("memory:save:") {
            let id = Self::parse(raw)?;
            let (key, text) = Self::fields(values)?;
            let updated = FileMemoryStore::new(self.root()?)
                .edit(id, key, text)
                .await
                .context("Could not save the edited fact. Nothing was changed.")?;
            let message = format!(
                "Saved {}. The previous text is kept in project history.",
                updated.key
            );
            self.flash = Some(message.clone());
            *dialog = Some(self.begin(self.root()?, "Reloading the saved project facts."));
            return Ok(Some(message));
        } else if let Some(raw) = action.strip_prefix("memory:archive-confirm:") {
            let id = Self::parse(raw)?;
            FileMemoryStore::new(self.root()?)
                .forget(id)
                .await
                .context("Could not archive that fact. It is still in the active list.")?;
            let message = "Archived. The active list no longer shows it, the agent no longer reads it, and its record is kept in private history.".to_string();
            self.flash = Some(message.clone());
            *dialog = Some(self.begin(self.root()?, "Reloading the saved project facts."));
            return Ok(Some(message));
        } else if let Some(raw) = action.strip_prefix("memory:archive:") {
            let id = Self::parse(raw)?;
            let fact = self.find(id)?;
            *dialog = Some(Dialog::choices(
                "Archive this fact?",
                &format!(
                    "{}\n\nIt leaves the active list and the agent's context. Its record is kept in private history.",
                    Fact::excerpt(&fact.content, MENU_CHARS)
                ),
                vec![
                    Choice::new(
                        "Archive this fact",
                        "Remove it from the active list; keep its record.",
                        format!("memory:archive-confirm:{}", fact.id),
                    ),
                    Choice::new("Keep it", "Leave everything unchanged.", "memory:back"),
                ],
            ));
        } else if action == "memory:remember" {
            let (key, text) = Self::fields(values)?;
            FileMemoryStore::new(self.root()?)
                .remember(self.confirmed_remember(&key, &text))
                .await
                .context("Could not save the fact. Nothing was written.")?;
            let message = format!(
                "Remembered {key}. It is saved as a shared project fact the agent reads from its next request."
            );
            self.flash = Some(message.clone());
            *dialog = Some(self.begin(self.root()?, "Reloading the saved project facts."));
            return Ok(Some(message));
        }
        Ok(None)
    }

    /// Move a finished listing request into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let result = job
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("The fact list stopped loading. Try again.")));
        match result {
            Ok(facts) => {
                self.facts = facts;
                let note = match self.flash.take() {
                    Some(flash) => flash,
                    None if self.facts.is_empty() => {
                        "No saved project facts yet. Use /remember to save one after you confirm its text."
                            .to_string()
                    }
                    None => "Search by key or text. Enter opens a fact to edit or archive it; Esc returns to chat."
                        .to_string(),
                };
                *dialog = Some(Dialog::choices(
                    "Project memory",
                    &note,
                    Self::rows(&self.facts),
                ));
            }
            Err(error) => {
                *dialog = Some(Dialog::choices(
                    "Project memory",
                    &error.to_string(),
                    vec![Choice::new(
                        "Try again",
                        "Reload the saved project facts.",
                        "memory:retry",
                    )],
                ));
            }
        }
    }
}

impl Drop for Memory {
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
        let root = std::env::temp_dir().join(format!(
            "chimera-memory-ui-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    async fn seed(root: &std::path::Path, key: &str, content: &str, relevance: f64) -> MemoryId {
        let store = FileMemoryStore::new(root.to_path_buf());
        let now = chrono::Utc::now();
        store
            .remember(MemoryItem {
                id: uuid::Uuid::new_v4(),
                layer: MemoryLayer::Semantic,
                key: key.into(),
                content: content.into(),
                metadata: serde_json::json!({}),
                created_at: now,
                last_accessed: now,
                session_id: None,
                creature_id: None,
                relevance,
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

    async fn wait_for_choices(memory: &mut Memory, dialog: &mut Option<Dialog>) {
        for _ in 0..500 {
            memory.poll(dialog).await;
            if matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. })) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the fact listing never finished");
    }

    #[test]
    fn excerpts_cut_by_characters_and_facts_skip_creature_scope() {
        let fact = Fact {
            id: uuid::Uuid::new_v4(),
            key: "deploy".into(),
            content: "第一行第二行第三行".into(),
            created_at: chrono::Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap(),
        };
        assert_eq!(fact.first_line(2).chars().count(), 3);
        assert!(fact.first_line(2).ends_with('…'));
        assert_eq!(Fact::excerpt("multi\nline", 20), "multi\nline");
        let mut scoped = make_store_item();
        scoped.creature_id = Some(uuid::Uuid::new_v4());
        assert!(Fact::from_item(scoped).is_none());
    }

    fn make_store_item() -> MemoryItem {
        let now = chrono::Utc::now();
        MemoryItem {
            id: uuid::Uuid::new_v4(),
            layer: MemoryLayer::Semantic,
            key: "scoped".into(),
            content: "creature note".into(),
            metadata: serde_json::json!({}),
            created_at: now,
            last_accessed: now,
            session_id: None,
            creature_id: None,
            relevance: 1.0,
        }
    }

    #[tokio::test]
    async fn rows_are_key_labelled_and_search_reaches_a_fact() {
        let root = temp_root();
        let _payments = seed(&root, "payments", "settle in euros only", 0.9).await;
        let deploy = seed(&root, "deploy", "check the deploy tags", 0.5).await;
        let mut memory = Memory::default();
        let mut dialog = Some(memory.open(root.clone()));
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        wait_for_choices(&mut memory, &mut dialog).await;
        let rows = rows(dialog.as_ref().unwrap());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "payments");
        assert_eq!(rows[1].2, format!("memory:fact:{deploy}"));
        assert!(rows[0].1.contains("settle in euros only"));
        assert!(rows[1].1.contains("check the deploy tags"));
        let mut dialog = dialog.unwrap();
        dialog.paste("tags");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == format!("memory:fact:{deploy}")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn editing_a_fact_persists_the_new_text_and_shows_on_reopen() {
        let root = temp_root();
        let id = seed(&root, "deploy", "check the deploy tags", 0.9).await;
        let mut memory = Memory::default();
        let mut dialog = Some(memory.open(root.clone()));
        wait_for_choices(&mut memory, &mut dialog).await;
        // Opening the fact and choosing edit prefills the form from the store.
        memory
            .action(&format!("memory:fact:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        let note = dialog.as_ref().unwrap().note.clone();
        assert!(note.contains("check the deploy tags"));
        memory
            .action(&format!("memory:edit:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        let mut dialog = dialog.unwrap();
        let Body::Form { fields, .. } = &dialog.body else {
            panic!("editing must open a form");
        };
        assert_eq!(fields[0].input.text(), "deploy");
        assert_eq!(fields[1].input.text(), "check the deploy tags");
        // Enter on the last field submits the real form action.
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Stay
        ));
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, values)
                if action == format!("memory:save:{id}")
                    && values == vec!["deploy".to_string(), "check the deploy tags".to_string()]
        ));
        let dialog = &mut Some(dialog);
        let saved = memory
            .action(
                &format!("memory:save:{id}"),
                &["deploy".to_string(), "check the deploy tags v2".to_string()],
                dialog,
            )
            .await
            .unwrap();
        assert!(saved
            .expect("an edit must report its result")
            .contains("Saved deploy"));
        wait_for_choices(&mut memory, dialog).await;
        assert!(rows(dialog.as_ref().unwrap())[0]
            .1
            .contains("check the deploy tags v2"));
        let store = FileMemoryStore::new(root.clone());
        let listed = store
            .retrieve(MemoryQuery {
                layer: Some(MemoryLayer::Semantic),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed[0].content, "check the deploy tags v2");
        let history = store.archived().await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].reason, "edited");
        assert_eq!(history[0].item.content, "check the deploy tags");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn archiving_requires_confirmation_then_hides_but_keeps_history() {
        let root = temp_root();
        let id = seed(&root, "deploy", "check the deploy tags", 0.9).await;
        let mut memory = Memory::default();
        let mut dialog = Some(memory.open(root.clone()));
        wait_for_choices(&mut memory, &mut dialog).await;
        // Opening the archive choice is not the destructive step.
        memory
            .action(&format!("memory:archive:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        let menu = rows(dialog.as_ref().unwrap());
        assert!(menu
            .iter()
            .any(|(_, _, a)| a == &format!("memory:archive-confirm:{id}")));
        let listed = FileMemoryStore::new(root.clone())
            .retrieve(MemoryQuery::default())
            .await
            .unwrap();
        assert_eq!(listed.len(), 1, "nothing may be written before confirming");
        // Confirming archives: hidden from the active list, kept in history.
        let message = memory
            .action(&format!("memory:archive-confirm:{id}"), &[], &mut dialog)
            .await
            .unwrap();
        assert!(message
            .expect("an archive must report its result")
            .contains("Archived"));
        wait_for_choices(&mut memory, &mut dialog).await;
        assert!(rows(dialog.as_ref().unwrap()).is_empty());
        assert!(dialog.as_ref().unwrap().note.contains("Archived."));
        let store = FileMemoryStore::new(root.clone());
        assert!(store
            .retrieve(MemoryQuery::default())
            .await
            .unwrap()
            .is_empty());
        let history = store.archived().await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].item.key, "deploy");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_remember_requires_confirmation_and_declining_writes_nothing() {
        let root = temp_root();
        let mut memory = Memory::default();
        let mut dialog = Some(memory.remember_form(root.clone(), "check the deploy tags"));
        let Body::Form { fields, .. } = &dialog.as_ref().unwrap().body else {
            panic!("remembering must show a confirmation form");
        };
        assert_eq!(fields[1].input.text(), "check the deploy tags");
        assert!(FileMemoryStore::new(root.clone())
            .retrieve(MemoryQuery::default())
            .await
            .unwrap()
            .is_empty());
        // Esc declines: the overlay closes and nothing is written.
        assert!(matches!(
            dialog
                .as_mut()
                .unwrap()
                .key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Outcome::Close
        ));
        assert!(FileMemoryStore::new(root.clone())
            .retrieve(MemoryQuery::default())
            .await
            .unwrap()
            .is_empty());
        // Confirming through the real form path writes the fact.
        let mut dialog = Some(memory.remember_form(root.clone(), "check the deploy tags"));
        dialog.as_mut().unwrap().paste("deploy-tags");
        dialog
            .as_mut()
            .unwrap()
            .key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let dialog = &mut dialog;
        assert!(matches!(
            dialog.as_mut().unwrap().key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, values)
                if action == "memory:remember"
                    && values == vec!["deploy-tags".to_string(), "check the deploy tags".to_string()]
        ));
        let remembered = memory
            .action(
                "memory:remember",
                &[
                    "deploy-tags".to_string(),
                    "check the deploy tags".to_string(),
                ],
                dialog,
            )
            .await
            .unwrap();
        assert!(remembered
            .expect("a remember must report its result")
            .contains("Remembered deploy-tags"));
        wait_for_choices(&mut memory, dialog).await;
        let listed_rows = rows(dialog.as_ref().unwrap());
        assert_eq!(listed_rows[0].0, "deploy-tags");
        let listed = FileMemoryStore::new(root.clone())
            .retrieve(MemoryQuery {
                layer: Some(MemoryLayer::Semantic),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_or_blank_confirmation_writes_nothing() {
        let root = temp_root();
        let mut memory = Memory::default();
        let mut dialog = Some(memory.remember_form(root.clone(), ""));
        let error = memory
            .action(
                "memory:remember",
                &["deploy-tags".to_string(), String::new()],
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("fact text is empty"));
        let error = memory
            .action(
                "memory:remember",
                &[String::new(), "some text".to_string()],
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Give the fact a short key"));
        assert!(FileMemoryStore::new(root.clone())
            .retrieve(MemoryQuery::default())
            .await
            .unwrap()
            .is_empty());
        assert!(dialog.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_bad_edit_target_errors_and_changes_nothing() {
        let root = temp_root();
        seed(&root, "deploy", "check the deploy tags", 0.9).await;
        let mut memory = Memory::default();
        let mut dialog = Some(memory.open(root.clone()));
        wait_for_choices(&mut memory, &mut dialog).await;
        let unknown = uuid::Uuid::new_v4();
        let error = memory
            .action(
                &format!("memory:save:{unknown}"),
                &["k".to_string(), "v".to_string()],
                &mut dialog,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Could not save the edited fact"));
        let error = memory
            .action(&format!("memory:edit:{unknown}"), &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("no longer in the saved list"));
        let error = memory
            .action("memory:fact:not-a-uuid", &[], &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("could not be identified"));
        assert!(dialog.is_some());
        let listed = FileMemoryStore::new(root.clone())
            .retrieve(MemoryQuery::default())
            .await
            .unwrap();
        assert_eq!(listed[0].content, "check the deploy tags");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_clears_an_in_flight_listing_job() {
        let root = temp_root();
        seed(&root, "deploy", "check the deploy tags", 0.9).await;
        let mut memory = Memory::default();
        let _busy = memory.open(root.clone());
        assert!(memory.job.is_some());
        memory.cancel();
        assert!(memory.job.is_none());
        assert!(memory.root.is_none());
        assert!(memory.facts.is_empty());
        let mut dialog = None;
        memory.poll(&mut dialog).await;
        assert!(dialog.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_store_shows_a_helpful_empty_state() {
        let root = temp_root();
        let mut memory = Memory::default();
        let mut dialog = Some(memory.open(root.clone()));
        wait_for_choices(&mut memory, &mut dialog).await;
        let dialog = dialog.unwrap();
        assert!(dialog.note.contains("No saved project facts"));
        assert!(rows(&dialog).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
