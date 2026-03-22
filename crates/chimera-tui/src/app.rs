use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// TUI app state
// ---------------------------------------------------------------------------

/// Which view the TUI is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    CreatureInspector,
    ShellGrid,
    ApprovalPrompt,
}

/// A creature row for the dashboard display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatureRow {
    pub id: String,
    pub role: String,
    pub state: String,
    pub world: String,
    pub confidence: f64,
    pub tokens_used: u64,
    pub last_action: String,
}

/// A task row for the task graph display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub description: String,
    pub status: TaskDisplayStatus,
}

/// Display status for a task in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskDisplayStatus {
    Pending,
    InProgress,
    Complete,
    Failed,
}

/// A line in the live feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedLine {
    pub timestamp: DateTime<Utc>,
    pub creature: String,
    pub message: String,
}

/// An approval request displayed in the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPrompt {
    pub request_id: String,
    pub actor: String,
    pub class: String,
    pub action: String,
    pub reason: String,
    pub touched_resources: Vec<String>,
    pub verification_steps: Vec<String>,
    pub rollback_info: String,
}

/// Top-level TUI application state.
pub struct App {
    /// Current active view.
    pub view: ActiveView,
    /// Session label.
    pub session_label: String,
    /// Session ID.
    pub session_id: Uuid,
    /// Execution mode label.
    pub mode: String,
    /// Current autonomy level label.
    pub autonomy: String,
    /// Repo path.
    pub repo: String,
    /// Active worlds description.
    pub worlds: String,
    /// Objective text.
    pub objective: String,
    /// Creature rows.
    pub creatures: Vec<CreatureRow>,
    /// Task graph.
    pub tasks: Vec<TaskRow>,
    /// Live feed (most recent last).
    pub feed: Vec<FeedLine>,
    /// Current approval prompt (if any).
    pub approval: Option<ApprovalPrompt>,
    /// Selected creature index (for inspector).
    pub selected_creature: usize,
    /// Whether the app should quit.
    pub should_quit: bool,
}

impl App {
    /// Create a new app with default/empty state.
    pub fn new(session_label: String, objective: String) -> Self {
        Self {
            view: ActiveView::Dashboard,
            session_label,
            session_id: Uuid::new_v4(),
            mode: "RUN".into(),
            autonomy: "MEDIUM".into(),
            repo: String::new(),
            worlds: String::new(),
            objective,
            creatures: Vec::new(),
            tasks: Vec::new(),
            feed: Vec::new(),
            approval: None,
            selected_creature: 0,
            should_quit: false,
        }
    }

    /// Push a line to the live feed.
    pub fn push_feed(&mut self, creature: &str, message: &str) {
        self.feed.push(FeedLine {
            timestamp: Utc::now(),
            creature: creature.into(),
            message: message.into(),
        });
        // Keep feed bounded.
        if self.feed.len() > 200 {
            self.feed.drain(0..self.feed.len() - 200);
        }
    }

