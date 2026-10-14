# Crossterm mouse capture

Read: 2026-10-11 (America/Indianapolis). Version: Crossterm 0.28.1.
Evidence: matching cached crate source.
Sources: [EnableMouseCapture](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.EnableMouseCapture.html), [DisableMouseCapture](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.DisableMouseCapture.html), [MouseEvent](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.MouseEvent.html).

Mouse capture is off by default. Enabling it asks the terminal to deliver mouse events to the application. Add capture only with a defined interaction and an implemented handler. Pair enable and disable operations in terminal cleanup.

CHIMERA's inspected chat handles keyboard and paste input, not mouse gestures. Keep native terminal selection behavior unless a requested feature calls for capture. Do not enable it merely because a general template does. Real terminal testing is needed to establish wheel, drag, or click behavior.

Cached file: `src/event.rs`.
