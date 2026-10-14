# CHIMERA terminal refresh blueprint

Written before implementation, 2026-10-11. Authorization: Floyd answered **Both** to improving the current terminal screen and saving the supplied guide as a reusable skill.

## Outcome and boundaries

Improve the existing Rust chat screen, preserve existing chat and approval behavior, and install a reusable guide. Keep the warm charcoal/ember/teal and light themes. Preserve unrelated changes in this shared working tree. No dependency upgrade, new workspace, model purchase, remote publication, or invented connection/usage/cost claim.

The real entry is `crates/chimera-cli/src/main.rs` → `chimera_tui::runner::run_saved_interactive` → `runner::run_screen`. The separate dashboard in `app.rs` is not the live chat. Current dependencies are Ratatui 0.29.0, Crossterm 0.28.1, Tokio 1.50.0, and anyhow 1.0.102. [Source notes and hashes](knowledgebase/INDEX.md) and [patterns](knowledgebase/patterns/patterns.md) were saved and read before this blueprint. `ratatui::run` is not available in this version.

## Observed issues

The opening conversation area is blank. Fixed-height suggestions compete with input in small windows. The status line is longer than the current browser terminal. Conversation drawing subtracts two border rows even though it draws no border. Help is clipped without scrolling. The custom terminal setup lacks a panic hook that restores the shell before reporting a panic. The existing diagnostics report one unused cursor-width helper, a nested character condition, and two test-initialization warnings.

## Screen design

```
◆ CHIMERA  version                         objective when it fits
────────────────────────────────────────────────────────────────
Opening guidance, or the existing scrollable conversation

Ready / Working · model · mode · other real details when they fit
┌ Message ──────────────────────────────────────────────────────┐
│ editable request                                              │
└───────────────────────────────────────────────────────────────┘
Command suggestions when there is space
Enter send · Alt+Enter new line · F1 help · Ctrl+C quit
```

Allocate space from the actual frame size. Keep editable input visible; reduce suggestions and decorative spacing before sacrificing input. Use all real conversation rows and the same geometry for scrolling limits. Use Unicode cell widths for text fitting. Fit whole status groups instead of clipping arbitrary fragments; on narrow screens prioritize Ready/Working, model, and mode. Ready means idle, not verified provider connectivity. Missing usage and cost remain unavailable. The opening guidance should explain entering a request and finding commands/help without implying a configured model.

Help keeps the heading `CHIMERA controls`, wraps descriptions to available width, and supports Up/Down and PageUp/PageDown. A fixed footer explains scrolling and closing. Clamp its scroll offset when resized. Tiny terminal regions must be safe even where there is insufficient room for full text.

## State and controls

Keep the existing `Tui`, channels, message log, and bounded event polling. One component reads terminal events. Ignore key-release events. Preserve Enter submit, Alt/Shift+Enter newline, bracketed paste, history, cursor movement, PageUp/PageDown conversation scrolling, Tab tool expansion, F1 help, theme switching, and slash commands. `q` remains ordinary input outside help; Escape closes help. Ctrl+C stops work and leaves. No mouse capture without a implemented mouse feature.

Add only help scroll state. Keep drawing separate from event handling. Remove unchecked production unwraps in affected input and task-completion paths with explicit Option/error handling, preserving valid cursor boundaries. Use the existing cursor-width helper in rendering. Keep normal/error/unwind cleanup and install a one-time panic hook that restores terminal state before calling the existing reporter, without changing the host application's reporter.

## File ownership and implementation order

1. Documentation is complete before this blueprint; review the blueprint before code.
2. `chat.rs` and `status.rs`: adaptive layout, opening guidance, status groups, footer, and meaningful rendering tests.
3. `input.rs`, `help.rs`, and `runner.rs`: compact input, help navigation, consistent scrolling geometry, terminal lifecycle, and relevant tests. Preserve other modules unless a demonstrated failure requires a scoped correction.
4. Verify the built CLI, show its actual screen, then write `IMPLEMENTATION-NOTES.md` with evidence and limits.

The starting terminal crate is copied under `.floyd/ratatui-refresh/before/`, with source hashes in `before-hashes.json`. Inspect changes against that copy because this checkout contains earlier user work. Do not reset or broadly format the workspace.

## Acceptance and evidence

