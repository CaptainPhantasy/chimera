//! One direct developer-visible acceptance test for each root-owned finding.
use super::*;
use ratatui::backend::TestBackend;

fn screen() -> Tui {
    Tui::new(String::new(), "test-model".into(), "local".into())
}
fn text(tui: &Tui) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| tui.render(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("chimera-ux-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn tool(name: &str, result: &str) -> Message {
    Message::ToolCall {
        id: name.into(),
        name: name.into(),
        arguments: "{}".into(),
        result: Some(result.into()),
        ok: Some(true),
        duration_ms: Some(1),
        expanded: false,
    }
}
fn choose(tui: &mut Tui, action: &str) {
    tui.transcript_action(action, &[]).unwrap();
}

/// WHAT: PLAN stays visibly selected for the next ordinary follow-up.
/// WHY: continuing a planning conversation must not silently allow edits.
#[test]
fn ux_01_planning_remains_selected_for_follow_up_requests() {
    let mut tui = screen();
    tui.select_mode("/plan");
    tui.input.set_buffer("Continue investigating");
    tui.handle_key(key(KeyCode::Enter));
    assert!(
        matches!(tui.rx.try_recv(),Ok(TuiEvent::Submit(request)) if request=="Continue investigating")
            && tui.plan_mode
            && text(&tui).contains("PLAN"),
        "PASS only when the ordinary follow-up retains the visible PLAN execution selection"
    );
}

/// WHAT: Ctrl+C requests cancellation while keeping the workspace open.
/// WHY: stopping a slow task must not force a developer out of the conversation.
#[test]
fn ux_02_interrupt_keeps_the_developer_in_the_workspace() {
    let mut tui = screen();
    tui.status.running = true;
    tui.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(
        !tui.should_quit
            && matches!(tui.rx.try_recv(),Ok(TuiEvent::Submit(command)) if command=="/cancel"),
        "PASS only when Ctrl+C requests task cancellation without quitting the screen"
    );
}

/// WHAT: follow-ups are retained in order, with their selected execution mode.
/// WHY: a developer can submit the next instruction while the current one runs.
#[test]
fn ux_05_follow_ups_are_visible_and_run_in_submission_order() {
    let mut tui = screen();
    tui.status.running = true;
    tui.enqueue("First follow-up".into(), true);
    tui.enqueue("Second follow-up".into(), false);
    let visible = text(&tui).contains("queued 2");
    let first = tui.queued.pop_front();
    tui.status.running = false;
    tui.transcript_action("queue:edit:0", &[]).unwrap();
    assert!(
        visible
            && first == Some(("First follow-up".into(), true))
            && tui.input.buffer == "Second follow-up"
            && !tui.plan_mode
            && tui.queued.is_empty(),
        "PASS only when both submitted requests remain visible and preserve FIFO order and mode"
    );
}

/// WHAT: the background-job panel opens during a foreground request.
/// WHY: inspecting background work must not require abandoning foreground work.
#[tokio::test]
async fn ux_06_background_work_is_accessible_while_chat_runs() {
    let mut tui = screen();
    tui.status.running = true;
    let mut jobs = crate::jobs::Jobs::default();
    tui.open_jobs(&mut jobs, &root());
    assert!(
        tui.status.running && text(&tui).contains("Background work"),
        "PASS only when the jobs panel is visible and the foreground request remains running"
    );
    jobs.cancel();
}

/// WHAT: selecting an earlier tool reveals that tool's actual stored output.
/// WHY: engineers must inspect earlier failures rather than only the newest card.
#[test]
fn ux_07_earlier_tool_results_can_be_selected_and_read() {
    let mut tui = screen();
    tui.log.push(tool("earlier", "EARLIER_RESULT"));
    tui.log.push(tool("newest", "NEWEST_RESULT"));
    tui.tools_view();
    choose(&mut tui, "transcript:tool:0");
    assert!(
        text(&tui).contains("EARLIER_RESULT"),
        "PASS only when selecting the earlier card displays its real result"
    );
}

/// WHAT: the end of a patch beyond the 400-line compact preview is reachable.
/// WHY: the developer must be able to review the entire stored change.
#[test]
fn ux_08_full_patch_inspection_reaches_the_last_line() {
    let mut tui = screen();
    let mut patch = "diff --git a/file b/file\n".to_string();
    for i in 0..450 {
        patch.push_str(&format!("+changed line {i}\n"));
    }
    patch.push_str("+FINAL_PATCH_LINE");
    tui.log.push(tool("large patch", &patch));
    tui.tools_view();
    choose(&mut tui, "transcript:tool:0");
    let _ = text(&tui);
    tui.handle_key(key(KeyCode::End));
    assert!(
        text(&tui).contains("FINAL_PATCH_LINE"),
        "PASS only when End in full patch inspection makes the final stored patch line visible"
    );
}

/// WHAT: transcript search finds an earlier diagnostic by its literal text.
/// WHY: developers must find previous errors without manually scrolling.
#[test]
fn ux_09_search_displays_the_matching_diagnostic() {
    let mut tui = screen();
    tui.log.push(Message::Assistant {
        text: "Build failed: missing_symbol_42".into(),
        streaming: false,
    });
    tui.search("missing_symbol_42").unwrap();
    assert!(
        text(&tui).contains("Build failed: missing_symbol_42"),
        "PASS only when search displays the actual matching diagnostic"
    );
}

/// WHAT: output evicted from the live 100-message window is available in an older page.
/// WHY: tool-heavy development must not silently lose inspectable scrollback.
#[test]
fn ux_10_older_output_remains_readable_after_live_window_eviction() {
    let mut tui = screen();
    tui.attach_scrollback(&root());
    for i in 0..150 {
        tui.log.push(Message::User {
            text: format!("HISTORY_ENTRY_{i:03}"),
        });
    }
    tui.load_older().unwrap();
    assert!(
        text(&tui).contains("HISTORY_ENTRY_000"),
        "PASS only when the first evicted entry is visible after loading older output"
    );
}

/// WHAT: a new conversation has no previous model context and preserves the draft/history.
/// WHY: an empty-looking screen must not secretly carry an old task's instructions.
#[tokio::test]
async fn ux_11_new_conversation_is_fresh_without_losing_previous_work() {
    use chimera_session::{SessionEvent, SessionStore};
    let root = root();
    let old = SavedSession::create(root.clone(), "Previous task")
        .await
        .unwrap();
    let old_id = old.id;
    old.store
        .append_event(
            old.id,
            SessionEvent::Custom {
                key: "messages".into(),
                payload: serde_json::json!([{"role":"user","content":"Previous private task"}]),
            },
        )
        .await
        .unwrap();
    let mut tui = screen();
    tui.input.set_buffer("My unsent draft");
    let mut session = Some(old);
    new_conversation(&root, &mut tui, &mut session)
        .await
        .unwrap();
    let fresh = session.as_ref().unwrap();
    assert!(fresh.id!=old_id && fresh.messages.is_empty() && tui.input.text()=="My unsent draft" && !SavedSession::resume(root,old_id).await.unwrap().messages.is_empty() && text(&tui).contains("New conversation"), "PASS only when new context is empty while the old task and unsent draft remain recoverable");
}

/// WHAT: the header identifies the real project and checked-out branch.
/// WHY: developers with multiple worktrees need to know where edits will run.
#[tokio::test]
async fn ux_19_header_identifies_the_current_project_and_git_branch() {
    let root = root();
    let output = std::process::Command::new("git")
        .args(["init", "--quiet", "-b", "feature/visible"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let mut tui = screen();
    tui.status.project = root.display().to_string();
    tui.status.branch = git_branch(&root).await;
    assert!(
        text(&tui).contains("feature/visible")
            && text(&tui).contains(root.file_name().unwrap().to_str().unwrap()),
        "PASS only when the actual project name and branch are visible together"
    );
}

/// WHAT: context usage distinguishes a known model limit from an unknown limit.
/// WHY: developers need honest estimated headroom rather than cumulative billing tokens.
#[test]
fn ux_20_context_budget_shows_estimated_usage_and_the_known_limit() {
    let mut tui = screen();
    tui.status.context_tokens = 2500;
    tui.status.context_window = Some(10000);
    let known = text(&tui);
    tui.status.context_window = None;
    let unknown = text(&tui);
    assert!(known.contains("ctx ~2500/10000") && unknown.contains("ctx ~2500/?"), "PASS only when estimated used/known capacity is visible and an unknown capacity is explicitly marked unknown");
}
