//! One direct acceptance case per assigned PR review finding.
use super::*;
use ratatui::backend::TestBackend;

fn workspace() -> Tui {
    Tui::new(
        "Session A objective".into(),
        "test-model".into(),
        "local".into(),
    )
}

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

fn card_text(message: &Message, styles: &Styles) -> String {
    message
        .to_lines(styles, 120)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// WHAT: pending instructions prevent opening or restoring another session.
/// WHY: a queued instruction must never run against a different conversation's context.
#[test]
fn reviewfix_02_queued_work_keeps_its_original_session_context() {
    let mut tui = workspace();
    tui.log.push(Message::User {
        text: "SESSION_A_CONTEXT".into(),
    });
    tui.enqueue("Continue session A".into(), true);
    let queue = tui.queued.clone();
    let session_id = tui.status.session_id.clone();
    for action in ["sessions:open:B", "sessions:restore:B"] {
        assert!(
            !tui.allow_session_action(action),
            "FAIL: {action} allowed a queued request to change sessions"
        );
    }
    let output = visible(&tui);
    assert!(tui.queued == queue && tui.objective == "Session A objective"
        && tui.status.session_id == session_id && output.contains("SESSION_A_CONTEXT")
        && output.contains("queued 1"),
        "PASS only when the visible original conversation and its pending instruction remain intact after all session-switch attempts");
    let event = tui
        .pop_queued_event()
        .expect("pending request should dispatch");
    assert!(
        tui.queued.is_empty() && tui.queued_in_flight,
        "fixture must exercise the last-item dispatch gap"
    );
    for action in ["sessions:open:B", "sessions:restore:B", "sessions:create"] {
        assert!(
            !tui.allow_session_action(action),
            "in-flight request must still prevent context switch: {action}"
        );
    }
    let TuiEvent::Queued(text, plan, origin) = event else {
        panic!("expected queued request");
    };
    assert_eq!(
        origin, session_id,
        "dispatch must retain the original conversation identity"
    );
    tui.status.session_id = "UNEXPECTED_DIFFERENT_SESSION".into();
    assert!(tui.accept_queued(text, plan, origin).is_none() && tui.queue_paused && tui.queued == queue, "PASS only when an unexpected context mismatch preserves and pauses the instruction instead of executing it");
}

/// WHAT: PLAN refuses file restoration and external program starts, and launch read-only stays sticky.
/// WHY: the developer's selected read-only boundary must cover actions outside the agent tools too.
#[test]
fn reviewfix_03_plan_and_launch_read_only_guard_mutating_actions() {
    let mut tui = workspace();
    assert!(tui.select_mode("/plan"));
    assert!(tui.effective_read_only());
    for action in [
        "rewind:confirm:snapshot",
        "external:check:server",
        "external:attach:server",
    ] {
        assert!(
            !tui.allow_mutating_action(action),
            "FAIL: PLAN allowed {action}"
        );
    }
    assert!(
        visible(&tui).contains("PLAN"),
        "FAIL: refused actions lost the visible PLAN mode"
    );
    tui.read_only = true;
    assert!(!tui.select_mode("/run"));
    assert!(tui.effective_read_only() && visible(&tui).contains("read-only"),
        "PASS only when file/external changes are refused in PLAN and /run visibly refuses to override a read-only launch");
}

/// WHAT: clearing a busy screen preserves the active card until its result arrives.
/// WHY: clearing the display must not swallow a running tool's eventual output.
#[test]
fn reviewfix_06_clear_during_a_tool_preserves_its_result_card() {
    let mut tui = workspace();
    // Give Clear working storage: refusal must come from the running-task
    // guard, rather than accidentally passing because archiving failed.
    let root = std::env::temp_dir().join(format!("chimera-review-06-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    tui.attach_scrollback(&root);
    assert!(
        tui.log.archive.is_some(),
        "FAIL: the Clear workflow has no usable scrollback storage"
    );
    tui.apply_agent_update(AgentUpdate::ToolCallStart {
        name: "read_file".into(),
        arguments: "{}".into(),
    });
    tui.clear_view();
    assert!(
        tui.log.messages.iter().any(
            |m| matches!(m, Message::ToolCall { name, result: None, .. } if name == "read_file")
        ),
        "FAIL: clearing while busy removed the unfinished tool card"
    );
    tui.apply_agent_update(AgentUpdate::ToolCallResult {
        name: "read_file".into(),
        result: "TOOL_RESULT_RETAINED".into(),
        ok: true,
        duration_ms: 5,
    });
    let card = tui
        .log
        .messages
        .iter_mut()
        .find(|m| matches!(m, Message::ToolCall { name, .. } if name == "read_file"))
        .unwrap();
    if let Message::ToolCall { expanded, .. } = card {
        *expanded = true;
    }
    // Dismiss the Clear refusal to inspect the retained conversation beneath it.
    tui.dialog = None;
    let output = visible(&tui);
    assert!(output.contains("TOOL_RESULT_RETAINED") && output.contains('✓'),
        "PASS only when the real completion update still produces the visible successful card and its output after Clear");
    std::fs::remove_dir_all(root).unwrap();
}

/// WHAT: a restored tool result lacking saved status is visibly completed with status unknown.
/// WHY: earlier output must not masquerade as a running tool or invent success/failure.
#[test]
fn reviewfix_07_restored_tool_output_has_an_honest_completed_status() {
    let mut tui = workspace();
    let saved = [
        chimera_llm::Message::assistant_with_tools(
            None,
            vec![chimera_llm::ToolCall {
                id: "saved-call".into(),
                kind: "function".into(),
                function: chimera_llm::FunctionCall {
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
            }],
        ),
        chimera_llm::Message::tool_result("saved-call", "SAVED_TOOL_OUTPUT"),
    ];
    tui.restore_messages(&saved);
    let card = tui
        .log
        .messages
        .iter_mut()
        .find(|m| matches!(m, Message::ToolCall { .. }))
        .unwrap();
    assert!(
        matches!(
            card,
            Message::ToolCall {
                result: Some(_),
                ok: None,
                ..
            }
        ),
        "FAIL: restoring invented a known success/failure or discarded the saved output"
    );
    if let Message::ToolCall { expanded, .. } = card {
        *expanded = true;
    }
    let rendered_card = card_text(card, &tui.styles);
    let output = visible(&tui);
    assert!(output.contains("completed") && output.contains("status unknown") && output.contains("SAVED_TOOL_OUTPUT")
        && !rendered_card.contains('⟳') && !rendered_card.contains('✓') && !rendered_card.contains('✗'),
        "PASS only when the restored card shows its output and completed/status unknown without a running, success or failure glyph");
}

/// WHAT: queued slash-prefixed objectives stay literal objectives with their saved mode.
/// WHY: a file path or requested text such as /quit must not become an application command later.
#[test]
fn reviewfix_09_queued_slash_objectives_remain_literal_requests() {
    let mut tui = workspace();
    tui.enqueue("/quit".into(), true);
    tui.enqueue("/src/main.rs needs investigation".into(), false);
    tui.plan_mode = false;
    for (expected, expected_mode) in [("/quit", true), ("/src/main.rs needs investigation", false)]
    {
        let (objective, saved_mode) = tui.queued.pop_front().unwrap();
        let (command, argument) = command_parts(&objective, Some(saved_mode));
        assert!(
            command.is_empty()
                && argument == expected
                && saved_mode == expected_mode
                && !tui.should_quit,
            "FAIL: queued objective {expected} became a command or lost its saved execution mode"
        );
        tui.input.set_buffer(argument);
        assert!(
            visible(&tui).contains(expected),
            "FAIL: the exact queued objective was not visible in the composer"
        );
    }
    assert_eq!(command_parts("/quit", None), ("/quit", ""),
        "PASS only when queued slash text remains literal while an ordinary explicit slash command remains a command");
}
