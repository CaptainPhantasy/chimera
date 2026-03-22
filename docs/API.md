# CHIMERA API Reference (public surface)

This document highlights the primary traits and data types of the platform that implementers should rely on when wiring production backends.

Key traits and locations
- Harness (crates/chimera-core/src/lib.rs): run_objective accepts an ObjectiveRequest and returns a RunReport.
- SessionStore (crates/chimera-session/src/lib.rs): create, append_event, snapshot, fork, load, list.
- ShellManager (crates/chimera-shell/src/lib.rs): spawn_shell, exec, transcript, create_worktree, destroy_worktree.
- ToolRouter (crates/chimera-tools/src/lib.rs): call, list, healthcheck.
- WorldManager (crates/chimera-tui/src/app.rs - world trait mirrored): allocate, inspect, pause, resume, destroy, list.
- BrowserController (crates/chimera-browser/src/lib.rs): open, act, screenshot, state, close, list_sessions.
- MobileController (crates/chimera-mobile/src/lib.rs): boot, perform, state, shutdown, list_devices.
- TraceSink (crates/chimera-trace/src/lib.rs): emit, query, query_creature.
- MemoryStore (crates/chimera-memory/src/lib.rs): remember, retrieve, forget, compact.
- Evaluator (crates/chimera-eval/src/lib.rs): grade, compare.
- McpClient (crates/chimera-mcp/src/lib.rs): list_tools, invoke, list_resources, read_resource, ping.
- CommsBridge (crates/chimera-comms/src/lib.rs): notify, request_approval, check_approval, receipt.

Data types
- ObjectiveRequest: objective text, mode, repo_root, policy_profile, evidence_level.
- RunReport: session_id, status, tasks, approvals, evidence, checkpoints.
- EvidenceRef: kind, uri, label, created_at.

Guidance
- Implementations should be async and return anyhow::Result for ergonomic error handling.
- Keep trace events rich and structured to support effective debugging and auditing.
