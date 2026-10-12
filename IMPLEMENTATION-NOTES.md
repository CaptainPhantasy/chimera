# CHIMERA terminal refresh — implementation notes

Completed work dated 2026-10-11, America/Indianapolis. The user's instruction was **Both**: save the supplied guide as a reusable skill and apply it to the current terminal screen.

## Delivered

The reusable skill is installed at `/Users/floyd/.codex/skills/chimera-ratatui-expert/SKILL.md`, with its `agents/openai.yaml`. The bundled skill validator reports `Skill is valid!`. The guide keeps documentation → knowledgebase → patterns → blueprint → implementation → verification and includes the version and terminal-ownership corrections found during this work.

[Knowledgebase](knowledgebase/INDEX.md) contains 11 original topic/pattern notes, source evidence, dates, exact versions, URLs, source hashes, and note hashes. [BLUEPRINT.md](BLUEPRINT.md) was written and independently reviewed before source changes. Review corrections were recorded before implementation. The matching cached published crate source was checked against its archive and Cargo.lock where a documentation page could not be retrieved.

CHIMERA remains Rust with Ratatui 0.29.0 and Crossterm 0.28.1. Tokio remains 1.50.0 and anyhow 1.0.102. This task made no changes to Cargo manifests or Cargo.lock. The existing application version remains 0.1.0.

## Changes

The actual saved chat runner now opens with useful guidance, keeps input visible in small windows, fits status groups to available width, and shows keyboard hints. `Ready` means the application is idle; it does not claim that a model is connected. Unavailable usage and cost remain labelled unavailable. Both existing themes are preserved.

Conversation scrolling and rendering share the same rectangle, including its width. All allocated conversation rows are used. Help wraps on narrow screens, scrolls with arrow and page keys, and retains its close hint. Input handles multiline cursor width, command-history hints, and a one-row terminal. `q` remains chat text; Escape closes help; Ctrl+C stops work and leaves.

Terminal setup installs an owner-aware panic hook once and preserves the previous reporter. The real runner and test probe share `TerminalGuard`, which cannot move between threads. Cleanup restores terminal modes, alternate screen, paste handling, and cursor before reporting an owner-thread panic. Worker and later non-terminal panics do not tear down the active owner's terminal. Update processing is bounded to 128 events per frame; completion is checked after the producer finishes and its queued updates are applied.

Behavior changes are in `crates/chimera-tui/src/chat.rs`, `status.rs`, `input.rs`, `help.rs`, and `runner.rs`. Formatting was limited to the terminal package. Other touched files in that package were checked against the saved starting copy after normalizing both with the same formatter; those changes are formatting only. Unrelated workspace changes were preserved. No commit, push, deployment, or normal-shell installation was performed.

## Verified build and checks

Final executable: `/Volumes/Storage/CHIMERA/.floyd/ratatui-refresh/verified/chimera`.

SHA-256: `61dc4ceedfbbe7421549fed7c0156fbef5466c04fff179c66db886a63a834b2f`.

The saved executable is a byte-identical copy of the tested `.floyd/ratatui-refresh/target/debug/chimera`. Source hashes are recorded in [tested-source-hashes.json](.floyd/ratatui-refresh/tested-source-hashes.json). The starting terminal files remain under `.floyd/ratatui-refresh/before/`.

| Check | Result | Evidence |
| --- | --- | --- |
| `cargo fmt --package chimera-tui --check` | Pass | [format log](.floyd/ratatui-refresh/format.log) |
| `cargo clippy --locked --offline -p chimera-tui --all-targets --no-deps -- -D warnings` | Pass | [Clippy log](.floyd/ratatui-refresh/clippy.log) |
| `cargo test --locked --offline -p chimera-tui` | 70 passed; 0 failed | [test log](.floyd/ratatui-refresh/unit-tests.log) |
| `cargo build --locked --offline -p chimera-cli` | Pass | [build report](.floyd/ratatui-refresh/build-report.json) |
| Actual executable, `scripts/check-user-flows.py --tui-only` | 105 checks passed | [flow report](.floyd/ratatui-refresh/user-flows-report.json) |
| Actual terminal modes, error/panic ordering, ownership, narrow help/paste/resize/exit | 48 checks passed | [cleanup report](.floyd/ratatui-refresh/lifecycle-i7871tsw/check-report.json) |

