# CHIMERA Port Status

**Date:** 2026-06-21

**Status:** No port binding needed — CHIMERA is a CLI/TUI agentic coding harness.

## Port Audit

| Check | Result |
|---|---|
| Does this project bind a network port? | **No** — CLI tool with terminal UI |
| `chimera-api` crate | Stub only — no active server code |
| Forbidden ports used in code? | **No** — `5000` in `chat.rs:181` is `duration_ms: Some(5000)` (timing value, false positive) |
| Port claimed in registry? | **N/A** — no claim needed |

## Future Considerations

If `chimera-api` is implemented, a port in the 10000-65535 range must be claimed in
`/Volumes/SanDisk1Tb/SSOT/port-registry.json` before binding, and this file should
be updated with the claimed port.

## Local Receipt Location

Per governance: `.supercache/` is a read-only symlink to the global template store.
Local governance receipts live in `SSOT/` per FLOYD.md.
