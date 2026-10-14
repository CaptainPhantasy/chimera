# CHIMERA Issues Ledger
**Created:** 2026-05-12T14:58:53-0400
**Governance:** .supercache/ v1.7.2

> **Compliance Notice:** This file must match the structure at
> `.supercache/templates/issues-template.md`. This is the living help-desk
> and issue tracker for **CHIMERA**.

---

## How to use this document

- This is the living help-desk for repo operations, CI/CD, bugs, and blockers for CHIMERA.
- Every new issue is added as a row in the **Issues Ledger** below with a fresh `ISSUE-NNNN` ID.
- Every significant update to an issue appends a timestamped entry to the **Change Log** at the bottom of this file.
- **Never overwrite historical facts.** Updates append; they do not replace.

---

## Status definitions

| Status | Meaning |
|---|---|
| **New** | Captured; not yet triaged |
| **Triaged** | Scoped; priority set; owner assigned |
| **In progress** | Active work underway |
| **Blocked** | Cannot proceed; blocker and next unblock action recorded |
| **Resolved** | Fix implemented; proof attached |
| **Verified** | Fix confirmed by rerun, test, or log evidence |
| **Closed** | Complete and stable; no further action expected |

Issues move forward through these states. Backward transitions are allowed if new information invalidates an earlier state (e.g., a Closed issue reopens if the bug recurs).

---

## Issues Ledger

| ID | Created | Title | Status | Owner | Evidence / Links | Resolution Proof |
|---|---|---|---|---|---|---|
| ISSUE-0001 | YYYY-MM-DD HH:MM TZ | Example issue — replace with real entries | New | Unassigned | Logs, screenshots, commands, failing step | N/A |

<!-- Each new issue gets its own row. Keep rows compact; if a row needs more detail, -->
<!-- create a companion file at Issues/NNNN-brief-description.md and link it in Evidence. -->

---

## Required fields per issue

Every row above MUST have:

1. **ID** — `ISSUE-NNNN`, monotonically increasing, never reused
2. **Created** — `YYYY-MM-DD HH:MM TZ` when the issue was first captured
3. **Title** — one-line summary
4. **Status** — from the status table above
5. **Owner** — assigned person, or "Unassigned"
6. **Evidence / Links** — logs, screenshots, commands, failing step, related file paths, companion issue file if present
7. **Resolution Proof** — how the fix was verified; "N/A" until Resolved or later

If any field is missing, the row is non-compliant and must be corrected.

---

## Per-issue detail files (optional)

For issues that need more than a single ledger row, create a companion file:

```
Issues/
├── CHIMERA_ISSUES.md       (this file — the ledger)
├── 0001-brief-description.md        (deep detail for ISSUE-0001)
├── 0002-another-issue.md            (deep detail for ISSUE-0002)
└── ...
```

Link the companion file from the ledger row's Evidence / Links column.

Companion files should contain:

- Full reproduction steps
- Observed vs expected behavior
- Logs and error messages
- Suspected root cause
- Proposed fix(es) with tradeoffs
- Verification plan

---

## Change Log (append-only)

- 2026-05-12T14:58:53-0400 — Initialized issues ledger.

<!-- Append new entries BELOW this comment line, keeping them in chronological order. -->
<!-- Never edit or remove existing entries — this is an audit trail. -->

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
