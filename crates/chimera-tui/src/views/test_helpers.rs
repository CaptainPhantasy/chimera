use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::app::{App, ApprovalPrompt, CreatureRow, TaskDisplayStatus, TaskRow};

/// Create a test terminal with a given size.
pub fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    Terminal::new(backend).unwrap()
}

/// Create an App populated with sample data for rendering tests.
pub fn sample_app() -> App {
    let mut app = App::new("test-session".into(), "Extract auth module and verify".into());
    app.mode = "SWARM".into();
    app.autonomy = "MEDIUM".into();
    app.repo = "~/work/app".into();
    app.worlds = "worktrees+vm".into();

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

    app.tasks = vec![
        TaskRow { description: "map auth files".into(), status: TaskDisplayStatus::Complete },
        TaskRow { description: "patch extraction".into(), status: TaskDisplayStatus::InProgress },
        TaskRow { description: "run smoke test".into(), status: TaskDisplayStatus::Pending },
        TaskRow { description: "broken step".into(), status: TaskDisplayStatus::Failed },
    ];

    app.push_feed("RAVEN-1", "found token refresh path");
    app.push_feed("MANTIS-1", "requests approval: write 6 files");

    app
}

/// Create an App with an active approval prompt.
pub fn app_with_approval() -> App {
    let mut app = sample_app();
    app.approval = Some(ApprovalPrompt {
        request_id: "ap-0044".into(),
        actor: "MANTIS-1".into(),
        class: "YELLOW".into(),
        action: "Modify 6 files".into(),
        reason: "Extract auth kernel".into(),
        touched_resources: vec!["src/auth.rs".into(), "src/session.rs".into()],
        verification_steps: vec!["cargo test auth_refresh".into()],
        rollback_info: "checkpoint #18 ready".into(),
    });
    app
}
