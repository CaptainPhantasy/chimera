//! chimera-tui::permissions — One place in chat to review and change permissions.
//!
//! Shows the saved project policy (autonomy, read-only, disabled tools) and the
//! pending approvals together. Every change is saved through the real policy
//! store, so the working agent applies it on its next tool call. Anything that
//! allows more access asks for an explicit confirmation first; narrowing takes
//! effect at once and every outcome is reported in the conversation.

use crate::dialog::{Choice, Dialog};
use crate::message::{Message, SystemKind};
use anyhow::{Context, Result};
use chimera_core::approval::{display_value, ApprovalStore, Decision, PolicyStore, ProjectPolicy};
use chimera_tools::{DefaultToolRouter, ToolDescriptor, ToolRouter};
use std::path::PathBuf;

const AUTONOMY_LEVELS: [&str; 3] = ["none", "low", "medium"];

/// One pending action that waits for a decision.
struct PendingApproval {
    id: uuid::Uuid,
    tool: String,
    detail: String,
}

/// The saved policy, the real tool list and the open approvals, as last loaded.
struct PermissionsView {
    policy: ProjectPolicy,
    tools: Vec<ToolDescriptor>,
    pending: Vec<PendingApproval>,
}

fn autonomy_rank(level: &str) -> Option<usize> {
    AUTONOMY_LEVELS.iter().position(|known| *known == level)
}

fn autonomy_detail(level: &str) -> &'static str {
    match level {
        "none" => "Stop every tool. Requests answer with words only.",
        "low" => "Every tool action needs its own approval.",
        _ => "File changes and commands need approval; lookups run on their own.",
    }
}

/// A permissions review that runs while the chat screen keeps drawing.
#[derive(Default)]
pub struct Permissions {
    root: Option<PathBuf>,
    plan: bool,
    view: Option<PermissionsView>,
    job: Option<tokio::task::JoinHandle<Result<PermissionsView>>>,
}

impl Permissions {
    /// Start loading the saved policy and show a waiting panel.
    pub fn open(&mut self, root: PathBuf, plan: bool) -> Dialog {
        let note = if plan {
            "Loading this project's permissions. This screen runs in plan mode; the saved policy below applies on top of it."
        } else {
            "Loading this project's permissions."
        };
        self.begin(root, plan, note)
    }

    fn begin(&mut self, root: PathBuf, plan: bool, note: &str) -> Dialog {
        self.cancel();
        self.root = Some(root.clone());
        self.plan = plan;
        self.job = Some(tokio::spawn(async move { Self::load(root).await }));
        Dialog::busy("Permissions", note)
    }

    /// The saved policy, every registered tool and the still-pending approvals.
    async fn load(root: PathBuf) -> Result<PermissionsView> {
        let policy = PolicyStore::new(root.clone()).get()?;
        let mut tools = DefaultToolRouter::new(Some(root.clone())).list().await?;
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        let mut pending = Vec::new();
        for row in ApprovalStore::new(root).list()? {
            if row.decision != Decision::Pending {
                continue;
            }
            pending.push(PendingApproval {
                id: row.id,
                tool: row.call.tool.clone(),
                detail: format!(
                    "{} · requested {} · expires {}",
                    display_value(row.call.args),
                    row.created_at.with_timezone(&chrono::Local).format("%H:%M"),
                    row.expires_at.with_timezone(&chrono::Local).format("%H:%M")
                ),
            });
        }
        Ok(PermissionsView {
            policy,
            tools,
            pending,
        })
    }

    fn loaded(&self) -> Result<&PermissionsView> {
        self.view.as_ref().context(
            "The permissions list has not loaded yet. Wait a moment or reopen /permissions.",
        )
    }

    fn known_tool(&self, name: &str) -> Result<()> {
        let view = self.loaded()?;
        anyhow::ensure!(
            view.tools.iter().any(|tool| tool.name == name),
            "unknown tool: {name}"
        );
        Ok(())
    }

