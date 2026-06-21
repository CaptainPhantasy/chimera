# Changelog

All notable changes to this project will be documented in this file.

## [0.3.0] - 2026-06-18
### Added — Competitive terminal UI
- chimera-tui: **complete rewrite** to a streaming chat-first interface competitive with top agentic coding tools.
  - **Theme system** (`theme.rs`): curated dark/light palettes with semantic roles (brand, foreground hierarchy, background hierarchy, semantic, syntax highlight, borders). 24-bit RGB color design tokens. Reusable `Styles` API.
  - **Streaming chat view** (`chat.rs`, `message.rs`): conversation stream with user/assistant/system/divider messages, word-wrapped rendering, streaming cursor indicator, tool-call cards with expand/collapse, status icons (✓/✗/⟳).
  - **Tool-call inspector**: inline cards showing tool name, arguments, result, duration, and success/failure status. Tab to expand.
  - **Status bar** (`status.rs`): live display of provider, model, mode, token usage (compact k/M format), iteration count, tool call count, estimated cost, session ID, running indicator.
  - **Input bar** (`input.rs`): multi-line text editor with cursor movement (char/word/home/end), history navigation, backspace/delete, and slash-command autocomplete suggestions.
  - **Help overlay** (`help.rs`): centered modal with keyboard shortcuts and slash-command reference. Toggle with `?`.
  - **Main event loop** (`runner.rs`): full terminal lifecycle (raw mode, alt screen, frame budget polling), event-driven architecture via channels, agent-loop integration via `AgentUpdate` stream.
  - **CLI integration**: new `chimera tui` subcommand and `chimera run --interactive` / `-i` flag.

### Keyboard shortcuts
- `Enter` submit · `Backspace/Delete` edit · `←→ Home End` cursor · `Alt+←→` word jump
- `↑↓` scroll log · `PageUp/Down` fast scroll · `?` help · `Ctrl+L` theme toggle · `Ctrl+Space` clear · `Ctrl+C/q` quit

### Slash commands
- `/help` `/run` `/model` `/tools` `/sessions` `/clear` `/theme` `/quit`

### Verified
- `cargo test --workspace` — **199 tests pass, 0 failures** (+31 new TUI tests).
- `cargo clippy` — 0 errors, 0 new warnings.

## [0.2.0] - 2026-06-17
### Added — Real harness implementation
- chimera-llm: LLM provider abstraction with OpenAI-compatible streaming (works with OpenAI, Together, Groq, LM Studio, Ollama, Portkey). `LlmProvider` trait, `CompletionRequest`/`Response`, `StreamDelta`, `ToolCall`/`ToolDefinition`, `OpenAiCompatibleProvider`.
- chimera-config: configuration resolution from env vars + `~/.chimera/config.json`. Provider name, base URL, API key, model, temperature, max_tokens, max_iterations.
- chimera-core::agent: real LLM-driven agent loop (`run_agent_loop`). ReAct-style: prompt → tool calls → execute → feed back → repeat until final answer. Iteration cap, streaming callbacks, token accounting.
- chimera-tools: `DefaultToolRouter` with 7 concrete tools: `read_file`, `write_file`, `edit_file`, `bash`, `glob`, `grep`, `git_status`. Respects .gitignore via the `ignore` crate.
- chimera-session: `FileSessionStore` — JSON file persistence under `.chimera/sessions/`. Full create/append/snapshot/fork/load/list.
- chimera-shell: `ProcessShellManager` — real subprocess execution via `sh -c`, worktree management via `git worktree`, transcript recording, read-only enforcement, timeout handling.
- chimera-trace: `FileTraceSink` — JSONL trace persistence under `.chimera/traces/`. In-memory mirror + disk fallback.
- chimera-cli: `run`/`plan`/`swarm` commands wired to the real agent loop with live streaming. `session list/fork`, `tool list/inspect/healthcheck`, `creature list`, `trace export` wired to concrete backends. Config gate for API key safety.

### Verified
- `cargo build --workspace` — clean, 0 errors.
- `cargo test --workspace` — **168 tests pass, 0 failures** (up from 134 baseline; +34 new tests).
- End-to-end agent loop verified against a live LLM (Ollama OpenAI-compatible endpoint): streaming output, final-answer termination, token accounting.

## [0.1.0] - Unreleased (initial public release)
