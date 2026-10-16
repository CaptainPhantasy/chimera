# CHIMERA competitive gaps and modernization roadmap

Reviewed 16 October 2026 against CHIMERA 0.1.0 (`main` at `5b1672f`).

This is a planning document. It lists the feature updates that keep CHIMERA from being competitive with current terminal coding agents. Nothing listed here is implemented by this change.

## How this was assessed

- **CHIMERA facts** come from this repository's code and documentation. File references point to the evidence.
- **Competitor facts** describe Claude Code, OpenAI Codex CLI, Gemini CLI, OpenCode and Aider. They come from those projects' documentation as known to the reviewer, spot-checked against public sources on 14 October 2026. Competitor command names can change between releases, so verify a specific behavior before relying on it.
- **Classification** is a judgement, not a measurement:
  - **Determinism**: the harness itself enforces the behavior the same way every run, independent of model choices or operator memory.
  - **Feature**: adds convenience, reach or review quality without changing how predictable a run is.
  - **Caution**: adds capability but can make runs less predictable unless constrained and logged.

## Summary

| # | Gap | Class | Priority |
|---|---|---|---|
| 1 | Context compaction | Feature (caution) | P1 |
| 2 | Project instruction files (`AGENTS.md`) | Determinism | P0 |
| 3 | Native providers and account sign-in | Feature | P2 |
| 4 | Persistent model selection | Determinism | P1 |
| 5 | Reasoning-effort control | Determinism | P1 |
| 6 | Continue / new conversation in chat | Feature | P1 |
| 7 | Persistent allow/deny rules | Determinism | P0 |
| 8 | One unified permission mode | Determinism | P0 |
| 9 | Linux and Windows sandbox | Feature | P1 |
| 10 | Diff preview in approvals | Feature | P0 |
| 11 | `@file` mentions with fuzzy search | Determinism | P1 |
| 12 | Image and screenshot input | Feature | P3 |
| 13 | Web fetch and search tools | Feature (caution) | P2 |
| 14 | Automatic checkpoints and rewind | Determinism | P0 |
| 15 | MCP in chat and over HTTP | Feature | P1 |
| 16 | Lifecycle hooks | Determinism | P0 |
| 17 | Custom slash commands | Determinism | P1 |
| 18 | Helper agents from chat | Feature (caution) | P3 |
| 19 | Editor integration | Feature | P3 |
| 20 | Unattended / CI runs with declared permissions | Determinism | P0 |
| 21 | Single policy-as-code file | Determinism | P0 |
| 22 | Verification gates on completion | Determinism | P0 |
| 23 | Run manifest and reproducible replay | Determinism | P0 |
| 24 | Documentation claims match the build | Credibility | P0 |
| 25 | Public reproducibility and benchmark results | Credibility | P2 |
| 26–36 | Further gaps, not yet assessed in detail | — | — |

P0 makes CHIMERA usable day to day and establishes its distinctive position. P1 broadens usability. P2 and P3 are lower value or already well served by competitors.

## Part 1 — The 20 competitor gaps

### 1. Context compaction
- **CHIMERA:** Work stops when the conversation reaches 4 MiB or 4,096 messages. Nothing is summarized (`crates/chimera-core/src/agent.rs:20`, `:261`).
- **Competitors:** Claude Code has `/compact [focus]` and compacts automatically near the limit. Codex CLI has `/compact`. OpenCode compacts automatically.
- **Needed:** Compaction the harness controls. Record exactly what was summarized, keep the original turns on disk, and allow a policy to turn it off. A model-written summary varies between runs, so it must be logged.

### 2. Project instruction files
- **CHIMERA:** A fixed `DEFAULT_SYSTEM_PROMPT` (`crates/chimera-core/src/agent.rs:78`). Nothing loads per-project rules.
- **Competitors:** Codex reads `AGENTS.md` from the project root down to the working folder (32 KiB default cap). Claude Code loads `CLAUDE.md` at user, project and subfolder level, with `@imports` and `/init`. Gemini CLI uses `GEMINI.md`.
- **Needed:** Load `AGENTS.md`, plus an optional `CHIMERA.md`, with a size cap. Record its hash with each run.