    fn rows(view: &PermissionsView) -> Vec<Choice> {
        let mut rows = Vec::new();
        for level in AUTONOMY_LEVELS {
            let current = view.policy.autonomy == level;
            let label = if current {
                format!("Autonomy: {level} — current")
            } else {
                format!("Autonomy: {level}")
            };
            let action = if autonomy_rank(level) > autonomy_rank(&view.policy.autonomy) {
                format!("permissions:ask-autonomy:{level}")
            } else {
                format!("permissions:set-autonomy:{level}")
            };
            rows.push(Choice::new(label, autonomy_detail(level), action));
        }
        if view.policy.read_only {
            rows.push(Choice::new(
                "Project read-only: on — writes are denied",
                "Select to allow file changes and commands again; it will ask to confirm.",
                "permissions:ask-readonly:off",
            ));
        } else {
            rows.push(Choice::new(
                "Project read-only: off",
                "Select to deny file changes and commands in this project.",
                "permissions:set-readonly:on",
            ));
        }
        for tool in &view.tools {
            let disabled = view.policy.disabled_tools.contains(&tool.name);
            let (label, action) = if disabled {
                (
                    format!("{} — disabled", tool.name),
                    format!("permissions:ask-enable:{}", tool.name),
                )
            } else {
                (
                    format!("{} — enabled", tool.name),
                    format!("permissions:set-disable:{}", tool.name),
                )
            };
            let detail = if disabled {
                format!(
                    "{} Select to enable it again; it will ask to confirm.",
                    tool.description
                )
            } else {
                format!(
                    "{} Select to disable it for this project.",
                    tool.description
                )
            };
            rows.push(Choice::new(label, detail, action));
        }
        for waiting in &view.pending {
            rows.push(Choice::new(
                format!("Pending approval: {}", waiting.tool),
                format!("{} · Select to approve once or deny.", waiting.detail),
                format!("permissions:review:{}", waiting.id),
            ));
        }
        rows
    }

    fn note(view: &PermissionsView, plan: bool) -> String {
        let mode = if plan {
            "This screen runs in plan mode; the saved policy below applies on top of it. "
        } else {
            ""
        };
        let waiting = if view.pending.is_empty() {
            "No actions are waiting for approval.".to_string()
        } else {
            format!("{} action(s) waiting for approval.", view.pending.len())
        };
        format!(
            "{mode}Autonomy {} · project read-only {}. Type to search; Enter changes a row. Anything that allows more access asks first. {waiting}",
            view.policy.autonomy,
            if view.policy.read_only { "on" } else { "off" }
        )
    }

    fn main_view(&self) -> Dialog {
        match &self.view {
            Some(view) => Dialog::choices(
                "Permissions",
                &Self::note(view, self.plan),
                Self::rows(view),
            ),
            None => Dialog::busy("Permissions", "Loading this project's permissions."),
        }
    }

    /// Reload the saved state and repaint the overlay with what is really stored.
    async fn repaint(&mut self, dialog: &mut Option<Dialog>) -> Result<()> {
        let root = self
            .root
            .clone()
            .context("The permissions panel is not open. Use /permissions.")?;
        let view = Self::load(root).await?;
        self.view = Some(view);
        *dialog = Some(self.main_view());
        Ok(())
    }

    fn report(text: String, kind: SystemKind) -> Option<Message> {
        Some(Message::System { text, kind })
    }

    /// Save one policy change, repaint the overlay and return the chat report.
    async fn apply(
        &mut self,
        dialog: &mut Option<Dialog>,
        change: impl FnOnce(&mut ProjectPolicy) -> Result<()>,
    ) -> Result<ProjectPolicy> {
        let root = self
            .root
            .clone()
            .context("The permissions panel is not open. Use /permissions.")?;
        let saved = PolicyStore::new(root).change(change)?;
        self.repaint(dialog).await?;
        Ok(saved)
    }

    /// Abandon a running listing request. Safe when nothing is running.
    pub fn cancel(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.root = None;
        self.plan = false;
        self.view = None;
    }

