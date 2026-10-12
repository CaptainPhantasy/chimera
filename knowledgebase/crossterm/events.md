# Crossterm input events

Read: 2026-10-11 (America/Indianapolis). Version: Crossterm 0.28.1; CHIMERA uses Tokio 1.50.0.
Evidence: version-specific documentation and matching cached crate source.
Sources: [Event module](https://docs.rs/crossterm/0.28.1/crossterm/event/index.html), [poll](https://docs.rs/crossterm/0.28.1/crossterm/event/fn.poll.html), [EventStream](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.EventStream.html).

`read()` waits until input is available. With one reader, a successful `poll(timeout)` followed by `read()` avoids that indefinite wait. Keep synchronous polling and reading on the same thread. Do not combine them with `EventStream` or create competing input readers.

`EventStream` is an alternative asynchronous input source. It requires the `event-stream` feature, which is not enabled by default. CHIMERA's current workspace enables `use-dev-tty` and already uses bounded polling; that does not justify adding a feature or dependency.

CHIMERA rule: preserve paste events as text, including newlines. Process only a bounded amount of work before redrawing so sustained updates cannot starve input. Retain existing channels for agent updates and submissions. Review each ignored event deliberately; a catch-all branch can be appropriate for unused external events.

Cached files: `src/event.rs`, `src/event/stream.rs`, `Cargo.toml`.