- Reusable skill is saved at `/Users/floyd/.codex/skills/chimera-ratatui-expert/SKILL.md`, includes discovery → sources → patterns → blueprint → implementation → verification, and passes the bundled skill validator.
- Knowledgebase index links to dated notes, source URLs, versions, patterns, and verified hashes. Adaptations from the supplied guide are explicit.
- Opening screen, conversation, status, input, suggestions, and help remain readable at normal and narrow terminal sizes; input/cursor bounds remain safe at tiny sizes. Both themes retain their intended background.
- Targeted `cargo fmt --package chimera-tui --check`, `cargo clippy --locked --offline -p chimera-tui --all-targets --no-deps -- -D warnings`, `cargo test --locked --offline -p chimera-tui`, and `cargo build --locked --offline -p chimera-cli` pass without diagnostic suppression. Use `.floyd/ratatui-refresh/target` for isolated build output.
- Exercise the final binary using the existing `scripts/check-user-flows.py --tui-only` against its controlled local service. This checks typing, streaming, tools/approval, errors, scrolling, paste, resizing, and normal shell restoration; it does not prove a live model connection.
- Add focused error/panic terminal restoration evidence and narrow help/input checks where the existing suite does not cover them. Observe exit state, terminal modes, and actual output. Do not infer restoration from a screenshot alone.
- Render and inspect final saved screens, including dark/light, help, suggestions, and changed conversation states. Check the live browser wrapper at 390×844 and 1440×900 pixels. It displays actual terminal output; it is not a replacement web application.
- Record the final binary hash and source hashes. Recheck affected files after verification. Report scoped checks as scoped checks, and any actual documentation or runtime gaps plainly.

## Review record

Original state when this blueprint was first saved: pending independent review before source changes. The completed review below was recorded before implementation.

Independent source review completed before implementation. Accepted corrections:

- Redefine the cursor-width helper to measure only the current line, then use it; its former whole-buffer sum is wrong for multiline input.
- Share the actual message rectangle width and height between drawing and scroll limits. Remove the stale width hint and forced minimum of 20 columns from those calculations; zero-width regions remain safe.
- The panic hook uses thread-local active ownership, set by terminal setup and cleared by restoration. Only a panic on the active terminal owner restores before reporting. Other threads and later non-terminal panics use the previous reporter without terminal cleanup. The ordinary returned-error guard still restores when the UI exits because a worker task failed.
- Replace the private generic cleanup closure with a small public `TerminalGuard` used by the actual runner and isolated lifecycle probe. Create it before setup so partial setup and early errors are covered. Do not claim forced-kill recovery.
- Bound each channel-update batch before returning to drawing and keyboard handling. Preserve update order and process remaining updates on later frames.

## 2026-10-14: Ten everyday workflows

User objective: modernize the ten compared features, commit each completed feature separately, then stop. Work on current main; preserve previous interface work, terminal controls, stored data, and exact-action approvals. No dependency upgrades or paid model requests.

Use the existing Ratatui 0.29.0 / Crossterm 0.28.1 screen. Add reusable searchable lists and editable forms as overlays. The main loop continues drawing while connection checks run in a worker. Keys update overlay state, confirmed choices produce explicit actions, storage services persist them, and the screen reports the real result. Escape cancels and returns the untouched chat draft. Use masked key fields, bounded responses, private atomic storage, and useful retry states.

Screen: existing chat underneath a centered panel with title, search or fields, scrollable choices, error/status text and key hints. At narrow sizes use the full available width; keep selection visible and wrap details. Check dark/light themes and 40x15, 80x24, 120x32 terminals.

Ordered milestones and exit criteria (one tested commit each):
1. Provider chooser: presets plus custom addresses, masked key input, connection test, saved profiles, cancellation and useful errors.
2. Model chooser: fetch available models, search, switch saved provider, persist selected default and honor explicit process overrides visibly.
3. Conversation chooser: titles, dates, search, preview and resume in current screen, preserving unsent text.
4. Rewind: automatic per-turn snapshots of agent-edited files, selectable history, exact restore preview and confirmation, preservation of displaced contents.
5. Permissions: unified in-chat controls for mode and tool access using existing policy enforcement.
6. Memory: conversational explicit remembering plus browse/edit/archive in chat, preserving history and not guessing consent to remember.
7. Reusable tasks: discover saved procedures, create from current work, invoke by slash name in current conversation.
8. External tools: saved guided connection setup, status, supported sign-in, attachment to interactive agent with existing approvals.
9. Background work: in-chat task list, details, create/start/pause/resume/stop using actual scheduler.
10. Long prompts: configured external editor round trip with correct terminal restoration, draft preservation on error and no automatic submission.

Verification: targeted Rust tests and real terminal flows per milestone, strict Clippy, formatting of affected modules, then full workspace tests and final rendered screen inspection. Build output stays under .floyd/build-cache/beta-target because the machine-wide target points outside this workspace. Preserve failed logs. Source: OpenCode provider/model/session/editor documentation and Claude Code checkpoint/permissions/memory/skills/tool/task documentation inspected in this conversation; existing version-matched knowledgebase supplies widget and lifecycle rules.
