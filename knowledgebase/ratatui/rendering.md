# Ratatui drawing loop

Read: 2026-10-11 (America/Indianapolis). Version: Ratatui 0.29.0.
Evidence: version-specific documentation and matching cached source.
Sources: [Terminal](https://docs.rs/ratatui/0.29.0/ratatui/struct.Terminal.html), [Frame](https://docs.rs/ratatui/0.29.0/ratatui/struct.Frame.html), [0.29 crate functions](https://docs.rs/ratatui/0.29.0/ratatui/).

`Terminal::draw` gives the callback a `Frame`. Draw the complete visible screen each time; Ratatui compares the finished buffer with the previous one and sends the changed cells to the terminal. The returned `CompletedFrame` is useful for inspection and is only valid until the next drawing call. `try_draw` supports callbacks that return an error.

Use `frame.area()` for the current draw. Fullscreen and inline terminals resize their internal buffers during drawing; fixed viewports need explicit handling. Use `frame.set_cursor_position` after computing a position within the input region.

CHIMERA rule: handlers change state; the outer loop draws. Keep long model or tool work out of the drawing callback and pass updates back through existing channels. Avoid printing directly over the active screen.

Version boundary: 0.29 exposes `init`, `try_init`, `restore`, and `try_restore`. It does **not** expose `ratatui::run`. Do not paste a newer example into this workspace without checking every used function against 0.29.

Cached files: `src/lib.rs`, `src/terminal/terminal.rs`, `src/terminal/frame.rs`, `src/terminal/init.rs`.
