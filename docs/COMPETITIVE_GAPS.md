# CHIMERA competitive gaps and modernization roadmap

First reviewed 16 October 2026 against `main` at `5b1672f`. Re-checked 23 October 2026 against `main` at `3c7cbb6` (CHIMERA 0.1.0).

This is a planning document. It lists the feature updates that keep CHIMERA from being competitive with current terminal coding agents. Nothing listed here is implemented by this document.

## Changes since the first review

The 39 commits between `5b1672f` and `3c7cbb6` closed or narrowed several gaps. Each change below was checked in the code, not only in commit messages.

| Item | Was | Now | Evidence |
|---|---|---|---|
| 4. Model selection | `/model NAME` only, not saved | **Done:** `/models` lists a service's models and saves the choice as the default | `crates/chimera-tui/src/models.rs`, `crates/chimera-config/src/connections.rs:130` |
| 14. Automatic checkpoints and rewind | Manual `checkpoint create --file` only | **Mostly done:** each turn that edits files leaves an automatic snapshot; `/rewind` previews and restores it | `crates/chimera-core/src/runtime.rs:121`, `crates/chimera-tui/src/rewind.rs` |
| 28. `$EDITOR` for long prompts | Not assessed | **Done:** Ctrl+E or `/editor` | `crates/chimera-tui/src/editor.rs` |
| 3. Providers | Hand-entered address and key | **Partly done:** `/connect` (F2) offers presets for OpenAI, OpenRouter, Groq, LM Studio, Ollama and Custom | `crates/chimera-config/src/connections.rs:11` |
| 6. Continue / new conversation | Session ID needed | **Partly done:** the `/sessions` picker lists and resumes conversations in place, and can delete them | `crates/chimera-tui/src/sessions.rs` |
| 8. Unified permission mode | Five separate controls | **Partly done:** `/permissions` shows and changes them in one place | `crates/chimera-tui/src/permissions.rs` |
| 15. MCP in chat and over HTTP | Not available in chat | **Partly done:** `/mcp` connects stdio servers to chat | `crates/chimera-tui/src/external.rs` |
| 17. Custom slash commands | Command line only | **Partly done:** `/skills NAME` runs saved procedures in chat; built-in commands complete on Tab | `crates/chimera-tui/src/skills.rs`, `crates/chimera-tui/src/input.rs:504` |
| 24. Documentation claims | Three mismatches | **Worse:** the old mismatches remain and the command docs now miss new chat commands | See item 24 |

Unchanged: items 1, 2, 5, 7, 9, 10, 11, 12, 13, 16, 18, 19, 20, 21, 22, 23 and 25. Tool results that look like a diff are now colored (`crates/chimera-tui/src/message.rs:270`), but approvals still show raw arguments, so item 10 stays open.

## How this was assessed

- **CHIMERA facts** come from this repository's code and documentation. File references point to the evidence.
- **Competitor facts** describe Claude Code, OpenAI Codex CLI, Gemini CLI, OpenCode and Aider. They come from those projects' documentation as known to the reviewer, spot-checked against public sources on 14 October 2026. Competitor command names can change between releases, so verify a specific behavior before relying on it.
- **Classification** is a judgement, not a measurement:
  - **Determinism**: the harness itself enforces the behavior the same way every run, independent of model choices or operator memory.
  - **Feature**: adds convenience, reach or review quality without changing how predictable a run is.
  - **Caution**: adds capability but can make runs less predictable unless constrained and logged.

## Summary

