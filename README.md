# CHIMERA

CHIMERA is a modular orchestration platform for automated, evidence-driven code operations. It models autonomous "creatures" (specialized agents) and execution "worlds" (isolated environments) to safely plan, execute, verify, and capture repeatable workflows.

Key principles
- Safety-first: explicit approval classes and execution scopes prevent unauthorized side effects.
- Evidence-driven: every run can capture diffs, transcripts, screenshots, and trace spans.
- Composable: small crates provide trait-based subsystems (shells, browsers, mobile, MCP, memory, eval).

Quickstart
1. Build: `cargo build --workspace`
2. Run CLI: `cargo run -p chimera-cli -- run "audit tests"`

Repository layout
- crates/: modular Rust crates (core, cli, tui, mobile, browser, memory, eval, etc.)
- target/: build artifacts and coverage data

Documentation
- ARCHITECTURE.md — detailed architecture and component map
- docs/API.md — public crate API reference and key traits
- docs/CLI.md — CLI command reference and examples
- CONTRIBUTING.md — how to contribute, run tests, and style rules
- SECURITY.md — security reporting and policies

License
MIT — see LICENSE
