# CHIMERA

CHIMERA is a modular orchestration platform for automated, evidence-driven code operations. It models autonomous "creatures" (specialized agents) and execution "worlds" (isolated environments) to safely plan, execute, verify, and capture repeatable workflows.

Key principles
- Safety-first: explicit approval classes and execution scopes prevent unauthorized side effects.
- Evidence-driven: every run can capture diffs, transcripts, screenshots, and trace spans.
- Composable: small crates provide trait-based subsystems (shells, browsers, mobile, MCP, memory, eval).

Quickstart
1. Build: `cargo build --workspace`
2. Configure: set `CHIMERA_API_KEY` (or `OPENAI_API_KEY`), optionally `CHIMERA_BASE_URL` and `CHIMERA_MODEL`. For local models, point `CHIMERA_BASE_URL` at an OpenAI-compatible server (Ollama at `http://127.0.0.1:11434/v1`, LM Studio at `http://127.0.0.1:1234/v1`).
3. Run: `cargo run -p chimera-cli -- run "audit tests"` or launch the interactive TUI: `cargo run -p chimera-cli -- tui`

What's implemented
- LLM provider abstraction (chimera-llm): OpenAI-compatible streaming, tool calling, multi-provider (OpenAI, Together, Groq, LM Studio, Ollama).
- Agent loop (chimera-core::agent): ReAct-style tool-calling loop with streaming output, iteration caps, token accounting.
- Concrete tools (chimera-tools::DefaultToolRouter): read_file, write_file, edit_file, bash, glob, grep, git_status — respects .gitignore.
- Session persistence (chimera-session::FileSessionStore): JSON-backed session event log with snapshot/fork.
- Shell execution (chimera-shell::ProcessShellManager): real subprocess spawning, worktree management, transcripts.
- Trace persistence (chimera-trace::FileTraceSink): JSONL trace files under `.chimera/traces/`.
- Config (chimera-config): env var + `~/.chimera/config.json` resolution.

Repository layout
- crates/chimera-llm: LLM provider trait + OpenAI-compatible streaming client.
- crates/chimera-config: configuration loading and provider construction.
- crates/chimera-core: Harness orchestration, agent loop, domain types, packs/skills/scheduler.
- crates/chimera-tools: ToolRouter trait + DefaultToolRouter with 7 builtin tools.
- crates/chimera-session: SessionStore trait + FileSessionStore.
- crates/chimera-shell: ShellManager trait + ProcessShellManager.
- crates/chimera-trace: TraceSink trait + FileTraceSink.
- crates/: modular Rust crates (creatures, browser, mobile, memory, eval, mcp, comms, sandbox, tui, api).
- target/: build artifacts and coverage data