### 3. Native providers and account sign-in
- **CHIMERA:** Only OpenAI-compatible `/chat/completions`, with a hand-entered base URL and key (`docs/USER_MANUAL.md`, Model settings).
- **Competitors:** OpenCode supports 75+ providers through Models.dev. Claude Code signs in with a Pro/Max subscription, Codex with ChatGPT, Gemini CLI with a Google account that has a free tier.
- **Needed:** Provider presets (address and authentication) and native adapters for the major APIs. Keep local-model support first-class.

### 4. Persistent model selection
- **CHIMERA:** `/model NAME` needs the exact string, isn't saved, and `/reload` resets it (`docs/COMMANDS.md`).
- **Competitors:** Claude Code, Codex and OpenCode open a pick-list from `/model` and remember the choice.
- **Needed:** A model pick-list from the provider's model listing, saved per project and recorded in each run.

### 5. Reasoning-effort control
- **CHIMERA:** No setting.
- **Competitors:** Codex has `model_reasoning_effort` in `config.toml` and `/model`. Claude Code has extended thinking and effort controls, and shows reasoning as it streams.
- **Needed:** A configurable effort or thinking-budget setting, passed through where the provider supports it and recorded in each run.

### 6. Continue / new conversation in chat
- **CHIMERA:** "To start a separate conversation, leave and launch a new chat." Resuming needs a session ID (`docs/USER_MANUAL.md`).
- **Competitors:** Claude Code has `claude -c`, `claude -r`, `/resume` and `/clear`. Codex has `codex resume --last` and `/new`.
- **Needed:** `chimera tui --continue`, a `/resume` picker and `/new` inside chat.

### 7. Persistent allow/deny rules
- **CHIMERA:** Each approval covers one exact action and expires after 5 minutes (`crates/chimera-core/src/approval.rs:52`). There's no "always allow this command".
- **Competitors:** Claude Code has "Yes, and don't ask again" and `settings.json` rules such as `Bash(npm test:*)` and `Edit(src/**)`. Codex lets edits inside the workspace sandbox run without asking.
- **Needed:** Allow and deny rules by tool, path and command pattern, versioned in the repo. Deny rules always win. Keep exact-action approval for anything no rule covers. This also cuts the number of prompts people end up approving without reading.

### 8. One unified permission mode
- **CHIMERA:** Five overlapping controls (`policy`, `autonomy`, `leash`, `--profile` and tool disable). The highest setting (`medium`) still reviews every change (`docs/COMMANDS.md`).
- **Competitors:** Claude Code cycles normal → accept-edits → plan with Shift+Tab. Codex has `--full-auto` and `/approvals`. Gemini CLI has `--approval-mode auto_edit` and `--yolo`.
- **Needed:** One mode switch, backed by the policy file (item 21), with the effective policy visible in the status bar.

### 9. Linux and Windows sandbox
- **CHIMERA:** macOS `sandbox-exec` only. Other platforms refuse to run commands (`crates/chimera-shell/src/process.rs:39`, `:68`).
- **Competitors:** Codex sandboxes Linux with Landlock/seccomp or Bubblewrap and has a native Windows sandbox. Claude Code sandboxes Linux with Bubblewrap.
- **Needed:** A Linux sandbox using Landlock or Bubblewrap, then Windows. Keep failing closed where no sandbox is available.

### 10. Diff preview in approvals
- **CHIMERA:** The tool card shows the tool name and raw `args` (`crates/chimera-tui/src/message.rs:135`).
- **Competitors:** Claude Code, Codex and Aider show a colored diff before approval. Claude Code can open it in the IDE for editing.
- **Needed:** A unified diff for `write_file` and `edit_file` in both the chat and terminal review prompts.

