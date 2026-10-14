# Terminal setup and cleanup

Read: 2026-10-11 (America/Indianapolis). Versions: Crossterm 0.28.1; Ratatui 0.29.0.
Evidence: matching cached crate source and CHIMERA's inspected runner.
Sources: [Crossterm terminal](https://docs.rs/crossterm/0.28.1/crossterm/terminal/index.html), [Ratatui try_init](https://docs.rs/ratatui/0.29.0/ratatui/fn.try_init.html), [Ratatui try_restore](https://docs.rs/ratatui/0.29.0/ratatui/fn.try_restore.html), [bracketed paste](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.EnableBracketedPaste.html).

Raw mode gives the application direct key input; normal terminal handling of Ctrl+C and line buffering changes. The alternate screen keeps the application's full-screen drawing separate from the shell's main screen. Enable only the modes actually used, then undo them on ordinary exit and error.

CHIMERA already uses raw mode, alternate screen, bracketed paste, and a cleanup guard. Preserve paste disable and cursor restoration. Setup failures must also attempt cleanup. A guard runs during ordinary stack unwinding, but the panic report normally runs before that cleanup; a hook can restore the terminal before forwarding to the previous panic hook.

Ratatui 0.29's initialization helpers install such a hook. CHIMERA's custom initialization must arrange equivalent behavior for the active terminal owner without dropping its extra cleanup. Avoid replacing a previous hook silently or repeatedly wrapping it. Do not promise cleanup after forced termination or a second panic inside cleanup.

## Owner-thread rule discovered during implementation

Observed in [CHIMERA's runner](../../crates/chimera-tui/src/runner.rs), 2026-10-11: the hook is installed once, preserves the previous reporter, and checks thread-local terminal activity. Only a panic on the active terminal's owning thread invokes restoration before the previous reporter. The cleanup guard cannot be sent to another thread, keeping ordinary exit and unwind cleanup with the terminal owner.

A worker-thread panic must not switch off raw mode or leave the alternate screen underneath a still-running drawing loop. The worker keeps the existing panic reporter; the owner handles the returned join failure through its normal cleanup path. Once the owner closes the terminal, later unrelated panics bypass terminal cleanup. Partial setup still needs cleanup, and restoration must not itself panic.

This records the inspected ownership design. Actual terminal verification was pending when this note was updated; source inspection alone does not establish successful normal, error, or panic restoration.

Cached files: Crossterm `src/terminal.rs`, `src/event.rs`; Ratatui `src/terminal/init.rs`. Hook and guard guidance is the application design derived from those sources.
