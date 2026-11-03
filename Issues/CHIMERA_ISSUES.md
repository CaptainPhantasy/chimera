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


## 2026-11-03: Ten follow-up review findings on PR #9

The ten items are Copilot review threads on [PR #9](https://github.com/CaptainPhantasy/chimera/pull/9), rather than new GitHub issue tickets. Implemented on `fix/reviewed-workspace-safety`. Each new `reviewfix_NN` acceptance case states WHAT, WHY and an observable pass/fail condition. The initial `review_` filter accidentally included three older preview tests; all passed, but the filter exceeded the requested scope. It was replaced with the unique `reviewfix_` filter, which selects exactly these ten cases. No broad suite was run.

| Finding | Correction | Direct case | Local result |
| --- | --- | --- | --- |
| [R01 FIFO preview](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974503) | Reject special files before opening; nonblocking/no-follow open and descriptor checks also protect a replacement race. Unsafe previews remain Deny-only. | `reviewfix_01_fifo_preview_refuses_without_waiting_for_a_writer` — real FIFO in a bounded subprocess, actual denial preview. | PASS |
| [R02 queued conversation](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974563) | Bind dispatch to its originating session, track the in-flight last item, block context switches and retain mismatched/paused deliveries. | `reviewfix_02_queued_work_keeps_its_original_session_context` — open/restore/new refusal, last-item dispatch gap and mismatched-context retention. | PASS |
| [R03 effective PLAN](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974601) | Actual external-start and rewind-confirm paths honor current PLAN plus sticky launch read-only. | `reviewfix_03_plan_and_launch_read_only_guard_mutating_actions` — shared production mutation guards refuse both actions and preserve launch restriction. | PASS |
| [R04 repeated wrapping](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974657) | Cache wrapped rows by retained immutable text identity and width; steady redraw renders only the viewport. | `reviewfix_04_large_review_redraws_scrolls_and_resizes_without_rewrapping_unchanged_text` — 60,000-line document, repeated redraw/End, resize and same-length edit. | PASS |
| [R05 page boundaries](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974706) | Carry an absolute before-entry cursor; concurrent append cannot shift previously selected page boundaries. | `reviewfix_05_history_cursor_preserves_contiguous_pages_during_append` — original entries appear once, without overlap or gaps after append. | PASS |
| [R06 clear pending tools](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974757) | Refuse Clear during a running request; the refusal does not append a notice that could evict the pending card. | `reviewfix_06_clear_during_a_tool_preserves_its_result_card` — real completion update still reaches the visible card after refused Clear. | PASS |
| [R07 restored completion](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974807) | Output with unavailable success metadata is explicitly completed/status unknown; known success/failure remain intact. | `reviewfix_07_restored_tool_output_has_an_honest_completed_status` — restores saved assistant/tool messages and renders completed output without a running/success/failure claim. | PASS |
| [R08 archive reads](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974874) | Search and paging execute in a serialized blocking worker with polled delivery, cancellation and exact dialog ownership; searches use a start-time snapshot. | `reviewfix_08_archive_reads_leave_the_workspace_responsive` — Escape and subsequent draft editing remain usable; background search/page results are displayed. | PASS |
| [R09 queued slash text](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974941) | Already-parsed queued objectives bypass command dispatch and keep their captured mode. | `reviewfix_09_queued_slash_objectives_remain_literal_requests` — slash paths and `/quit` remain literal objectives. | PASS |
| [R10 session I/O wait](https://github.com/CaptainPhantasy/chimera/pull/9#discussion_r4453974988) | Open, restore, archive and new-conversation storage run as background jobs; stale delivery cannot replace an unrelated dialog. | `reviewfix_10_restore_wait_keeps_escape_responsive_and_can_resume` — archive/restore with a held lock, Escape, successful retry and same-title unrelated-modal preservation. | PASS |

Independent review identified and corrected the in-flight queue gap, foreground archive/new-conversation waits and modal ownership. Dialog ownership uses a private UUID, avoiding allocator-pointer reuse. The existing global interrupt path also cancels pending external connection work, addressing the related Codex comment. Its cancellation call was source-reviewed; no extra standalone case was added. Two other Codex comments duplicate R07/R09.

Limits: a document's first layout/changed-width layout still requires one complete wrapping pass; subsequent idle redraws reuse it. Started read-only blocking reads may finish after cancellation, but they cannot publish stale results and only one reader per screen executes at once. Started session filesystem operations may also finish after cancellation; the preserved session remains available while UI delivery is canceled. Missing historical tool success metadata is explicitly unknown rather than guessed.

Verification: exactly ten new cases passed; terminal formatting, strict workspace/all-target Clippy and CLI application build passed. The FIFO case runs its dangerous operation in a bounded child so a regression cannot hang the check. Existing UX09/UX10/UX11 and archive-restore expectations were updated for polled delivery without adding duplicate cases. CI retains the twenty existing direct UX cases, four prior lifecycle cases and these ten review cases; unrelated suites are not selected.

```sh
cargo test --locked --offline -p chimera-tui --lib reviewfix_ -j 2 -- --test-threads=1
cargo fmt --package chimera-tui -- --check
cargo clippy --workspace --all-targets --locked --offline -j 2 -- -D warnings
cargo build --locked --offline -p chimera-cli -j 2
```
