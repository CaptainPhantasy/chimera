//! Direct acceptance of background transcript reads, including keyboard cancellation.
use super::*;
use ratatui::backend::TestBackend;

fn visible(tui: &Tui) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
    terminal.draw(|frame| tui.render(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// WHAT: archive search/page reads leave keyboard cancellation responsive and publish their results when finished.
/// WHY: finding earlier diagnostics must not freeze the development workspace.
#[tokio::test(flavor = "current_thread")]
async fn reviewfix_08_archive_reads_leave_the_workspace_responsive() {
    let root = std::env::temp_dir().join(format!("chimera-review-08-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let mut tui = Tui::new(String::new(), "local-model".into(), "local".into());
    tui.attach_scrollback(&root);
    for i in 0..250 {
        tui.log.push(Message::User {
            text: format!("ARCHIVED_DIAGNOSTIC_{i:03}"),
        });
    }
    tui.input.set_buffer("UNSENT_WORK");
    tui.search("ARCHIVED_DIAGNOSTIC_000").unwrap();
    assert!(
        tui.history_job.is_some() && visible(&tui).contains("Conversation history"),
        "FAIL: search did not return a background busy view to the user"
    );
    tui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        tui.dialog.is_none() && tui.history_job.is_none(),
        "FAIL: Escape could not cancel the pending read without waiting for it"
    );
    tui.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
    assert_eq!(
        tui.input.buffer, "UNSENT_WORK!",
        "the composer must remain usable after cancel"
    );
    tui.search("ARCHIVED_DIAGNOSTIC_000").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while tui.history_job.is_some() {
            tui.poll_history().await;
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background search must finish");
    assert!(
        visible(&tui).contains("ARCHIVED_DIAGNOSTIC_000"),
        "FAIL: completed search did not show the earlier diagnostic"
    );
    tui.dialog = None;
    tui.load_older().unwrap();
    assert!(
        tui.history_job.is_some(),
        "FAIL: page reading ran inline instead of starting a background read"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while tui.history_job.is_some() {
            tui.poll_history().await;
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("background page read must finish");
    assert!(
        visible(&tui).contains("Earlier transcript") && tui.input.buffer == "UNSENT_WORK!",
        "PASS only when history results are visible and the developer's draft remains intact"
    );
    std::fs::remove_dir_all(root).unwrap();
}
