# Crossterm terminal styles

Read: 2026-10-11 (America/Indianapolis). Version: Crossterm 0.28.1.
Evidence: matching cached crate source.
Source: [style module](https://docs.rs/crossterm/0.28.1/crossterm/style/index.html).

Crossterm exposes foreground and background colors, text attributes, and commands to reset them. Terminal support varies, so a declared color or attribute is not a guarantee of identical display on every terminal.

For CHIMERA's active screen, apply Ratatui styles through the existing theme and let its backend emit terminal commands. Direct Crossterm styling is appropriate for controlled setup, cleanup, or output outside that drawing loop. Interleaving direct writes with Ratatui frames can make the displayed screen disagree with its remembered cell buffer.

Cached file: `src/style.rs`. The separation of drawing responsibilities is CHIMERA's application rule.
