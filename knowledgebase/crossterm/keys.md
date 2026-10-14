# Crossterm keys and modifiers

Read: 2026-10-11 (America/Indianapolis). Version: Crossterm 0.28.1.
Evidence: matching cached crate source and CHIMERA's inspected chat handler.
Sources: [KeyEvent](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.KeyEvent.html), [KeyEventKind](https://docs.rs/crossterm/0.28.1/crossterm/event/enum.KeyEventKind.html), [KeyModifiers](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.KeyModifiers.html).

Match key code, modifiers, and event kind. Key kinds include press, repeat, and release. Modifiers are flags; `contains` checks that required flags are present while `intersects` accepts any flag in a group. Decide intentionally whether extra modifiers should be accepted. Enhanced key reporting depends on terminal support and enabled features.

CHIMERA controls to preserve unless the user changes them:

- `q` and `?` are ordinary chat characters while composing.
- Escape closes help; it does not globally quit the chat.
- Ctrl+C requests exit; `/quit` and `/exit` also leave. `/cancel` stops the running request.
- F1 opens help. Alt/Shift+Enter inserts a newline where the terminal reports those modifiers; Enter submits.

These are observed application rules, not Crossterm defaults. Existing source can change in another chat; re-read `crates/chimera-tui/src/runner.rs` and `help.rs` before editing key behavior. Test repeats and releases so holding a key does not duplicate one-time actions.

Cached file: `src/event.rs`.
