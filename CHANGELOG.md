# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - Unreleased (initial public release)
### Added
- Initial multi-crate workspace with core harness and subsystem trait stubs.
- chimera-cli: user-facing CLI with objective execution commands and many stubs (crates/chimera-cli/src/main.rs).
- chimera-core: Harness, RunReport, ObjectiveRequest, Approval model, Evidence model (crates/chimera-core/src/lib.rs).
- In-memory and stub backends for safe local development across subsystems (shell, world, memory, eval, comms, mcp).

### Notes
- Current release focuses on architecture and API surface. Concrete production backends are left to implementers.
- Test coverage reported in repository: recent commits indicate coverage instrumentation (see commit 096c3a6, a030539, 75f4755).
