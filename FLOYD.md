# CHIMERA — FLOYD.md
**Version:** 1.7.2
**Initialized:** 2026-05-12T14:58:53-0400
**Governance:** .supercache/ v1.7.2
**Port:** N/A — CLI/TUI tool, no network port binding
**Drive:** Storage
**Path:** /Volumes/Storage/CHIMERA

> **Compliance Notice:** This file must match the template at
> `.supercache/templates/floyd-md-template.md`. If you are an agent reading
> this file and it is missing sections from the template, edit this file to
> add them. Preserve all project-specific content below. The template is the
> spec. This file is the implementation. Make them match.

---

## Agent Contract

You are working on **CHIMERA**, a Legacy AI project.

**This file (`FLOYD.md`) is the canonical project spec.** It is authoritative for project identity, stack, ports, build commands, environment variables, and project-specific rules. All agents — Floyd, Claude, or any model routed through the OhMyFloyd harness — read this file first.

**Some projects also have a `CLAUDE.md` adapter** alongside this file. That adapter is optional and applies only when Claude is the active agent. It does not duplicate anything here; it layers Claude-specific behavior and role guidance on top. If `CLAUDE.md` conflicts with `FLOYD.md` on project facts, `FLOYD.md` wins. See `.supercache/templates/claude-md-template.md` for the adapter spec.

### Before You Start
1. Read this file completely. Do not skim. Every section constrains your behavior.
2. **If you are Claude Code**: also read `CLAUDE.md` if it exists at the project root. It contains your role, division of labor with Floyd, and Claude-specific rules.
3. Read `.supercache/READONLY` — you MUST NOT write to `.supercache/`.
4. Read `SSOT/CHIMERA_SSOT.md` for current project state. Perform the Verification Sweep Protocol defined in `.supercache/contracts/document-management.md` for sections relevant to your task.
5. Read `Issues/CHIMERA_ISSUES.md` for open issues and blockers.
6. Read `.supercache/manifests/port-allocation-policy.yaml` — NEVER use port 3000, 5000, 8000, 8080, or any other forbidden port. This project uses port **{{PORT}}**. Do not change it without Douglas Talley's explicit approval.
7. Read `.supercache/contracts/execution-contract.md` — this governs how you prove your work.
8. Read `.supercache/contracts/repo-structure.md` — canonical layout for this project's language, plus the migration workflow if structural changes are needed.
9. Read `.supercache/contracts/git-discipline.md` — pre-commit checklist, commit message standards, secret hygiene, and reputation guardrails.
10. Read `.supercache/contracts/document-management.md` — Anti-Cruft Rule, canonical document homes, SSOT verification sweep, reference materials tier.
11. Read `.supercache/contracts/repo-hygiene.md` — `.gitignore` baseline for this language, cleanup triggers, project root tidiness standards.
12. Read `.supercache/manifests/model-routing.yaml` — this tells you which LLM to use for what.

### Governance Location
```
.supercache/ → /Volumes/SanDisk1Tb/.supercache
```
This directory contains global templates, contracts, manifests, and routing config.
It is **READ-ONLY**. Do not create, modify, or delete any file there.

### Where You Write

| Location             | Purpose                                          | Example                                         |
|----------------------|--------------------------------------------------|-------------------------------------------------|
| `SSOT/`              | Project status, decisions, findings, verification | `SSOT/CHIMERA_SSOT.md`, `SSOT/decision-log.md` |
| `Issues/`            | Bugs, blockers, tasks, help-desk ledger          | `Issues/CHIMERA_ISSUES.md`, `Issues/0001-description.md` |
| `.floyd/`            | Agent working state, session logs, runtime cache | `.floyd/agent_log.jsonl`                        |
| Project source files | Your actual work                                 | Any file in the project tree not listed below   |

### Where You Do NOT Write

| Location          | Reason                                       |
|-------------------|----------------------------------------------|
| `.supercache/`    | Global governance — READ-ONLY for all agents |
| <!-- ADD HERE --> | <!-- Project-specific no-write zones -->     |

---

## Project Identity

| Field                | Value                                                                   |
|----------------------|-------------------------------------------------------------------------|
| **Name**             | CHIMERA                                                        |
| **Purpose**          | <!-- One sentence. What this project does and why it exists. -->        |
| **Primary Language** | <!-- e.g. TypeScript (ES2022, strict), Python 3.12, Go 1.23 -->         |
| **Runtime**          | <!-- e.g. Node.js ≥ 22.0.0, Python 3.12+, N/A for CLI -->               |
| **Module System**    | <!-- e.g. ESM, CommonJS, Go modules, N/A -->                            |
| **Framework**        | <!-- e.g. FastAPI, Express, Next.js 14, React, None -->                 |
| **Database**         | <!-- e.g. SQLite, PostgreSQL via Prisma, None -->                       |
| **Port**             | **{{PORT}}** — claimed in `/Volumes/SanDisk1Tb/SSOT/port-registry.json` |
| **Repository**       | <!-- e.g. github.com/org/repo or 'None — not yet initialized' -->       |
| **Current Phase**    | <!-- e.g. Active development, Production, Archived, Prototype -->       |

---

## Project Structure