### 11. `@file` mentions with fuzzy search
- **CHIMERA:** None.
- **Competitors:** Claude Code, Gemini CLI and OpenCode open a fuzzy file picker on `@`. Aider has `/add` and `/read-only`.
- **Needed:** `@path` completion in `crates/chimera-tui/src/input.rs` that attaches file contents, respecting `.gitignore` and size limits.

### 12. Image and screenshot input
- **CHIMERA:** Chat accepts text only.
- **Competitors:** Claude Code accepts pasted (Ctrl+V) or dragged-in images. Codex has `-i shot.png` and paste. Gemini CLI accepts images through `@file`.
- **Needed:** Image attachments for providers that support images.

### 13. Web fetch and search tools
- **CHIMERA:** Seven built-in tools, none for the web (`crates/chimera-tools/src/lib.rs:208`). The browser feature needs scripted actions plus Node and Puppeteer.
- **Competitors:** Claude Code has `WebFetch` and `WebSearch`. Codex has web search. Gemini CLI has `google_web_search`.
- **Needed:** A `web_fetch` tool limited to allowed origins, off by default. Treat fetched content as untrusted (instructions inside a page can hijack the model), and log every fetch in the run record.

### 14. Automatic checkpoints and rewind
- **CHIMERA:** Checkpoints are manual: `checkpoint create --file X` for each file (`docs/USER_MANUAL.md`, Checkpoints).
- **Competitors:** Claude Code checkpoints every edit and restores with Esc Esc or `/rewind`. Aider commits each change and `/undo` reverts it. Gemini CLI keeps shadow-git checkpoints with `/restore`.
- **Needed:** An automatic checkpoint before every file-changing tool call, plus `/rewind` that can restore code, conversation or both.

### 15. MCP in chat and over HTTP
- **CHIMERA:** `--mcp-config` can't be combined with interactive chat or `--world`. HTTP and SSE servers are rejected (`crates/chimera-mcp/src/stdio.rs:619`).
- **Competitors:** Claude Code has `claude mcp add --transport http` with OAuth, and MCP works in every session. Codex uses the `[mcp_servers]` section of `config.toml`; Gemini CLI uses `mcpServers` in `settings.json`.
- **Needed:** MCP servers available in chat, and the streamable-HTTP transport the config already describes. Keep per-call approval.

### 16. Lifecycle hooks
- **CHIMERA:** No user hooks around tool calls or at the end of a run.
- **Competitors:** Claude Code has PreToolUse, PostToolUse, Stop, Notification and SessionStart hooks that can block an action or run commands. Codex has a `notify` hook.
- **Needed:** Pre- and post-tool hooks plus run-start and run-end hooks, with block or allow results. Hooks must not be able to approve actions the policy would otherwise deny.

### 17. Custom slash commands
- **CHIMERA:** 15 fixed chat commands. `skill run` only exists on the command line and replays a saved objective (`docs/COMMANDS.md`).
- **Competitors:** Claude Code turns `.claude/commands/*.md` and skills into `/name` with arguments. Gemini CLI uses `.gemini/commands/*.toml`, Codex `~/.codex/prompts/`, OpenCode `.opencode/command/`.
- **Needed:** Project prompt templates in `.chimera/commands/`, available as `/name` in chat, with arguments.

### 18. Helper agents from chat
- **CHIMERA:** `swarm`, `pack` and `creature` are separate terminal commands.
- **Competitors:** Claude Code delegates to helper agents mid-chat, with custom agents in `.claude/agents/*.md`. OpenCode calls agents with `@agent`.
- **Needed (later):** Delegation from chat to existing creatures, with their scope and results visible and logged.

