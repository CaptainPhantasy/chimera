# Ratatui layout

Read: 2026-10-11 (America/Indianapolis). Version: Ratatui 0.29.0.
Evidence: matching cached crate source. Sources: [Layout](https://docs.rs/ratatui/0.29.0/ratatui/layout/struct.Layout.html), [Constraint](https://docs.rs/ratatui/0.29.0/ratatui/layout/enum.Constraint.html), [Frame](https://docs.rs/ratatui/0.29.0/ratatui/struct.Frame.html).

Start each frame from `frame.area()`, then split it with `Layout::vertical` or `Layout::horizontal`. Use `Length` for fixed rows, `Min` for a region that can grow, and `Percentage` or `Ratio` for proportional regions. `Fill` distributes remaining space. Constraints compete when space is short; a requested size is not a guarantee that every screen can fit it.

`split` returns areas for nested layout. Derive child areas from their parent's actual rectangle, including `Block::inner` when borders or padding are present. In 0.29, `Frame::size()` is deprecated; use `area()`. Its result stays consistent throughout the drawing callback.

CHIMERA rule: handle zero-width and zero-height regions without placing an out-of-bounds cursor. Reduce decorative space before hiding essential input or status. Cover narrow, short, and resized terminals in saved-screen tests.

Cached files: `src/layout/layout.rs`, `src/layout/constraint.rs`, `src/terminal/frame.rs`.