    /// Handle a key event. Returns true if the event was consumed.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                true
            }
            KeyCode::Char('a') => {
                // Approve — placeholder
                true
            }
            KeyCode::Char('d') => {
                // Deny — placeholder
                true
            }
            KeyCode::Char('f') => {
                // Fork — placeholder
                true
            }
            KeyCode::Char('c') => {
                // Checkpoint — placeholder
                true
            }
            KeyCode::Char('r') => {
                // Replay — placeholder
                true
            }
            KeyCode::Char('l') => {
                // Leash — placeholder
                true
            }
            KeyCode::Tab => {
                // Cycle views
                self.view = match self.view {
                    ActiveView::Dashboard => ActiveView::CreatureInspector,
                    ActiveView::CreatureInspector => ActiveView::ShellGrid,
                    ActiveView::ShellGrid => ActiveView::Dashboard,
                    ActiveView::ApprovalPrompt => ActiveView::Dashboard,
                };
                true
            }
            KeyCode::Up => {
                if self.selected_creature > 0 {
                    self.selected_creature -= 1;
                }
                true
            }
            KeyCode::Down => {
                if self.selected_creature + 1 < self.creatures.len() {
                    self.selected_creature += 1;
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app_with_creatures() -> App {
        let mut app = App::new("test-session".into(), "test objective".into());
        app.creatures = vec![
            CreatureRow {
                id: "RAVEN-1".into(),
                role: "recon".into(),
                state: "running".into(),
                world: "wt-ro-1".into(),
                confidence: 0.84,
                tokens_used: 28000,
                last_action: "map auth".into(),
            },
            CreatureRow {
                id: "MANTIS-1".into(),
                role: "patcher".into(),
                state: "waiting".into(),
                world: "wt-rw-1".into(),
                confidence: 0.77,
                tokens_used: 21000,
                last_action: "needs approve".into(),
            },
        ];
        app
    }

    #[test]
    fn new_app_defaults() {
        let app = App::new("s1".into(), "obj".into());
        assert_eq!(app.view, ActiveView::Dashboard);
        assert_eq!(app.session_label, "s1");
        assert_eq!(app.objective, "obj");
        assert!(!app.should_quit);
        assert_eq!(app.selected_creature, 0);
        assert!(app.creatures.is_empty());
        assert!(app.feed.is_empty());
        assert!(app.approval.is_none());
    }

    #[test]
    fn push_feed_appends() {
        let mut app = App::new("s".into(), "o".into());
        app.push_feed("RAVEN-1", "found auth path");
        app.push_feed("MANTIS-1", "patching");
        assert_eq!(app.feed.len(), 2);
        assert_eq!(app.feed[0].creature, "RAVEN-1");
        assert_eq!(app.feed[1].message, "patching");
    }

    #[test]
    fn push_feed_bounds_at_200() {
        let mut app = App::new("s".into(), "o".into());
        for i in 0..210 {
            app.push_feed("C", &format!("msg {}", i));
        }
        assert_eq!(app.feed.len(), 200);
    }

    #[test]
    fn key_q_quits() {
        let mut app = App::new("s".into(), "o".into());
        assert!(app.handle_key(key(KeyCode::Char('q'))));
        assert!(app.should_quit);
    }

    #[test]
    fn key_a_d_f_c_r_l_consumed() {
        let mut app = App::new("s".into(), "o".into());
        assert!(app.handle_key(key(KeyCode::Char('a'))));
        assert!(app.handle_key(key(KeyCode::Char('d'))));
        assert!(app.handle_key(key(KeyCode::Char('f'))));
        assert!(app.handle_key(key(KeyCode::Char('c'))));
        assert!(app.handle_key(key(KeyCode::Char('r'))));
        assert!(app.handle_key(key(KeyCode::Char('l'))));
    }

    #[test]
    fn tab_cycles_views() {
        let mut app = App::new("s".into(), "o".into());
        assert_eq!(app.view, ActiveView::Dashboard);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.view, ActiveView::CreatureInspector);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.view, ActiveView::ShellGrid);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.view, ActiveView::Dashboard);
    }

    #[test]
    fn tab_from_approval_goes_to_dashboard() {
        let mut app = App::new("s".into(), "o".into());
        app.view = ActiveView::ApprovalPrompt;
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.view, ActiveView::Dashboard);
    }

    #[test]
    fn arrow_keys_navigate_creatures() {
        let mut app = app_with_creatures();
        assert_eq!(app.selected_creature, 0);

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_creature, 1);

        // Can't go past last
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_creature, 1);

        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected_creature, 0);

        // Can't go below 0
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected_creature, 0);
    }

    #[test]
    fn unknown_key_not_consumed() {
        let mut app = App::new("s".into(), "o".into());
        assert!(!app.handle_key(key(KeyCode::Char('z'))));
    }

    #[test]
    fn task_display_status_variants() {
        let statuses = [
            TaskDisplayStatus::Pending, TaskDisplayStatus::InProgress,
            TaskDisplayStatus::Complete, TaskDisplayStatus::Failed,
        ];
        for s in &statuses {
            let json = serde_json::to_string(s).unwrap();
            let _: TaskDisplayStatus = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn creature_row_serialization() {
        let row = CreatureRow {
            id: "RAVEN-1".into(),
            role: "recon".into(),
            state: "running".into(),
            world: "wt-ro-1".into(),
            confidence: 0.84,
            tokens_used: 28000,
            last_action: "map".into(),
        };
        let json = serde_json::to_string(&row).unwrap();
        let _: CreatureRow = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn approval_prompt_serialization() {
        let prompt = ApprovalPrompt {
            request_id: "ap-1".into(),
            actor: "MANTIS-1".into(),
            class: "yellow".into(),
            action: "modify files".into(),
            reason: "extract auth".into(),
            touched_resources: vec!["src/auth.rs".into()],
            verification_steps: vec!["cargo test".into()],
            rollback_info: "checkpoint ready".into(),
        };
        let json = serde_json::to_string(&prompt).unwrap();
        let _: ApprovalPrompt = serde_json::from_str(&json).unwrap();
    }
}
