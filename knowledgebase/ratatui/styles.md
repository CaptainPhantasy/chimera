# Ratatui styles

Read: 2026-10-11 (America/Indianapolis). Version: Ratatui 0.29.0.
Evidence: matching cached crate source.
Sources: [Style](https://docs.rs/ratatui/0.29.0/ratatui/style/struct.Style.html), [Color](https://docs.rs/ratatui/0.29.0/ratatui/style/enum.Color.html).

Use `Style::default().fg(color).bg(color).add_modifier(modifier)` or the equivalent `Stylize` helpers. Styles can layer at widget, block, line, and span level. Keep the intended order clear so a text style does not accidentally erase a selected or focused state.

`Color::Reset` asks for the terminal's default color. It is not an alpha transparency value and does not guarantee the same appearance across terminals.

CHIMERA rule: reuse `theme.rs` and its existing dark and light palettes. Check text contrast, selected rows, border visibility, and the active input cursor in both palettes. Avoid scattering color literals through handlers.

Cached files: `src/style.rs`, `src/style/color.rs`.
