# CHIMERA terminal patterns

Verified against: Ratatui 0.29.0, Crossterm 0.28.1, Tokio 1.50.0, anyhow 1.0.102. Read: 2026-10-11 (America/Indianapolis). Sources and hashes: [knowledgebase index](../INDEX.md). These are project rules derived from the linked notes and the inspected workspace, not claims that all current behavior has passed runtime tests.

## Preserve the existing app

Read the manifests, lockfile, current source, and local changes before editing. CHIMERA is already a Rust workspace. Do not run `cargo init`, replace the workspace, or upgrade dependencies to follow a newer tutorial. The live chat enters through `chimera-cli` and `runner.rs`; the separate dashboard `App` has different controls. Re-read these paths because another chat may change them.

Consult documentation for locked versions. Ratatui 0.29 has no `ratatui::run`; use the existing explicit terminal loop. A newer release is research context, not permission for an upgrade. No mandatory new dependency or mouse feature is needed.

## State, updates, and drawing

Keep owned conversation, input, and status state in the existing `Tui`. Keep event handling separate from rendering. Model and tool work sends updates through channels; the outer loop applies them, draws a complete frame, and yields. Bound batches so an endless update stream cannot prevent key handling or redraws. Do not reorganize modules just to match a template.

Implementation finding, 2026-10-11: the inspected runner processes at most 128 agent updates per pass before returning to keys and drawing. Check that the producing task is finished **before** checking whether its update queue is empty. Checking emptiness first permits a final update to arrive between the two checks, so completion can be shown before the final text or usage. Join and report completion only after the finished producer's queued updates have been applied. This ordering is independent of the chosen batch limit.

Use `frame.area()` and nested layouts, then draw widgets into those areas. Store list or table selection state across frames when those widgets are used. For text, preserve the relation between wrapped rows, scroll offsets, Unicode cell width, and the cursor. Zero-sized regions must remain safe.

References: [rendering](../ratatui/rendering.md), [layout](../ratatui/layout.md), [widgets](../ratatui/widgets.md).

## One input reader

Keep one input mechanism. A short successful `poll` followed by `read` is valid; an indefinite `read` on the drawing path is not. Do not mix that pair with `EventStream`. If an actual requirement later needs `EventStream`, inspect its feature and dependency changes before adopting it.

Preserve `q` as chat text, Escape as help close, and the current Ctrl+C exit request. Preserve `/quit`, `/exit`, `/cancel`, paste newlines, and existing help. Ignore release events and decide repeat behavior per action. Match the events CHIMERA owns exhaustively; allow documented ignored external events instead of forcing meaningless handlers.

References: [events](../crossterm/events.md), [keys](../crossterm/keys.md), [mouse](../crossterm/mouse.md).

## Terminal ownership and errors

Pair setup with cleanup for raw mode, alternate screen, bracketed paste, and visible cursor. A guard covers ordinary return and unwinding. On a panic in the thread that owns the active terminal, restore it before calling the previous reporter. Keep cleanup safe when setup is partial and prevent it from panicking. Mouse capture stays off unless an implemented feature needs it.

Implementation finding, 2026-10-11: install the process-wide hook once and preserve the previous reporter. Track terminal activity on its owning thread, and keep the cleanup guard on that thread. The inspected runner uses thread-local activity and a guard that cannot be sent between threads. A worker panic must not tear down a screen that the owner is still drawing; failures returned when joining the worker follow the owner's ordinary error-cleanup path. Panics after the terminal closes must not emit terminal-restoration commands. These source-level rules still require actual terminal cleanup checks.

Use the existing anyhow application errors with useful context. Avoid unchecked `unwrap` in production paths. Where an invariant is relied on, prove it locally and test its boundary; do not weaken tests merely to satisfy a blanket search rule. Cleanup cannot guarantee recovery from forced termination.

References: [current runner](../../crates/chimera-tui/src/runner.rs), [lifecycle](../crossterm/lifecycle.md), [anyhow 1.0.102](https://docs.rs/anyhow/1.0.102/anyhow/), [Tokio tasks 1.50.0](https://docs.rs/tokio/1.50.0/tokio/task/).

## Style and verification

Use the existing centralized theme. Check both dark and light modes. `Color::Reset` means the terminal's default color, not portable transparency.

Test state changes separately from drawing. Use `TestBackend` for saved cell contents and cursor bounds at multiple terminal sizes. Then exercise changed controls, resize, ordinary exit, error cleanup, and panic cleanup in an actual terminal. A cell-buffer test does not verify shell restoration or a model connection.

Derive formatting, build, lint, and test commands from the project. Do not format unrelated work or change tests to hide failures. Record skipped or failing checks and identify the final source version tested. Concurrent edits require checking whether the tested files changed afterward.

References: [backends](../ratatui/backends.md), [Ratatui styles](../ratatui/styles.md), [Crossterm styles](../crossterm/styles.md).
