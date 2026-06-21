# CHIMERA SSOT (Single Source of Truth)
**Created:** 2026-05-12T14:58:53-0400
**Last Updated:** 2026-05-12T14:58:53-0400
**Governance:** .supercache/ v1.7.0

> **Compliance Notice:** This file must match the structure at
> `.supercache/templates/ssot-template.md`. This is the authoritative
> document for architecture and programmatic change facts of **CHIMERA**.

---

## Authority

This document is the **single source of truth** for architecture and programmatic change facts of CHIMERA. All other documents must be treated as **potentially flawed** unless their facts are confirmed here.

When a fact in any other document contradicts this SSOT, the SSOT wins. If the SSOT itself is wrong, it is corrected via the **Verification Sweep Protocol** below, not by editing other documents to match.

---

## Verification Sweep Protocol (required on every read)

When an agent reads this SSOT to perform a task:

1. Perform a **line-by-line verification review** of the sections relevant to the current task.
2. For each verified fact, append a verification entry to the **Verification Log** at the bottom of this file with:
   - Timestamp (`YYYY-MM-DD HH:MM TZ`)
   - Section/line reference
   - Evidence source (code path + line, command + output, build log, runtime behavior, etc.)
   - Confidence = 100%
3. If any fact cannot be verified to 100% confidence:
   - Mark it **UNVERIFIED** inline in the section where it appears
   - Add an entry to `Issues/CHIMERA_ISSUES.md` to track the discrepancy
   - Do NOT proceed on the assumption that the fact is true

### Positive Reinforcement (required)

For each fact verified at 100% confidence during a sweep, emit the acknowledgement:

```
Verified as fact (100%): <fact summary>
```

This pattern is deliberate — it reinforces evidence-first thinking and makes the verification record auditable after the fact.

---

## Current State

**Phase:** <!-- e.g., Active development, Production, Maintenance, Archived -->
**Status:** <!-- Active / Paused / Archived -->
**Last Agent Session:** <!-- YYYY-MM-DD HH:MM TZ -->

---

## Architecture Facts

<!-- Add verified architecture facts here. Keep each fact concise and evidence-backed. -->
<!-- Facts should be the kind of thing that, if wrong, would mislead the next agent. -->

### Stack

- **Primary language**: <!-- e.g., TypeScript (ES2022, strict), Python 3.12, Rust 1.75 -->
- **Framework**: <!-- e.g., Next.js 16.1.0, FastAPI, None -->
- **Runtime**: <!-- e.g., Node.js 22.x, Python 3.12, N/A -->
- **Module system**: <!-- e.g., ESM, CommonJS, Cargo, Go modules -->

### Key architectural choices

<!-- Document architectural decisions in 1-3 sentences each. -->
<!-- Link to the full rationale in Key Decisions section below if needed. -->

---

## Key Decisions

| Date | Decision | Rationale | Decided By |
|---|---|---|---|
| 2026-05-12T14:58:53-0400 | <!-- Example: Chose X over Y --> | <!-- Example: because Z --> | <!-- Name or "Unassigned" --> |

<!-- Decisions are append-only. When a decision is superseded, add a new row with the -->
<!-- superseding decision and link back to the old one. Never edit historical rows. -->

---

## Dependencies

| Dependency | Version | Purpose | Criticality |
|---|---|---|---|
| <!-- e.g., next --> | <!-- 16.1.0 --> | <!-- App framework --> | <!-- critical / supporting / dev-only --> |

---

## Deployment

| Environment | URL / Location | Status | Last Deploy |
|---|---|---|---|
| production | <!-- e.g., https://example.com --> | <!-- live / down / maintenance --> | <!-- YYYY-MM-DD --> |
| staging | <!-- e.g., https://staging.example.com --> | <!-- --> | <!-- --> |
| local | <!-- e.g., localhost:{PORT} --> | <!-- dev --> | <!-- N/A --> |

---

## Known Patterns & Lessons

<!-- Proven solutions to recurring problems in this project. Apply immediately when you hit the trigger. -->

| Pattern | Trigger | Fix | Confidence |
|---|---|---|---|
| <!-- e.g., build-restart --> | <!-- e.g., After running build --> | <!-- e.g., pkill + restart --> | <!-- 0.0-1.0 --> |

---

## Verification Log (append-only)

Every sweep of this SSOT must append one or more entries here. Never edit or remove existing entries.

| Timestamp | Section / Line | Fact Verified | Evidence Source | Confidence |
|---|---|---|---|---|
| 2026-05-12T14:58:53-0400 | Authority | Document initialized as SSOT | bootstrap.sh --init created from template | 100% |

---

## Change Log (append-only)

- 2026-05-12T14:58:53-0400 — Initialized SSOT.

<!-- Append new entries BELOW this comment line, in chronological order. -->
<!-- Never edit or remove existing entries — this is the authoritative change history. -->

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
