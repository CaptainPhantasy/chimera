# CHIMERA CLI Reference

The CLI binary (`chimera`) is provided by the `chimera-cli` crate. It exposes a wide set of commands for lifecycle, objective execution, session management, and subsystem control.

Usage (high level)
- `chimera --help` — show top-level usage
- `chimera run "objective"` — execute an objective in Run mode
- `chimera plan "objective"` — plan-only (no write side effects)
- `chimera swarm "objective"` — parallel execution mode
- `chimera pack <name>` — launch predefined pack

Examples
- Run a simple objective:
  cargo run -p chimera-cli -- run "audit tests"

- Launch pack:
  cargo run -p chimera-cli -- pack ship-hotfix

Top-level categories (see crates/chimera-cli/src/main.rs for full enums)
- Bootstrap & lifecycle: init, up, down, doctor, status, daemon
- Objective execution: run, plan, swarm, pack
- Session control: session, checkpoint, rollback, replay
- Creature control: creature spawn/list/inspect/stop/promote/leash
- Tool control: tool list/inspect/healthcheck/test/disable
- World control: world spawn/list, browser, phone
- Policy & approvals: approve/deny/policy/autonomy/leash
- Trace & eval: trace, eval
- Memory & skills: memory, skill
- Durable jobs: schedule, watch

Dispatch
- Objective commands are wired to the Harness via `run_objective` which returns a RunReport printed to stdout.
