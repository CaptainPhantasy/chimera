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


## 2026-11-03: GitHub review issues #5–#8

Implemented on `fix/session-and-connection-lifecycle`, based on the existing developer workspace branch. The four direct acceptance cases below each define WHAT, WHY and an observable PASS/FAIL. No broad test suite was executed locally. GitHub CI also retains the twenty existing direct UX acceptance cases. Merge and issue closure are separate from implementation verification.

| Issue | Correction | Direct evidence | Local result |
| --- | --- | --- | --- |
| [#5](https://github.com/CaptainPhantasy/chimera/issues/5) | `FileSessionStore::delete` permanently removes the session's checkpoint records/backups, owned restore directories, active/archived/recovered records and identifiable pending writes. Reversible UI removal calls `archive` explicitly. | `chimera-session` case `issue_05_deleted_conversation_has_no_retained_recovery_data` creates real checkpoints/restores/archive, checks that session bytes disappear, verifies another session's recovery and working files survive, and checks fail-closed behavior for an unidentified partial write. | PASS |
| [#6](https://github.com/CaptainPhantasy/chimera/issues/6) | Async store entry points move lock acquisition, reads/writes and fsync into blocking workers. The live callback uses an ordered bounded progress queue, drains before final saves, and keeps the run lease in every in-flight transaction. | `chimera-session` case `issue_06_workspace_responds_while_a_save_waits_for_the_lock` demonstrates a UI heartbeat executing on a single-threaded async runtime before the external storage lock is released; the save subsequently completes. Callback ordering/cancellation lease retention were independently source-reviewed. | PASS |
| [#7](https://github.com/CaptainPhantasy/chimera/issues/7) | Connection checks always await shutdown after ping, preserving both the protocol error and any cleanup error. | `chimera-tui` case `issue_07_failed_connection_checks_stop_the_actual_server_and_show_the_error` uses real invalid-response and timed-out servers, verifies the error menu and observes the recorded child PID has gone. | PASS |
| [#8](https://github.com/CaptainPhantasy/chimera/issues/8) | Detach displays stopped status, removes attached tools, and preserves cleanup ownership for retry if stopping fails. | `chimera-tui` case `issue_08_detach_shows_stopped_connection_in_the_actual_menu` attaches/detaches a real server, renders the returned menu and verifies stopped status, unavailable tools and process exit. Cleanup-error retry behavior was source-reviewed. | PASS |

Permanent deletion here covers the session store, its checkpoints and restore recovery records. It does not erase working files, independent TUI scrollback, external backups or physical disk sectors. Unidentifiable partial legacy writes cause an explicit error before any records are removed; recovery/ownership resolution is required before retrying. No actual user sessions were deleted during implementation or verification; fixtures are disposable local workspaces.

Verification commands:

```sh
cargo test --locked --offline -p chimera-session -p chimera-tui --lib issue_ -j 2 -- --test-threads=1
cargo fmt --package chimera-session --package chimera-tui -- --check
cargo clippy --workspace --all-targets --locked --offline -j 2 -- -D warnings
cargo build --locked --offline -p chimera-cli -j 2
```

The integrated run passed all four cases. After strengthening interrupted-write ownership handling, the same two session cases passed again. Publication requires Git Steward approval and successful macOS CI on the exact pushed revision. CI's test commands are limited to the four issue cases plus the existing twenty direct developer UX cases, each documented with its user-facing purpose; unrelated suites are not executed.
