# Ratatui terminal backends

Read: 2026-10-11 (America/Indianapolis). Version: Ratatui 0.29.0.
Evidence: matching cached crate source.
Sources: [Backend module](https://docs.rs/ratatui/0.29.0/ratatui/backend/index.html), [CrosstermBackend](https://docs.rs/ratatui/0.29.0/ratatui/backend/struct.CrosstermBackend.html), [TestBackend](https://docs.rs/ratatui/0.29.0/ratatui/backend/struct.TestBackend.html).

A backend connects Ratatui's cell buffer to a terminal. CHIMERA uses `Terminal<CrosstermBackend<Stdout>>`; preserve that existing choice. Other backends exist but are not needed for the current work.

`TestBackend` stores cells in memory and supports tests of text, colors, placement, and cursor bounds. It does not prove that a real terminal enters or leaves raw mode correctly, handles physical keys correctly, or restores the shell after an error or panic. Check those separately in a real terminal session.

Cached files: `src/backend.rs`, `src/backend/test.rs`.
