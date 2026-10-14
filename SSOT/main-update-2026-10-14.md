# Main reconciliation and user documentation — 14 October 2026

The October implementation from `a70d0e1` and the older remote main at `c4cd6d3` are combined in source commit `8d86ead1ddb7b4e816fad07a5b1ef0bed599250b`. Both histories are preserved. No branch was force-pushed or deleted by this update.

The large earlier working-tree change was preserved in `85f825d` (87 files) and was already on the remote October branch. It had not reached main. Both original working copies had no unsaved source changes at the start of this task. The morning-recovery branch and the previous remote main have identical file trees.

The terminal interface upgrade is retained. All 16 interface files in its saved verification inventory match `85f825d`. Twelve remain identical in the later October version; four received later fixes. This integration retains the current rendering and controls. The runner's only change groups the existing event and cancellation values for an internal call.

An audit of ignored local Rust files found saved-history copies, generated files, test probes, pre-upgrade backups, and older release-check snapshots. The distinct older source copies were compared with the current implementation; they lack subsequent fixes. The backups and diagnostics remain on disk. They were not substituted for current code or published as private project data. This audit covers this repository and its recorded second working copy, not unrelated locations.

The merge retains main's required-check configuration and independent code cleanups. Required checks exposed partial-request reads in test servers and structural warnings. The test servers now read complete bounded requests. Related function arguments are grouped, a helper was moved before its tests, and a watchdog propagates the same error directly. Existing assertions and required checks remain in place.

## Current verification

- `cargo test --workspace --locked`: 360 tests passed, zero failures.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo build --release --locked -p chimera-cli`: passed.
- `scripts/check-user-flows.py`: 296 checks passed against the final executable; controlled local service, not a hosted model.
- `scripts/check-setup.py`: 79 checks passed against that same executable.
- All 101 terminal examples in the commands sheet accepted by the final executable with `--help`; their outside actions were not executed.
- Four user documents rendered at 1440 by 900 and 390 by 844; image loading, page overflow, text, and local links checked.

Final tested executable SHA-256: `723614f4a308e136425c50019a495ab979c67a0d464be208919b1306014f981e`.

The [quick start](../docs/QUICKSTART.md), [commands sheet](../docs/COMMANDS.md), [manual](../docs/USER_MANUAL.md), and [README](../README.md) document the current app. The [screenshot record](../docs/images/capture-record.json) identifies actual captures from the final executable. No model responses were fabricated for the documentation.

Hosted services, real Android devices, and external notification recipients were not exercised in this task. Local detailed logs, source comparisons, and rendered pages are preserved in `.floyd/docs-20261014/`.