    /// Handle one confirmed overlay choice. `Ok(Some(message))` is a report to
    /// add to the conversation; the overlay stays open with the real saved state.
    pub async fn action(
        &mut self,
        action: &str,
        dialog: &mut Option<Dialog>,
    ) -> Result<Option<Message>> {
        let root = self
            .root
            .clone()
            .context("The permissions panel is not open. Use /permissions.")?;
        if action == "permissions:back" {
            *dialog = Some(self.main_view());
            return Ok(None);
        }
        if action == "permissions:refresh" {
            *dialog =
                Some(self.begin(root, self.plan, "Loading this project's permissions again."));
            return Ok(None);
        }
        if let Some(level) = action.strip_prefix("permissions:set-autonomy:") {
            let level = level.trim().to_lowercase();
            anyhow::ensure!(
                AUTONOMY_LEVELS.contains(&level.as_str()),
                "autonomy must be none, low or medium; automatic approval of mutations is unavailable"
            );
            let current = self.loaded()?.policy.autonomy.clone();
            if current == level {
                return Ok(Self::report(
                    format!("Autonomy is already {level}; nothing changed."),
                    SystemKind::Info,
                ));
            }
            let saved = self
                .apply(dialog, move |policy| {
                    policy.autonomy = level;
                    Ok(())
                })
                .await?;
            return Ok(Self::report(
                format!(
                    "Autonomy saved: {}. {}",
                    saved.autonomy,
                    autonomy_detail(&saved.autonomy)
                ),
                SystemKind::Success,
            ));
        }
        if let Some(level) = action.strip_prefix("permissions:ask-autonomy:") {
            let level = level.trim().to_lowercase();
            anyhow::ensure!(
                AUTONOMY_LEVELS.contains(&level.as_str()),
                "autonomy must be none, low or medium; automatic approval of mutations is unavailable"
            );
            let current = self.loaded()?.policy.autonomy.clone();
            anyhow::ensure!(
                level != current,
                "Autonomy is already {current}; nothing to confirm."
            );
            *dialog = Some(Dialog::choices(
                "Allow more autonomy",
                &format!(
                    "This project would allow {level}: {}. Esc keeps {current}.",
                    autonomy_detail(&level)
                ),
                vec![
                    Choice::new(
                        format!("Allow {level}"),
                        "Save this autonomy for the project.",
                        format!("permissions:set-autonomy:{level}"),
                    ),
                    Choice::new(
                        "Keep current permissions",
                        format!("Leave autonomy as {current}."),
                        "permissions:back",
                    ),
                ],
            ));
            return Ok(None);
        }
        match action.strip_prefix("permissions:set-readonly:") {
            Some("on") => {
                anyhow::ensure!(
                    !self.loaded()?.policy.read_only,
                    "The project is already read-only; nothing changed."
                );
                let saved = self
                    .apply(dialog, |policy| {
                        policy.read_only = true;
                        Ok(())
                    })
                    .await?;
                return Ok(Self::report(
                    format!(
                        "Project read-only saved: {}. File changes and commands are denied until you change it back.",
                        if saved.read_only { "on" } else { "off" }
                    ),
                    SystemKind::Success,
                ));
            }
            Some("off") => {
                anyhow::ensure!(
                    self.loaded()?.policy.read_only,
                    "The project already allows changes; nothing changed."
                );
                let saved = self
                    .apply(dialog, |policy| {
                        policy.read_only = false;
                        Ok(())
                    })
                    .await?;
                return Ok(Self::report(
                    format!(
                        "Project read-only saved: {}. File changes and commands are allowed again; each still needs its approval when required.",
                        if saved.read_only { "on" } else { "off" }
                    ),
                    SystemKind::Success,
                ));
            }
            _ => {}
        }
        if action == "permissions:ask-readonly:off" {
            anyhow::ensure!(
                self.loaded()?.policy.read_only,
                "The project already allows changes; nothing to confirm."
            );
            *dialog = Some(Dialog::choices(
                "Allow changes again",
                "Turning read-only off lets the agent change files and run commands in this project again; each still needs its approval when required. Esc keeps the project read-only.",
                vec![
                    Choice::new(
                        "Allow changes",
                        "Save the wider permission for this project.",
                        "permissions:set-readonly:off",
                    ),
                    Choice::new(
                        "Keep the project read-only",
                        "Change nothing.",
                        "permissions:back",
                    ),
                ],
            ));
            return Ok(None);
        }
        if let Some(name) = action.strip_prefix("permissions:set-enable:") {
            self.known_tool(name)?;
            anyhow::ensure!(
                self.loaded()?.policy.disabled_tools.contains(name),
                "{name} is already enabled; nothing changed."
            );
            let owned = name.to_string();
            self.apply(dialog, move |policy| {
                policy.disabled_tools.remove(&owned);
                Ok(())
            })
            .await?;
            return Ok(Self::report(
                format!("{name} is enabled; normal action approvals still apply."),
                SystemKind::Success,
            ));
        }
        if let Some(name) = action.strip_prefix("permissions:ask-enable:") {
            self.known_tool(name)?;
            anyhow::ensure!(
                self.loaded()?.policy.disabled_tools.contains(name),
                "{name} is not disabled; nothing to confirm."
            );
            *dialog = Some(Dialog::choices(
                "Allow a disabled tool again",
                &format!(
                    "This project would allow the agent to call {name} again; normal action approvals still apply. Esc keeps it disabled."
                ),
                vec![
                    Choice::new(
                        format!("Allow {name}"),
                        "Save the wider permission for this project.",
                        format!("permissions:set-enable:{name}"),
                    ),
                    Choice::new(
                        "Keep it disabled",
                        "Change nothing.",
                        "permissions:back",
                    ),
                ],
            ));
            return Ok(None);
        }
        if let Some(name) = action.strip_prefix("permissions:set-disable:") {
            self.known_tool(name)?;
            anyhow::ensure!(
                !self.loaded()?.policy.disabled_tools.contains(name),
                "{name} is already disabled; nothing changed."
            );
            let owned = name.to_string();
            self.apply(dialog, move |policy| {
                policy.disabled_tools.insert(owned);
                Ok(())
            })
            .await?;
            return Ok(Self::report(
                format!("{name} is disabled; the agent cannot call it in this project."),
                SystemKind::Success,
            ));
        }
        if let Some(id) = action.strip_prefix("permissions:review:") {
            let id = uuid::Uuid::parse_str(id)
                .context("That approval could not be identified. Choose it from the list again.")?;
            let row = ApprovalStore::new(root).get(id)?;
            anyhow::ensure!(
                row.decision == Decision::Pending,
                "action is no longer pending: {:?}",
                row.decision
            );
            *dialog = Some(Dialog::choices(
                "Pending approval",
                &format!(
                    "Approve lets this exact action run once when its request rechecks the decision; deny refuses it. Esc keeps it waiting. {} {}",
                    row.call.tool, display_value(row.call.args)
                ),
                vec![
                    Choice::new(
                        format!("Approve {} once", row.call.tool),
                        "Allow this exact action to run.",
                        format!("permissions:approve:{}", row.id),
                    ),
                    Choice::new(
                        format!("Deny {} once", row.call.tool),
                        "Refuse this action; the request is told it was denied.",
                        format!("permissions:deny:{}", row.id),
                    ),
                    Choice::new(
                        "Keep waiting",
                        "Leave the decision for later.",
                        "permissions:back",
                    ),
                ],
            ));
            return Ok(None);
        }
        for (prefix, approved) in [("permissions:approve:", true), ("permissions:deny:", false)] {
            let Some(id) = action.strip_prefix(prefix) else {
                continue;
            };
            let id = uuid::Uuid::parse_str(id)
                .context("That approval could not be identified. Choose it from the list again.")?;
            let store = ApprovalStore::new(root.clone());
            let row = store.get(id)?;
            anyhow::ensure!(
                row.decision == Decision::Pending,
                "action is no longer pending: {:?}",
                row.decision
            );
            store.decide(id, approved)?;
            self.repaint(dialog).await?;
            return Ok(if approved {
                Self::report(
                    format!(
                        "Approved {} once. The request continues when it rechecks this decision; an approval that is never used expires.",
                        row.call.tool
                    ),
                    SystemKind::Success,
                )
            } else {
                Self::report(
                    format!(
                        "Denied {}. The request is told the action was refused.",
                        row.call.tool
                    ),
                    SystemKind::Info,
                )
            });
        }
        anyhow::bail!("That permissions choice is not offered anymore. Choose from the list.")
    }