| # | Gap | Class | Priority | Status at `3c7cbb6` |
|---|---|---|---|---|
| 1 | Context compaction | Feature (caution) | P1 | Open |
| 2 | Project instruction files (`AGENTS.md`) | Determinism | P0 | Open |
| 3 | Native providers and account sign-in | Feature | P2 | Partly done |
| 4 | Persistent model selection | Determinism | P1 | Done |
| 5 | Reasoning-effort control | Determinism | P1 | Open |
| 6 | Continue / new conversation in chat | Feature | P1 | Partly done |
| 7 | Persistent allow/deny rules | Determinism | P0 | Open |
| 8 | One unified permission mode | Determinism | P0 | Partly done |
| 9 | Linux and Windows sandbox | Feature | P1 | Open |
| 10 | Diff preview in approvals | Feature | P0 | Open |
| 11 | `@file` mentions with fuzzy search | Determinism | P1 | Open |
| 12 | Image and screenshot input | Feature | P3 | Open |
| 13 | Web fetch and search tools | Feature (caution) | P2 | Open |
| 14 | Automatic checkpoints and rewind | Determinism | P0 | Mostly done |
| 15 | MCP in chat and over HTTP | Feature | P1 | Partly done |
| 16 | Lifecycle hooks | Determinism | P0 | Open |
| 17 | Custom slash commands | Determinism | P1 | Partly done |
| 18 | Helper agents from chat | Feature (caution) | P3 | Open |
| 19 | Editor integration | Feature | P3 | Open |
| 20 | Unattended / CI runs with declared permissions | Determinism | P0 | Open |
| 21 | Single policy-as-code file | Determinism | P0 | Open |
| 22 | Verification gates on completion | Determinism | P0 | Open |
| 23 | Run manifest and reproducible replay | Determinism | P0 | Open |
| 24 | Documentation claims match the build | Credibility | P0 | Open, wider |
| 25 | Public reproducibility and benchmark results | Credibility | P2 | Open |
| 26–36 | Further gaps, not yet assessed in detail | — | — | 28 done; rest not assessed |

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
- **CHIMERA (updated):** `/connect` (F2) now offers presets for OpenAI, OpenRouter, Groq, LM Studio, Ollama and Custom, checks the connection and saves it (`crates/chimera-config/src/connections.rs:11`). Every preset still uses the OpenAI-compatible `/chat/completions` format. There's no native Anthropic or Gemini API and no account sign-in.
- **Competitors:** OpenCode supports 75+ providers through Models.dev. Claude Code signs in with a Pro/Max subscription, Codex with ChatGPT, Gemini CLI with a Google account that has a free tier.
- **Still needed:** Native adapters for the major non-OpenAI APIs. Keep local-model support first-class.

### 4. Persistent model selection
- **CHIMERA (updated, done):** `/models` lists the models a saved service offers and saves the choice as the default in the settings file (`crates/chimera-tui/src/models.rs`, `crates/chimera-config/src/connections.rs:130`). `/model NAME` still changes only the open chat.
- **Competitors:** Claude Code, Codex and OpenCode open a pick-list from `/model` and remember the choice.
- **Remaining:** The default is saved per user, not per project, and the model used isn't yet recorded per run. Recording it belongs with item 23.

### 5. Reasoning-effort control
- **CHIMERA:** No setting.
- **Competitors:** Codex has `model_reasoning_effort` in `config.toml` and `/model`. Claude Code has extended thinking and effort controls, and shows reasoning as it streams.
- **Needed:** A configurable effort or thinking-budget setting, passed through where the provider supports it and recorded in each run.

### 6. Continue / new conversation in chat
- **CHIMERA (updated):** The `/sessions` picker lists saved conversations newest first, with title, date and preview, and resumes one in place (`crates/chimera-tui/src/sessions.rs`). There's still no `/new` in chat and no `--continue` flag, and the manual still says to leave and relaunch for a new conversation (`docs/USER_MANUAL.md:149`).
- **Competitors:** Claude Code has `claude -c`, `claude -r`, `/resume` and `/clear`. Codex has `codex resume --last` and `/new`.
- **Still needed:** `/new` inside chat and `chimera tui --continue`.

### 7. Persistent allow/deny rules
- **CHIMERA:** Each approval covers one exact action and expires after 5 minutes (`crates/chimera-core/src/approval.rs:52`). There's no "always allow this command".
- **Competitors:** Claude Code has "Yes, and don't ask again" and `settings.json` rules such as `Bash(npm test:*)` and `Edit(src/**)`. Codex lets edits inside the workspace sandbox run without asking.
- **Needed:** Allow and deny rules by tool, path and command pattern, versioned in the repo. Deny rules always win. Keep exact-action approval for anything no rule covers. This also cuts the number of prompts people end up approving without reading.

