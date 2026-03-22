# CHIMERA Architecture

Overview
CHIMERA orchestrates autonomous "creatures" through a harness (crates/chimera-core) that holds trait-object handles to subsystem backends. The harness follows a phased execution model: intake → decompose → assign creatures → assign worlds → execute → verify → approve → commit → memory update.

Components
- chimera-core: orchestration harness, domain types, run lifecycle (src/lib.rs)
- chimera-cli: user-facing CLI that dispatches to the harness (crates/chimera-cli/src/main.rs)
- chimera-tui: terminal UI for live sessions and approval flows
- chimera-shell, chimera-browser, chimera-mobile: world/controllers for execution environments
- chimera-tools: ToolRouter abstraction for invoking external tools
- chimera-comms: CommsBridge for notifications and approval prompts
- chimera-memory: layered memory store (scratch/session/episodic/semantic/pattern)
- chimera-eval: evaluation and diffing subsystems
- chimera-mcp: MCP client for external tool integrations

Data flows
1. Operator issues an ObjectiveRequest (chimera-core::ObjectiveRequest).
2. Harness decomposes into TaskReports and assigns CreatureSpec instances.
3. Creatures invoke tools via ToolRouter, spawn shells/worktrees via ShellManager or Worlds via WorldManager.
4. Trace events are emitted to TraceSink to support replay and auditing.
5. Approvals are requested/checked through CommsBridge when needed.
6. Successful runs produce CapturedSkill and RunReport artifacts that are stored or surfaced.

Security & Safety
- ExecutionScope (packs/pack.rs) defines allowed/denied paths, network and commit permissions.
- Approval classes (core::ApprovalClass) gate low/medium/high-risk actions.
- Stubs and mocks enable safe local runs without side effects for development and testing.

Deployment
- Local developer: run CLI; harness uses stub backends by default.
- Production: implement concrete backends for SessionStore, ToolRouter, WorldManager, TraceSink, MemoryStore, McpClient, etc., and run the daemon (chimera-cli daemon start).

Observability
- Tracing uses `tracing` spans and channeled TraceSink implementations.
- TUI provides live dashboard and feed for session activity (crates/chimera-tui).

For implementers
- Key trait locations: see crates/chimera-core/src (Harness & domain types), and per-subsystem crates' lib.rs files for trait method definitions.