The tests used isolated folders and a controlled local model service. Approval tests performed their allowed writes only inside the disposable test folder. No hosted model request was made. The cleanup probe used the actual final `chimera_tui` library; it did not replace the production cleanup code with a mock.

The live read-only preview at `http://127.0.0.1:18762/` launches the identified saved executable. [Preview identity](.floyd/ratatui-refresh/preview-launch.json) records its process, hash, and intentionally unconfigured model. Launching without a request makes no model call. Use the saved executable with `tui` in a normal terminal for keyboard input; the browser preview intentionally does not accept input.

Visual inspection covered the live preview at 1440×900 and 390×844 pixels, and actual saved terminal output for light/dark chat, full and narrow help, long input, tool details, long answers, and errors. Screens are in [.floyd/ratatui-refresh/screens/](.floyd/ratatui-refresh/screens/), plus [desktop](.floyd/ratatui-refresh/desktop-1440x900.jpg) and [narrow](.floyd/ratatui-refresh/mobile-390x844.jpg) captures. The terminal tests also exercised 40×12, 66×15, 80×24, 100×30, and 160×45 cells. Automated cell checks include tiny input and cursor bounds.

Command suggestions were also captured from the final executable at 40×12, 66×15, and 80×24 cells in both themes. Visual review checked the narrow dark screen, compact dark menu, and full light menu: the smallest screen deliberately hides suggestions to preserve the message box; the other two show three and five complete choices. All three sessions exited cleanly and restored their terminal modes. [Capture report](.floyd/ratatui-refresh/suggestions-r_n8cqcf/capture-report.json).

## Corrections and limits

The baseline strict code check found four terminal-package warnings; all were fixed without suppression. The first source review also caught the one-row input title, compact welcome wrapping, and completion-check ordering; each was corrected before final tests.

Early cleanup-helper attempts revealed macOS terminal-descriptor behavior and differential terminal output. The helper now checks the master terminal descriptor after exit and forces a redraw before checking complete help text. Those failed helper reports are retained beside the final passing report. They were not treated as evidence of application failure.

The saved-output viewer originally used old Unicode width rules and rendered emoji incorrectly. It now uses the official `@xterm/addon-unicode11` 0.8.0 package, downloaded into `.floyd/ratatui-refresh/viewer-deps/` and checked against its registry SHA-512 integrity. This is a preview-tool dependency only; the Rust application dependencies did not change. Screens were checked again after that correction. The live preview also clears its inherited `NO_COLOR` setting so the application theme is visible. A browser connection error recorded during the deliberate preview restart was a temporary wrapper disconnect.

The supplied guide was adapted to this existing workspace: no `cargo init`, no dependency upgrade, no unsupported `ratatui::run`, no global `q` quit binding, and no unused mouse capture. The existing explicit event loop was retained. Context7 results were checked against exact-version documentation because some returned examples belonged to newer versions.

There is no unresolved documentation gap for the interfaces changed here. A live model account, physical keyboard behavior in every native terminal, other operating systems, forced-kill recovery, full-workspace diagnostics, and overall beta readiness were not tested by this task. The controlled test service and browser wrapper do not prove those results. The knowledgebase's source-level lifecycle note predates the final runtime tests; the passing runtime evidence is linked above.

The [final evidence check](.floyd/ratatui-refresh/final-verification.json) ties the passing reports to the unchanged final source, executable, library, documents, and saved screenshots. The blueprint retains its original review state as history and identifies the review that was completed before implementation.
