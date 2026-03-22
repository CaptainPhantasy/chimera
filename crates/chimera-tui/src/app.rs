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