### 19. Editor integration
- **CHIMERA:** None.
- **Competitors:** Claude Code has VS Code and JetBrains extensions (selection context, diffs in the editor). Codex has an IDE extension. Gemini CLI has a VS Code companion.
- **Needed (later):** A small editor bridge for the current selection and diff review.

### 20. Unattended / CI runs with declared permissions
- **CHIMERA:** With `--format json`, any action that needs approval is denied unless someone approves it from another terminal (`docs/USER_MANUAL.md`, Approvals).
- **Competitors:** Claude Code has `claude -p` with `--output-format stream-json`, `--allowedTools` and a GitHub Action. Codex has `codex exec` and GitHub review.
- **Needed:** `chimera run --policy ci.toml --format json` with clear exit codes, plus a CI example.

## Part 2 — Harness guarantees that set CHIMERA apart

These turn the existing supervision features into guarantees no competitor currently enforces.

### 21. Single policy-as-code file
Replace `policy`, `autonomy`, `leash` and `--profile` with one versioned file (for example `.chimera/policy.toml`) holding modes plus allow/deny rules. The harness evaluates it the same way every time and records its hash with each run. Items 7 and 8 depend on this.

### 22. Verification gates on completion
A run can't report success until its named `eval` checks pass (`chimera eval` already exists). A failed gate leaves the run unfinished, with the failing output attached.

### 23. Run manifest and reproducible replay
Each run writes a manifest: model, sampling settings, system-prompt and instruction-file hashes, policy hash, tool versions, and every tool call with its result. `replay` can then re-execute it and report where results differ, instead of only displaying the saved conversation.

### 24. Documentation claims match the build
- `CHANGELOG.md` lists "estimated cost" in the status bar, but the status bar always shows "cost unavailable" (`crates/chimera-tui/src/status.rs:44`), and the manual says the app does not estimate cost.
- `CHANGELOG.md` describes releases 0.2.0 and 0.3.0, but the workspace version and README say 0.1.0 (`Cargo.toml`, `README.md`).
- `CHANGELOG.md` calls the interface "competitive with top agentic coding tools", which this review does not support.

Correct these before any public announcement.

### 25. Public reproducibility and benchmark results
Publish repeat-run results (the same task N times, with and without items 21–23) and a public benchmark run, such as Terminal-Bench or SWE-bench with a local model, including the logs.

## Part 3 — Further gaps, not yet assessed in detail

These were named during the review but haven't been checked against CHIMERA's code or competitor documentation yet. Each needs the same evidence review before it's prioritized.

26. A live task / to-do list shown while the agent works.
27. Language-server diagnostics fed back to the model after edits.
28. Opening `$EDITOR` for long prompts (Ctrl+G).
29. A desktop notification when a task finishes or needs approval.
30. Cost estimates from configured provider prices.
31. Shareable session links or exported transcripts.
32. Git helpers for commit messages and pull requests.
33. Per-project settings files that override user settings.
34. Background shell tasks the agent can start and check on.
35. Supported Linux and Windows builds and install packages (the README targets macOS only).
36. A desktop app.

## Recommended order

| Step | Items | Why |
|---|---|---|
| 1 | 2, 10, 24 | Small changes that immediately fix how it feels to use and restore trust in the docs |
| 2 | 21, 7, 8 | Biggest combined safety and usability gain |
| 3 | 16, 14 | The core enforced guarantees |
| 4 | 22, 23, 20 | The position no competitor holds |
| 5 | 1, 11, 4, 5, 6, 15, 9, 17 | Broadens who can use it |
| 6 | 25 | Proves the claims |
| Later | 3, 13, 12, 18, 19, 26–36 | Lower value or already well served elsewhere |

## What CHIMERA should keep

These are already stronger than most competitors and shouldn't be weakened by the work above: exact-action approvals, approval only from an authenticated terminal (never from a webhook reply), failing closed without a sandbox, pausing interrupted background jobs instead of retrying, the stale-read check before edits, redacted session exports, and evidence-based documentation.