### 8. One unified permission mode
- **CHIMERA (updated):** `/permissions` now shows autonomy, project read-only, disabled tools and pending approvals in one panel. Widening access asks for confirmation; narrowing applies at once (`crates/chimera-tui/src/permissions.rs`). The underlying controls are unchanged, though: separate settings, with `medium` the highest autonomy and `full` rejected (`crates/chimera-core/src/approval.rs` tests).
- **Competitors:** Claude Code cycles normal → accept-edits → plan with Shift+Tab. Codex has `--full-auto` and `/approvals`. Gemini CLI has `--approval-mode auto_edit` and `--yolo`.
- **Still needed:** One mode switch, backed by the policy file (item 21), with the effective mode visible in the status bar.

### 9. Linux and Windows sandbox
- **CHIMERA:** macOS `sandbox-exec` only. Other platforms refuse to run commands (`crates/chimera-shell/src/process.rs:39`, `:68`).
- **Competitors:** Codex sandboxes Linux with Landlock/seccomp or Bubblewrap and has a native Windows sandbox. Claude Code sandboxes Linux with Bubblewrap.
- **Needed:** A Linux sandbox using Landlock or Bubblewrap, then Windows. Keep failing closed where no sandbox is available.

### 10. Diff preview in approvals
- **CHIMERA:** The approval notice still shows the tool name and raw arguments (`crates/chimera-tui/src/runner.rs:700`). Since the first review, tool *results* that look like a unified diff are colored (`crates/chimera-tui/src/message.rs:270`), but nothing builds a diff of a proposed edit before approval.
- **Competitors:** Claude Code and Codex show a colored diff before approval, and Claude Code can open it in the IDE for editing. Aider works differently: it applies and auto-commits edits by default, then offers `/diff` and `/undo` for review and rollback (https://aider.chat/docs/git.html) — post-edit review, not an approval boundary.
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
- **CHIMERA (updated, mostly done):** Each turn that edits files now leaves one automatic snapshot (`crates/chimera-core/src/runtime.rs:121`). `/rewind` lists snapshots newest first, previews the exact restore, asks before changing anything, and keeps the current file versions in a private archive (`crates/chimera-tui/src/rewind.rs`). A snapshot failure is logged but doesn't stop the run (`runtime.rs:346`).
- **Competitors:** Claude Code checkpoints every edit made through its file tools (not changes from Bash/shell commands) and restores with Esc Esc or `/rewind` (https://code.claude.com/docs/en/checkpointing). Aider commits each change and `/undo` reverts it. Gemini CLI keeps shadow-git checkpoints with `/restore`.
- **Remaining:** Snapshots are per turn, not per tool call. A failed snapshot should be shown to the user, not only logged, if "edits are always recoverable" is to be claimed. Neither `/rewind` nor the automatic snapshots are in `docs/USER_MANUAL.md` or `docs/COMMANDS.md` yet.

### 15. MCP in chat and over HTTP
- **CHIMERA (updated):** `/mcp` now shows saved connections, offers a guided setup form, starts a server after explicit confirmation, and attaches its tools to the chat. Every tool call still needs exact-action approval (`crates/chimera-tui/src/external.rs`). Only stdio servers are accepted; HTTP and SSE are still rejected (`crates/chimera-mcp/src/lib.rs:248`, `crates/chimera-mcp/src/stdio.rs:619`).
- **Competitors:** Claude Code has `claude mcp add --transport http` with OAuth, and MCP works in every session. Codex uses the `[mcp_servers]` section of `config.toml`; Gemini CLI uses `mcpServers` in `settings.json`.
- **Still needed:** The streamable-HTTP transport. Keep per-call approval.

### 16. Lifecycle hooks
- **CHIMERA:** No user hooks around tool calls or at the end of a run.
- **Competitors:** Claude Code has PreToolUse, PostToolUse, Stop, Notification and SessionStart hooks that can block an action or run commands. Codex has a `notify` hook.
- **Needed:** Pre- and post-tool hooks plus run-start and run-end hooks, with block or allow results. Hooks must not be able to approve actions the policy would otherwise deny.

### 17. Custom slash commands
- **CHIMERA (updated):** `/skills` lists, captures and runs saved procedures inside chat as normal turns with the usual approvals (`crates/chimera-tui/src/skills.rs`). Built-in commands complete on Tab (`crates/chimera-tui/src/input.rs`). Procedures replay a saved objective and take no arguments, and the user can't add their own `/name` commands.
- **Competitors:** Claude Code turns `.claude/commands/*.md` and skills into `/name` with arguments. Gemini CLI uses `.gemini/commands/*.toml`, Codex `~/.codex/prompts/`, OpenCode `.opencode/command/`.
- **Still needed:** Project prompt templates in `.chimera/commands/`, available as `/name` in chat with arguments and Tab completion.

### 18. Helper agents from chat
- **CHIMERA:** `swarm`, `pack` and `creature` are separate terminal commands.
- **Competitors:** Claude Code delegates to helper agents mid-chat, with custom agents in `.claude/agents/*.md`. OpenCode calls agents with `@agent`.
- **Needed (later):** Delegation from chat to existing creatures, with their scope and results visible and logged.

### 19. Editor integration
- **CHIMERA:** None.
- **Competitors:** Claude Code has VS Code and JetBrains extensions (selection context, diffs in the editor). Codex has an IDE extension. Gemini CLI has a VS Code companion.
- **Needed (later):** A small editor bridge for the current selection and diff review.

### 20. Unattended / CI runs with declared permissions
- **CHIMERA:** With `--format json`, any action that needs approval is denied, unless `CHIMERA_WAIT_APPROVAL=1` is set and someone approves it from an authenticated terminal in the same project; otherwise the run denies the action (`crates/chimera-cli/src/main.rs`, `docs/USER_MANUAL.md`, Approvals).
- **Competitors:** Claude Code has `claude -p` with `--output-format stream-json`, `--allowedTools` and a GitHub Action. Codex has `codex exec` and GitHub review.
- **Needed:** `chimera run --policy ci.toml --format json` with clear exit codes, plus a CI example.

## Part 2 — Harness guarantees that set CHIMERA apart

These turn the existing supervision features into guarantees the harness enforces by default. Some competitors let users build parts of this themselves (for example, a Claude Code Stop hook that blocks completion until tests pass). None known to this review ships it built in.

### 21. Single policy-as-code file
Replace `policy`, `autonomy`, `leash` and `--profile` with one versioned file (for example `.chimera/policy.toml`) holding modes plus allow/deny rules. The harness evaluates it the same way every time and records its hash with each run. Items 7 and 8 depend on this.

### 22. Verification gates on completion
A run can't report success until its named `eval` checks pass (`chimera eval` already exists). A failed gate leaves the run unfinished, with the failing output attached.

### 23. Run manifest and reproducible replay
Each run writes a manifest: model, sampling settings, system-prompt and instruction-file hashes, policy hash, tool versions, and every tool call with its result. `replay` can then re-execute it and report where results differ, instead of only displaying the saved conversation.

### 24. Documentation claims match the build
Still true at `3c7cbb6` (`CHANGELOG.md` hasn't changed since the first review):
- `CHANGELOG.md` lists "estimated cost" in the status bar, but the status bar always shows "cost unavailable" (`crates/chimera-tui/src/status.rs:61`), and the manual says the app does not estimate cost.
- `CHANGELOG.md` describes releases 0.2.0 and 0.3.0, but the workspace version and README say 0.1.0 (`Cargo.toml`, `README.md`).
- `CHANGELOG.md` calls the interface "competitive with top agentic coding tools", which this review does not support.

New since the first review:
- `CHANGELOG.md` doesn't mention any of the features added since, such as `/connect`, `/models`, `/sessions`, `/rewind`, `/permissions`, `/memory`, `/skills`, `/mcp`, `/jobs` and `/editor`.
- `docs/COMMANDS.md` and `docs/USER_MANUAL.md` don't document `/rewind`, `/permissions`, `/mcp`, `/memory`, `/skills`, `/jobs` or `/editor`, although the in-app help lists them (`crates/chimera-tui/src/help.rs`).
- `docs/COMMANDS.md:37` still describes `/sessions` as listing identifiers; it now opens a picker that resumes and deletes conversations.
- `docs/USER_MANUAL.md:149` still says to leave and relaunch to start a new conversation.

Correct these before any public announcement.

### 25. Public reproducibility and benchmark results
Publish repeat-run results (the same task N times, with and without items 21–23) and a public benchmark run, such as Terminal-Bench or SWE-bench with a local model, including the logs.

## What the positioning claim requires

The intended position is:

> The coding agent whose rules are enforced by default, and whose runs carry a manifest you can re-run to check reproducibility.

Don't claim "the only coding agent" until item 25 has published evidence and a current competitor check backs it up.

| Requirement | Items | What must be true before claiming it |
|---|---|---|
| Rules enforced by the harness | 21, 7, 16 | Policy and hooks are evaluated in code on every tool call. A model or a hook can't bypass a deny rule. |
| Edits are always recoverable | 14 | Every file-changing tool call creates a checkpoint without any operator action. |
| Completion is verified | 22 | A run with a gate can't report success until the named `eval` passes. |
| Runs are reproducible and checkable | 23 | Each run writes a manifest, and `replay` re-executes it and reports any differences. |
| Works with no one at the keyboard | 20 | Unattended runs use only declared permissions and return clear exit codes. |
| Evidence is public | 25 | Repeat-run and benchmark results are published with their logs. |

### Keeping the claim from hurting the app or its users

- **"Reproducible" means checkable, not identical.** Hosted models can return different output at temperature 0 and can change behind the same name, so no harness can guarantee identical model output. Replay should offer two modes: re-executing the recorded model responses, which repeats the tool steps exactly, and re-asking the model, which reports where results differ. Wording must promise that a run can be checked, never that it will come out the same.
- **Strict must not mean slow to start.** Ship a standard policy preset that lets reads and edits inside the project pass by rule, and keep prompts for commands, network and anything outside the project. Strict presets stay opt-in. Otherwise the positioning brings back the approval fatigue item 7 is meant to fix.
- **Manifests hold private data.** Tool results can contain source code and secrets. Store manifests under `.chimera/` with the same privacy rules as sessions, and redact them in exports, as `session export` already does.
- **Overhead stays bounded.** Checkpoints need a retention limit, and gates run only the checks a project names. Manifests store hashes plus references to data already kept on disk, rather than second copies.
- **The claim isn't the whole product.** Items 1–20 still decide whether people use CHIMERA every day. The positioning only helps once those basics are in place.

## Part 3 — Further gaps, not yet assessed in detail

These were named during the review but haven't been checked against competitor documentation yet. Each needs the same evidence review before it's prioritized.

26. A live task / to-do list shown while the agent works.
27. Language-server diagnostics fed back to the model after edits.
28. ~~Opening `$EDITOR` for long prompts.~~ **Done:** Ctrl+E or `/editor` opens the configured editor with the draft and puts the saved text back without sending it (`crates/chimera-tui/src/editor.rs`).
29. A desktop notification when a task finishes or needs approval.
30. Cost estimates from configured provider prices.
31. Shareable session links or exported transcripts.
32. Git helpers for commit messages and pull requests.
33. Per-project settings files that override user settings.
34. Background shell tasks the agent can start and check on.
35. Supported Linux and Windows builds and install packages (the README targets macOS only).
36. A desktop app.

## Recommended order

Updated 23 October 2026. Items marked done are removed; partly done items keep only their remaining work.

| Step | Items | Why |
|---|---|---|
| 1 | 24, 2, 10 | The command docs now miss seven chat commands; fix that first, then the two small daily-use gaps |
| 2 | 21, 7, 8 | Biggest combined safety and usability gain; `/permissions` is a ready home for the rules |
| 3 | 16, 14 (remaining) | The core enforced guarantees; rewind already covers most of 14 |
| 4 | 22, 23, 20 | The position no competitor holds |
| 5 | 1, 11, 5, 6, 15, 9, 17 (remaining) | Broadens who can use it |
| 6 | 25 | Proves the claims |
| Later | 3 (remaining), 13, 12, 18, 19, 26–27, 29–36 | Lower value or already well served elsewhere |

## What CHIMERA should keep

These are already stronger than most competitors and shouldn't be weakened by the work above: exact-action approvals, approval only from an authenticated terminal (never from a webhook reply), failing closed without a sandbox, pausing interrupted background jobs instead of retrying, the stale-read check before edits, redacted session exports, and evidence-based documentation.
