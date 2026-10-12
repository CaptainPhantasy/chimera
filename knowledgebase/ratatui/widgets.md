# Ratatui widgets

Read: 2026-10-11 (America/Indianapolis). Version: Ratatui 0.29.0.
Evidence: matching cached crate source, checked against the archive whose checksum is recorded in CHIMERA's Cargo.lock. These are original summaries, not copied manuals.

## Block

A `Block` supplies borders, titles, padding, and a base style. Compute the content area with `block.inner(area)` before laying out its children. Test tiny areas and long titles; borders consume cells.

Source: [Block](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Block.html); cached `src/widgets/block.rs`.

## List and ListState

`List` displays items without table columns. Keep `ListState` in application state when selection or scrolling must survive redraws. `render_stateful_widget` can update the visible offset to keep selection on screen.

Sources: [List](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.List.html), [ListState](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.ListState.html); cached `src/widgets/list/list.rs` and `src/widgets/list/state.rs`.

## Table and TableState

Build rows and explicit column constraints with `Table::new(rows, widths)`. Keep `TableState` outside the drawing function. Selection can cover rows, columns, or cells; rendering adjusts the visible row offset. Account for column spacing and borders when sizing the table.

Sources: [Table](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Table.html), [TableState](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.TableState.html); cached `src/widgets/table/table.rs` and `src/widgets/table/table_state.rs`.

## Paragraph

`Paragraph` displays styled text with optional wrapping and scrolling. Scrolling takes vertical then horizontal offsets. CHIMERA should compute cursor and scroll positions from displayed cell widths, not assume a Unicode character occupies one cell. Wrapping policy must match the scroll calculations.

Source: [Paragraph](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Paragraph.html); cached `src/widgets/paragraph.rs`. Cell-width guidance is CHIMERA's application rule.

## Tabs

`Tabs` draws labels and the selected index. It does not process key input or switch application views. Keep that transition in the event handler and draw from the resulting state.

Source: [Tabs](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Tabs.html); cached `src/widgets/tabs.rs`.

## Gauge

`Gauge` draws a progress bar. `percent` accepts 0 through 100; `ratio` accepts 0.0 through 1.0. Invalid inputs panic, including a non-finite ratio. Validate incoming measurements. Unknown progress should be labelled unknown rather than invented.

Source: [Gauge](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Gauge.html); cached `src/widgets/gauge.rs`.

## Chart

`Chart` draws datasets against axes. A `Dataset` borrows `(f64, f64)` points and supplies its marker, graph type, and style. Set meaningful axis bounds and labels. Keep owned data in application state long enough for the frame to draw.

Source: [Chart](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Chart.html); cached `src/widgets/chart.rs`.