<!-- Replace this entire block with the actual directory tree. -->
<!-- Every entry must have a one-line annotation explaining what it is. -->
<!-- Do not list node_modules, .git, or build cache directories. -->

```
CHIMERA/
├── src/                          # Source code root
│   └── index.ts                  # Entry point
├── tests/                        # Test files
├── SSOT/                         # Project status and decisions
├── Issues/                       # Bug and task tracking
├── .floyd/                       # Agent working state
├── .env                          # Environment variables (DO NOT COMMIT)
├── .env.example                  # Environment variable template
└── FLOYD.md                      # This file
```

---

## Build & Verify Commands

<!-- Every command must be copy-pasteable from the project root. -->
<!-- Every command must have an expected result so the agent knows pass/fail. -->
<!-- If a command does not apply, write 'N/A — [reason]' in the Expected Result column. -->

| Action         | Command                                                        | Expected Result             |
|----------------|----------------------------------------------------------------|-----------------------------|
| **Type check** | <!-- e.g. npm run typecheck, go vet ./..., mypy src/ -->       | Exit 0, no errors           |
| **Build**      | <!-- e.g. npm run build, go build ./..., python -m build -->   | Exit 0                      |
| **Test**       | <!-- e.g. npm test, go test ./..., pytest -->                  | Exit 0, all tests pass      |
| **Lint**       | <!-- e.g. npm run lint, golangci-lint run, ruff check -->      | Exit 0                      |
| **Start**      | <!-- e.g. PORT={{PORT}} node dist/index.js -->                 | Service up on port {{PORT}} |
| **Dev**        | <!-- e.g. npm run dev, go run ., uvicorn app:main --reload --> | Live reload active          |

### Verification sequence after any change:
```bash
# Replace with actual commands. All must exit 0.
# Example: npm run typecheck && npm run lint && npm test && npm run build
```

---

## Port Allocation

This project is a CLI/TUI tool. No port binding. No claim needed.

**Note:** `crates/chimera-tui/src/chat.rs:181` contains `duration_ms: Some(5000)` which is a tool-call timing value (5 seconds), not a port binding. The OMP governance port guard may flag this as a false positive.

---

## Project-Specific Rules

<!-- These rules are unique to THIS project. Violating any of them is a governance failure. -->
<!-- Every rule must have a rationale. If you can't explain why, the rule doesn't belong here. -->
<!-- Delete the examples below and replace with real rules. -->

| #   | Rule                                                                | Rationale                                                     |
|-----|---------------------------------------------------------------------|---------------------------------------------------------------|
| R1  | <!-- e.g. Never delete wa_auth/ — WhatsApp session state -->        | <!-- e.g. Corruption requires re-auth and causes downtime --> |
| R2  | <!-- e.g. All database writes go through the ORM, never raw SQL --> | <!-- e.g. Schema enforcement and migration safety -->         |
| R3  | <!-- e.g. After every build, restart the running process -->        | <!-- e.g. Process serves stale dist/ until restarted -->      |

---

## Known Patterns & Lessons

<!-- Proven solutions to recurring problems. Apply immediately when you hit the trigger. -->
<!-- The agent and the Learning Engine populate this section over time. -->
<!-- Delete the examples below and replace with real patterns as they are discovered. -->

| Pattern                     | Trigger                                  | Fix                                                   | Confidence       |
|-----------------------------|------------------------------------------|-------------------------------------------------------|------------------|
| <!-- e.g. build-restart --> | <!-- e.g. You just ran npm run build --> | <!-- e.g. pkill + restart command -->                 | <!-- 0.0–1.0 --> |
| <!-- e.g. port-conflict --> | <!-- e.g. EADDRINUSE on startup -->      | <!-- e.g. lsof -i :PORT, kill conflicting process --> | <!-- 0.0–1.0 --> |

---

## Environment Variables

<!-- Source: .env (DO NOT COMMIT — .gitignore must list it) -->
<!-- Template: .env.example -->
<!-- If this project has no env vars, write "None — all configuration is hardcoded or CLI-driven." -->

| Variable          | Required        | Purpose               | Example               |
|-------------------|-----------------|-----------------------|-----------------------|
| `PORT`            | Yes             | HTTP server port      | `{{PORT}}`            |
| <!-- ADD HERE --> | <!-- Yes/No --> | <!-- What it does --> | <!-- Sample value --> |

---

## Execution Contract

Before claiming any task complete, provide:

1. **Exact action taken** — what you did, specifically
2. **Direct evidence** — file path + line, command + output, diff, or screenshot
3. **Verification result** — run the verification sequence above, all must exit 0
4. **Status** — mark COMPLETE only after steps 1-3 are proven

See `.supercache/contracts/execution-contract.md` for the full contract.

---

## Mandatory execution contract
For EACH requested item:
1) Show exact action taken
2) Show direct evidence (file/line/command/output)
3) Show verification result
4) Mark status only after proof

## Forbidden behaviors
- Declaring "done" without evidence
- Collapsing multiple requested items into one vague summary
- Skipping failed steps without explicit blocker report

## Required output structure
A) Requested items checklist
B) Per-item evidence ledger
C) Verification receipts
D) Completeness matrix (item -> done/blocked -> evidence)

## Hard gate
If any requested item has no evidence row, final status MUST be INCOMPLETE.