    /// Move a finished listing request into the overlay.
    pub async fn poll(&mut self, dialog: &mut Option<Dialog>) {
        if !self.job.as_ref().is_some_and(|job| job.is_finished()) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let result = job.await.unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "The permissions list stopped loading. Try again."
            ))
        });
        match result {
            Ok(view) => {
                self.view = Some(view);
                *dialog = Some(self.main_view());
            }
            Err(error) => {
                *dialog = Some(Dialog::choices(
                    "Permissions",
                    &error.to_string(),
                    vec![Choice::new(
                        "Try again",
                        "Reload the saved policy and pending approvals.",
                        "permissions:refresh",
                    )],
                ));
            }
        }
    }
}

impl Drop for Permissions {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::{Body, Outcome};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "chimera-permissions-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
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

    fn labels(dialog: &Dialog) -> Vec<String> {
        rows(dialog)
            .into_iter()
            .map(|(label, _, _)| label)
            .collect()
    }

    async fn wait_for_choices(permissions: &mut Permissions, dialog: &mut Option<Dialog>) {
        for _ in 0..500 {
            permissions.poll(dialog).await;
            if matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Choices { .. })) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the permissions listing never finished");
    }

    async fn stored(root: &std::path::Path) -> ProjectPolicy {
        PolicyStore::new(root.to_path_buf()).get().unwrap()
    }

    async fn request_approval(root: &std::path::Path, tool: &str) -> uuid::Uuid {
        ApprovalStore::new(root.to_path_buf())
            .request(
                uuid::Uuid::new_v4(),
                chimera_tools::ToolCall {
                    tool: tool.into(),
                    args: serde_json::json!({"path": "answer.txt", "content": "yes"}),
                    world: None,
                    requested_by: uuid::Uuid::new_v4(),
                },
                std::time::Duration::from_secs(60),
            )
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn the_overlay_reflects_current_mode_disabled_tools_and_pending() {
        let root = temp_root();
        PolicyStore::new(root.clone())
            .change(|policy| {
                policy.autonomy = "low".into();
                policy.disabled_tools.insert("bash".into());
                Ok(())
            })
            .unwrap();
        let waiting = request_approval(&root, "write_file").await;
        let mut permissions = Permissions::default();
        let mut dialog = Some(permissions.open(root.clone(), true));
        assert!(matches!(dialog.as_ref().map(|d| &d.body), Some(Body::Busy)));
        wait_for_choices(&mut permissions, &mut dialog).await;
        let dialog = dialog.as_ref().unwrap();
        assert!(dialog.note.contains("plan mode"));
        let shown = labels(dialog);
        assert!(shown.contains(&"Autonomy: low — current".to_string()));
        assert!(shown.contains(&"Autonomy: medium".to_string()));
        assert!(shown.contains(&"Project read-only: off".to_string()));
        assert!(shown.contains(&"bash — disabled".to_string()));
        assert!(shown.contains(&"write_file — enabled".to_string()));
        assert!(shown.contains(&"read_file — enabled".to_string()));
        assert!(shown
            .iter()
            .any(|label| label == "Pending approval: write_file"));
        assert!(rows(dialog)
            .iter()
            .any(|(_, _, action)| action == &format!("permissions:review:{waiting}")));
        // The current autonomy row is offered through the apply action; a wider
        // row goes through the confirmation first.
        let actions: Vec<_> = rows(dialog).into_iter().map(|(_, _, a)| a).collect();
        assert!(actions.contains(&"permissions:set-autonomy:none".to_string()));
        assert!(actions.contains(&"permissions:ask-autonomy:medium".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn toggling_a_tool_persists_and_is_visible_on_reopen() {
        let root = temp_root();
        let mut permissions = Permissions::default();
        let mut dialog = Some(permissions.open(root.clone(), false));
        wait_for_choices(&mut permissions, &mut dialog).await;
        let report = permissions
            .action("permissions:set-disable:bash", &mut dialog)
            .await
            .unwrap()
            .expect("a disabling reports into the conversation");
        assert!(report_text(&report).contains("bash is disabled"));
        assert!(stored(&root).await.disabled_tools.contains("bash"));
        let shown = labels(dialog.as_ref().unwrap());
        assert!(shown.contains(&"bash — disabled".to_string()));
        // Reopening loads the persisted state from disk.
        let mut permissions = Permissions::default();
        let mut dialog = Some(permissions.open(root.clone(), false));
        wait_for_choices(&mut permissions, &mut dialog).await;
        assert!(labels(dialog.as_ref().unwrap()).contains(&"bash — disabled".to_string()));
        // Enabling is widening: it must ask before it changes anything.
        permissions
            .action("permissions:ask-enable:bash", &mut dialog)
            .await
            .unwrap();
        assert!(stored(&root).await.disabled_tools.contains("bash"));
        let actions: Vec<_> = rows(dialog.as_ref().unwrap())
            .into_iter()
            .map(|(_, _, action)| action)
            .collect();
        assert!(actions.contains(&"permissions:set-enable:bash".to_string()));
        permissions
            .action("permissions:set-enable:bash", &mut dialog)
            .await
            .unwrap();
        assert!(!stored(&root).await.disabled_tools.contains("bash"));
        assert!(labels(dialog.as_ref().unwrap()).contains(&"bash — enabled".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn report_text(message: &Message) -> String {
        match message {
            Message::System { text, .. } => text.clone(),
            other => panic!("expected a system message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_pending_approval_can_be_approved_and_denied_through_the_overlay() {
        let root = temp_root();
        let denied = request_approval(&root, "write_file").await;
        let mut permissions = Permissions::default();
        let mut dialog = Some(permissions.open(root.clone(), false));
        wait_for_choices(&mut permissions, &mut dialog).await;
        permissions
            .action(&format!("permissions:review:{denied}"), &mut dialog)
            .await
            .unwrap();
        let report = permissions
            .action(&format!("permissions:deny:{denied}"), &mut dialog)
            .await
            .unwrap()
            .expect("a denial reports into the conversation");
        assert!(report_text(&report).contains("Denied write_file"));
        let store = ApprovalStore::new(root.clone());
        assert_eq!(store.get(denied).unwrap().decision, Decision::Denied);
        let approved = request_approval(&root, "edit_file").await;
        permissions
            .action(&format!("permissions:review:{approved}"), &mut dialog)
            .await
            .unwrap();
        permissions
            .action(&format!("permissions:approve:{approved}"), &mut dialog)
            .await
            .unwrap();
        assert_eq!(store.get(approved).unwrap().decision, Decision::Approved);
        // The decided approvals leave the pending list.
        assert!(!labels(dialog.as_ref().unwrap())
            .iter()
            .any(|label| label.contains("Pending approval")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unknown_tool_or_mode_is_refused_and_changes_nothing() {
        let root = temp_root();
        let mut permissions = Permissions::default();
        let mut dialog = Some(permissions.open(root.clone(), false));
        wait_for_choices(&mut permissions, &mut dialog).await;
        for action in [
            "permissions:set-autonomy:full",
            "permissions:ask-autonomy:banana",
            "permissions:set-disable:no-such-tool",
            "permissions:set-enable:no-such-tool",
            "permissions:set-readonly:sideways",
        ] {
            let error = permissions
                .action(action, &mut dialog)
                .await
                .expect_err("an invalid choice must be refused");
            assert!(
                error.to_string().contains("unknown tool")
                    || error.to_string().contains("none, low or medium")
                    || error.to_string().contains("not offered"),
                "user-facing refusal for {action}: {error}"
            );
            assert!(dialog.is_some());
        }
        let policy = stored(&root).await;
        assert_eq!(policy.autonomy, "medium");
        assert!(!policy.read_only);
        assert!(policy.disabled_tools.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn narrowing_applies_at_once_and_widening_needs_a_confirmation() {
        let root = temp_root();
        let mut permissions = Permissions::default();
        let mut dialog = Some(permissions.open(root.clone(), false));
        wait_for_choices(&mut permissions, &mut dialog).await;
        // Narrowing to none applies straight away.
        let report = permissions
            .action("permissions:set-autonomy:none", &mut dialog)
            .await
            .unwrap()
            .unwrap();
        assert!(report_text(&report).contains("Autonomy saved: none"));
        assert_eq!(stored(&root).await.autonomy, "none");
        // Widening back to medium asks first and stores nothing yet.
        permissions
            .action("permissions:ask-autonomy:medium", &mut dialog)
            .await
            .unwrap();
        assert_eq!(stored(&root).await.autonomy, "none");
        let note = &dialog.as_ref().unwrap().note;
        assert!(note.contains("would allow medium"));
        permissions
            .action("permissions:set-autonomy:medium", &mut dialog)
            .await
            .unwrap();
        assert_eq!(stored(&root).await.autonomy, "medium");
        // An equal choice is an honest no-op.
        let report = permissions
            .action("permissions:set-autonomy:medium", &mut dialog)
            .await
            .unwrap()
            .unwrap();
        assert!(report_text(&report).contains("already medium"));
        // Read-only on is narrowing; off is widening and asks first.
        permissions
            .action("permissions:set-readonly:on", &mut dialog)
            .await
            .unwrap();
        assert!(stored(&root).await.read_only);
        permissions
            .action("permissions:ask-readonly:off", &mut dialog)
            .await
            .unwrap();
        assert!(stored(&root).await.read_only);
        permissions
            .action("permissions:set-readonly:off", &mut dialog)
            .await
            .unwrap();
        assert!(!stored(&root).await.read_only);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_clears_an_in_flight_listing_job() {
        let root = temp_root();
        let mut permissions = Permissions::default();
        let _busy = permissions.open(root.clone(), false);
        assert!(permissions.job.is_some());
        permissions.cancel();
        assert!(permissions.job.is_none());
        assert!(permissions.root.is_none());
        assert!(permissions.view.is_none());
        let mut dialog = None;
        permissions.poll(&mut dialog).await;
        assert!(dialog.is_none());
        // Actions after a close are refused without touching storage.
        let error = permissions
            .action("permissions:set-autonomy:none", &mut dialog)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not open"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_damaged_store_shows_an_error_with_a_retry() {
        let root = temp_root();
        std::fs::create_dir_all(root.join(".chimera/policy")).unwrap();
        std::fs::write(root.join(".chimera/policy/settings.json"), "damaged").unwrap();
        let mut permissions = Permissions::default();
        let mut dialog = Some(permissions.open(root.clone(), false));
        for _ in 0..500 {
            permissions.poll(&mut dialog).await;
            let recovery = dialog.as_ref().is_some_and(|d| match &d.body {
                Body::Choices { rows, .. } => {
                    rows.len() == 1 && rows[0].action == "permissions:refresh"
                }
                _ => false,
            });
            if recovery {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let dialog = dialog.expect("an error view is shown");
        assert!(matches!(dialog.body, Body::Choices { .. }));
        assert!(rows(&dialog)
            .iter()
            .any(|(_, _, action)| action == "permissions:refresh"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn searching_filters_rows_and_enter_targets_the_chosen_row() {
        let view = PermissionsView {
            policy: ProjectPolicy::default(),
            tools: vec![
                ToolDescriptor {
                    name: "bash".into(),
                    description: "Run a shell command".into(),
                    read_only: false,
                    category: chimera_tools::ToolCategory::LocalExecution,
                },
                ToolDescriptor {
                    name: "glob".into(),
                    description: "Find files by pattern".into(),
                    read_only: true,
                    category: chimera_tools::ToolCategory::LocalExecution,
                },
            ],
            pending: Vec::new(),
        };
        let mut dialog = Dialog::choices("Permissions", "", Permissions::rows(&view));
        dialog.paste("glob");
        assert!(matches!(
            dialog.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Outcome::Action(action, _) if action == "permissions:set-disable:glob"
        ));
    }
}
