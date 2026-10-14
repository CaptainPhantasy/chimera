# CHIMERA — marked beta list and handoff

Reconciled 2026-10-12 UTC by the replacement Codex session. **13 original requirements have recorded Mac acceptance evidence. The other 12 entries distinguish remaining personal-use checks from broader release criteria that are now out of scope. Reliable use of the current app for the owner's intended tasks is not yet fully substantiated.**

This is the safe separate status document authorized for the dirty checkout. The [original canonical checklist](beta-release-todo.md), its full requirement/test wording, [original review](../SSOT/beta-readiness-2026-10-11.md), and [previous implementation report](../SSOT/beta-implementation-2026-10-11.md) are preserved. Original IDs remain unchanged. This reconciliation governs the current Mac handoff; historical statements that Android remains required do not govern this task.

**Android: OUT OF SCOPE by the owner's instruction, never passed.** No Android app, SDK, emulator, download, license acceptance, or testing is requested or required for this Mac reconciliation.

**Owner's scope clarification — 2026-10-11, America/Indianapolis:** CHIMERA is a personal macOS application. “Beta quality” means human usable and reliable for its intended tasks on the owner's Mac. Licensing, license acceptance, public distribution, developer signing, notarization, public/private security-reporting contacts, other operating systems, fresh-machine installation, fresh OS accounts, a recruited first-time tester, and compatibility with every hosted model service are not requirements for this goal. They must not be used as blockers. Scope exclusion is not a passing test. Preserve the older broad checklist as history rather than imposing its distribution criteria on this personal app.

Checked B-numbered entries below credit acceptance checks already executed on the identified retained Mac build, with matching unchanged backend source. They do not certify a new release containing the later terminal changes. Checked subitems under open entries credit only that specific completed work. Missing acceptance evidence stays open; absence of evidence is not a reproduced product failure.

## Quality standard supplied by the owner

Use the owner's five stated beta criteria as the quality bar. Do not invent additional requirements or turn an unperformed check into a confirmed critical defect. The personal macOS scope above remains in force; no external rollout was requested.

| Owner's criterion | Existing evidence | Work still needed or evidence limit |
|---|---|---|
| Functional completeness | Checked backend features and saved terminal-flow results in A/B. | Validate the primary intended tasks through the current actual app and intended model; test counts alone do not establish feature completeness. Keep primary feature scope fixed during validation. |
| Quality assurance and stability | Retained workspace, terminal and lifecycle checks have concrete passing receipts. | Establish whether any actual blocker/critical bugs remain, verify responsiveness in intended long sessions, and document known minor issues. No crash-free percentage was measured or claimed; the owner's 99% example is not an observed result. |
| Operational and infrastructure readiness | Saved sessions, trace/export, errors, recovery and controlled update preservation have receipts. | Verify useful automatic error capture, a repeatable hotfix path and an effective feedback path for the owner's use. No particular hosted telemetry vendor or external community service is mandatory. |
| Security and compliance | File boundaries, exact permissions, private state and redacted diagnostics have passing receipts. | Dependency/security static-analysis reports with no high-risk findings are not supplied by the retained evidence. Safety tests are not substitutes for those reports. External-user compliance is not claimed for this personal-use scope. |
| Documentation and support | A saved guide and actual help/control checks exist. | Verify that the owner's intended workflow and recovery instructions match the current app. An agreement for an external testing rollout is a separate rollout item; it is not an instruction to license this personal application or a prerequisite for using it. |

These rows identify completed evidence and remaining validation work. They do not authorize new features, installation, paid services, contracts, Android work or a public rollout. No new defect, test pass or readiness claim is added by this correction.

## A) Marked requirements

- [x] **B02 — Enforce planning limits and action permissions in code.** Recorded plan/write refusal, individual CLI and terminal approval/denial, one-use/changed/expired approvals, live policy, autonomy and leash checks. Evidence: R-tests `plan_denies_mutation_even_when_called_directly`, `approvals_are_exact_one_use_expiring_and_revocable`, `policy_changes_apply_to_already_open_instances`, `scopes_leash_and_file_conflicts_are_enforced_at_dispatch`; R-flows plan, remote approval/denial and actual file results; T-flows plan/deny/tool-result checks.
- [x] **B03 — Keep tools inside their allowed boundaries.** Recorded outside/parent/private/symlink refusals and actual macOS shell denial of outside files, private files, environment and network, with allowed work succeeding. Evidence: R-tests `rejects_outside_parent_private_and_symbolic_paths`, `sandbox_denies_outside_files_private_files_environment_and_network`, `saved_agent_reads_writes_and_executes_only_in_selected_worktree`. Unsupported isolation fails explicitly; this credit is Mac only.
- [ ] **B04 — Preserve edits and keep private data private.**
  - [x] Concurrent human-edit detection, recoverable originals, checkpoint quarantine, private records, fake-secret export redaction and approval-message masking have passing R-tests/R-flows receipts. `.chimera/` protection exists in the current `.gitignore`.
  - [ ] The complete stated interrupted built-in file-write/recovery journey is not separately substantiated here. Session/trace interruption results under B12 do not by themselves close this file-edit requirement.
- [x] **B05 — Make cancellation and timeouts stop the work.** Recorded stalled-request cancellation, child-group timeout/close, no later write, preserved interrupted outcomes, no replayed effects and terminal restoration. Evidence: R-tests `dropping_agent_future_cancels_provider_work`, `timeout_and_dropped_future_kill_child_groups`, `closing_world_cancels_owned_shell_and_never_runs_later_write`, `pause_running_job_and_service_crash_do_not_replay_effects`; R-startup 40 and R-prompts 11 passing checks; T-flows cancellation and T-lifecycle 48 checks of normal/error/panic/owner restoration.
- [x] **B06 — Report the real outcome.** Recorded provider/tool/empty/truncated/malformed/iteration failures and visible non-success results. Evidence: R-tests `failed_tool_is_saved_and_stops_before_a_false_success_answer`, `empty_truncated_and_malformed_tools_are_errors_without_side_effects`, `loop_stops_on_max_iterations`, `zero_and_one_iteration_limits_count_actual_requests`, `transport_errors_and_limits_are_preserved_without_exposing_credentials`; R-flows missing-tool/server/empty/cut-off checks; T-flows saved fullscreen provider error.
- [ ] **B07 — Connect the interactive screen to the agent.**
  - [x] R-flows records approved real file edits through both `tui` and `run --interactive`, follow-up/resume context and one-time submission. T-flows records current screen requests, saved context and real approved tool results.
  - [ ] Verify the current runnable build the owner will actually use on this Mac. Exercise both interactive entrypoints if both remain advertised. The refresh's 105 checks are explicitly `tui_only`, so they do not independently close both current entrypoints. A public release package is not required.
- [ ] **B08 — Fix typing, navigation, and exit.**
  - [x] Current refresh has 70 terminal tests, 105 actual screen-flow checks, 48 lifecycle checks and three suggestion sessions, with recorded paste, multiline/history, ordinary `?`/`q`, help/scrolling, tool details, themes, sizes and restoration. Skill, knowledgebase and blueprint artifacts are present and match retained hashes.
  - [ ] Verify readable input/output and advertised controls in the owner's normal terminal, including text used in the intended tasks. Native Chinese/emoji font appearance remains unverified by offline/browser records; universal font coverage is not a personal-use release gate.
- [x] **B09 — Show model output as it arrives and handle broken responses.** Recorded delayed-first-fragment delivery, every UTF-8 split, interleaved tool arguments, usage and corrupt/truncated/error rejection. Evidence: R-tests `decoder_preserves_every_utf8_split_and_delivers_before_finish`, `decoder_assembles_interleaved_tools_and_usage`, `decoder_rejects_corrupt_empty_truncated_and_error_events`, `absent_usage_is_not_presented_as_zero_usage`, `network_stream_delivers_before_server_finishes_and_stops_at_done_marker`; R-flows `first text displayed before completion`. The model client/backend matches retained source.
- [ ] **B10 — Make configuration and model connections reliable.**
  - [x] Saved configuration/precedence/invalid-address/provider-error tests pass. An earlier debug-build local-model receipt records a real tool read and streamed answer.
  - [ ] Verify a real task with the owner's intended model using the current runnable build, including actual tool work and streaming. Earlier real-model debug binary hash differs from both the retained release and terminal refresh. Testing every provider or obtaining hosted credentials the owner does not use is out of scope.
- [x] **B11 — Save runs and restore their context.** Recorded run/resume/fork/replay/export and distinct outcomes. Evidence: R-tests `conversation_survives_restart_and_fork_without_replaying_actions`, `cancellation_saves_input_and_distinct_outcome`, `stored_permission_claim_cannot_authorize_a_denied_write`; R-flows separate fork identity, resumed context, readable redacted export and replay. Storage/runtime source is unchanged.
- [x] **B12 — Prevent lost or misleading saved history.** Recorded interrupted writes, concurrent independent processes, legacy/corrupt records, OS write refusal and prior trace history. Evidence: R-tests `partial_write_error_and_killed_writer_preserve_committed_bytes`, `independent_processes_keep_every_event_and_crashed_lock_releases`, `operating_system_read_only_storage_does_not_report_an_event_saved`, `legacy_load_versioned_save_and_explicit_corruption`, `reopen_then_append_includes_prior_history`, `real_partial_trace_write_preserves_saved_history_and_has_no_cache_success`.
- [x] **B13 — Make checkpoints restore real state.** Recorded effective context, file restoration, stale-preview/concurrent-run refusal and recovery/quarantine. Evidence: R-tests `checkpoints_restore_effective_context_and_preserve_audit`, `file_restore_rejects_stale_preview_and_preserves_original`, `restore_absent_file_quarantines_later_file_and_parent_cannot_use_child_checkpoint`; R-flows restored selected file, preserved unrelated file and refused human edit; R-prompts restore cancellation/denial preserves state.
- [ ] **B14 — Handle large output and long sessions without crashing.**
  - [x] Passing retained tests cover bounded Unicode file/shell output, live answer/tool-result/history/token limits; current terminal has bounded updates per frame and long-text controls.
  - [ ] No retained memory-measurement receipt substantiates the entire long-session/continuous-output acceptance scenario. Bounds tests receive credit; the full stress requirement stays open.
- [ ] **B15 — Make commands and options do what they say.**
  - [x] R-flows records 300 passing checks on each optimized build; R-local records 33 settings/memory/job controls, including failures, state preservation and policy behavior.
  - [ ] Verify that controls and commands needed for the owner's intended tasks behave as described on the current build. The existing counts include repeated polls and screen assertions, so they do not establish an exhaustive public command/option matrix. Such a matrix is not a prerequisite for this personal-use goal.
- [x] **B16 — Make specialist agents and `swarm` perform real work.** Recorded two concurrent saved loops, permission/leash/conflict limits, denial, stop, interruption and actual pack behavior. Evidence: R-tests `two_real_saved_loops_run_concurrently_and_survive_registry_restart`, `scopes_leash_and_file_conflicts_are_enforced_at_dispatch`, `saved_stop_from_another_registry_cancels_live_work`, `denied_team_action_never_becomes_a_successful_member`, `limits_and_unsupported_scope_fail_before_any_model_call`; R-flows actual separate swarm sessions and pack invocation. Unsupported pack scope is explicitly rejected.
- [x] **B17 — Finish isolated workspaces and shell controls.** Recorded real worktree agent read/write/shell/lifetime behavior, pause/close and preservation on failed dirty cleanup. Evidence: R-tests `saved_agent_reads_writes_and_executes_only_in_selected_worktree`, `selected_world_rejects_host_cwd_and_expired_lifetime`, `closed_shell_rejects_work_and_is_not_active`, `failed_cleanup_is_an_error_and_keeps_dirty_files`; R-flows world spawn/list/pause/resume/close and waiting-provider cancellation. Credit covers implemented Mac isolation; unsupported resource/backend promises fail explicitly.
- [ ] **B18 — Connect real browser and mobile control.** The original combined requirement remains unpassed as history. Its mobile portion is excluded from this personal Mac goal; it is not a blocker.
  - [x] Mac browser portion: R-browser records actual controlled-page actions, screenshot, action cancellation and close; all three real Chrome tests pass.
  - **Android/mobile portion: OUT OF SCOPE for this Mac task, never passed.** Preserve the original combined wording as history; no setup or license action is pending from this reconciliation.
- [x] **B19 — Connect external tool servers.** Recorded real stdio discovery/calls/resources/health, invalid input, timeout/disconnect/reconnect and operator permissions. Evidence: R-tests `real_stdio_initializes_discovers_calls_and_reads_resources`, `timeout_disconnect_and_reconnection_do_not_repeat_calls`, `startup_never_runs_before_exact_operator_decisions`, `denial_and_read_only_annotations_never_authorize_external_execution`, `resource_reads_are_exactly_approved_and_timeouts_cannot_auto_restart`; R-flows actual external process/resource/tool results. Unsupported transport is explicitly rejected.
- [x] **B20 — Make memory, saved workflows, and evaluation real.** Recorded durable project isolation/compaction/archive, successful-only workflow capture including restored history, permission checks and real failing evaluation. Evidence: R-tests `durable_restart_compaction_and_archival_preserve_content`, `durable_session_and_project_isolation`, `durable_capture_requires_success_and_survives_restart`, `restored_completed_source_captures_only_effective_success_and_tools`; R-local memory restart/project isolation; R-flows actual check failure output, disabled-shell refusal, checkpoint comparison and saved workflow invocation.
- [x] **B21 — Make background work survive restart and stop on request.** Recorded protected service access, occupied socket/startup failure, once-only recovery, watcher pause/resume/cancel and failure isolation. Evidence: R-tests `service_runs_marker_once_across_restart_and_requires_authentication`, `occupied_socket_and_duplicate_service_are_errors_and_files_are_private`, `once_claim_survives_reopen_and_is_never_replayed_after_success`, `watcher_scan_failure_is_saved_without_stopping_other_jobs`; R-flows service restart did not repeat work; R-local stopped schedules/watchers stay stopped after restart.
- [ ] **B22 — Deliver notifications and bind remote approval to the right action.**
  - [x] Local/controlled destinations, exact-action approve/deny/expiry/duplicates, delivery failure/timeout and explicit acknowledgement have R-tests/R-flows receipts.
  - [ ] If notifications or remote approval are among the owner's intended tasks, verify the owner's authorized destination and the exact-action decision there. Delivery to another human is not required. The previous external-recipient test remains unperformed and is out of scope as a mandatory release gate.
- [ ] **B24 — Make builds and functional tests repeatable.**
  - [x] Retained source copy: 347 tests, three real Chrome tests, two optimized builds and 300 public checks per build. Current refresh: targeted format, strict Clippy, 70 tests and debug build pass.
  - [ ] Keep the runnable Mac artifact tied to the source and behavior actually checked, and use that artifact for the remaining intended-task journey. Current source differs in 13 terminal files from the older optimized package, while the terminal refresh has its own matching debug artifact. A public optimized package, clean machine, other OS builds, and exact byte reproducibility are not personal-use acceptance requirements.
- [ ] **B25 — Make installation and updates work.**
  - [x] R-install records 19 install/update/damaged-update/resume/remove/reinstall checks, exact package bytes and preserved conversations/settings on this account.
  - [ ] Verify normal launch and real-model task/resume on the owner's existing Mac account using the current app. Fresh-account installation and public installer/update/remove certification are out of scope; their historical absence must not block use of this app.
- [ ] **B26 — Give users the instructions and errors they need.**
  - [x] Retained package includes a working guide; current `SECURITY.md` removes the fake reporting address. Its historical open-contact statement is not a requirement for this personal app.
  - [ ] Confirm that the owner can follow normal task, stop, resume and recovery instructions, with useful errors when something fails. A private reporting contact, recruited unfamiliar reader and public documentation certification are out of scope.
- [ ] **B27 — Exercise the complete human workflow.**
  - [x] Controlled automated terminal journeys and an earlier real-model debug read have saved receipts.
  - [ ] Verify the complete intended-task journey on the owner's existing Mac account with the current app and intended real model: plan without changes, approve a small edit, run its check, inspect the result, cancel work, restart and resume without losing context or files. A first-time recruited tester and fresh account are not required. No completion is inferred from test counts.

## B) Evidence ledger and build identity

The saved logs and JSON results below were read in this session. Counts were recomputed from individual result rows; artifacts and source manifests were hashed independently. No suite, model, service, installation or app workflow was run for this reconciliation.

| Label | Exact retained evidence | Observed result |
|---|---|---|
| R-tests | [workspace test log](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/02-workspace-tests.log) | Summed runner results: 347 passed, 0 failed, 3 ignored; receipt records exit 0. |
| R-flows | [first public result rows](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/tmp/chimera-user-flows-n4hujvi3/check-report.json), [second rows](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/tmp/chimera-user-flows-08ydzxw1/check-report.json) | Each has 300/300 `pass: true`, `failure: null`; cleanup rows finished. |
| R-browser | [real Chrome log](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/13-real-browser.log) | `test result: ok. 3 passed; 0 failed`. |
| R-startup | [startup report](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/chimera-startup-cancel-i7nv4ygg/report.json) | 40 passing expected interruption/failure cases. |
| R-prompts | [approval/restore report](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/chimera-prompt-check-iyp6e6_z/report.json) | 11/11 `passed: true`; expected application exit 1 is not a test failure. |
| R-local | [local-control report](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/chimera-local-flows-7q1t578m/report.json) | 33/33 `passed: true`. |
| R-install | [package lifecycle report](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/install-check/report.json) | `pass: true`, 19 result rows; boundary explicitly isolated HOME/controlled model. |
| R-release | [release report](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/verification.json) | Retained source hash matches; both binary hashes and archive hash match. Both build commands recorded exit 0. |
| T-build | [terminal build report](../.floyd/ratatui-refresh/build-report.json), [70-test log](../.floyd/ratatui-refresh/unit-tests.log) | Four commands recorded exit 0; 70 passed, 0 failed. |
| T-flows | [terminal flow report](../.floyd/ratatui-refresh/user-flows-report.json), [runner log](../.floyd/ratatui-refresh/user-flows.log) | 105/105 `pass: true`, `failure: null`, `tui_only: true`. |
| T-lifecycle | [terminal lifecycle report](../.floyd/ratatui-refresh/lifecycle-i7871tsw/check-report.json) | 48/48 passing; linked library hash matches. Earlier failed helper reports remain preserved and are not counted as passes. |
| T-identity | [refresh verification](../.floyd/ratatui-refresh/final-verification.json), [source manifest](../.floyd/ratatui-refresh/tested-source-hashes.json) | All 72 source hashes and all 26 artifact hashes match current files; both debug/verified binaries match. |
| Skill | `/Users/floyd/.codex/skills/chimera-ratatui-expert/SKILL.md` | Present, read, SHA-256 `00ff29669960ddaa2a737cfbd85711e213247c98ff373fb47aa2fdccbf4571dc`; matches T-identity. This credits the saved reusable skill, not a fresh skill evaluation. |

There are two distinct deliverables:

- Retained optimized Mac release: source `78fcdc615e7e281581978825904fa46a041e91aa0766f910fa401a28d2558e94`; binary `8a1a9beed83bae26caf4cc559421fe8acd9a51e801df3c5d1f93ae71b708a1e5`; archive `69a8a28b111e297fe985fa152ff0aef1250f97247c0069035e74df2a27b1f885`. Package path is recorded exactly in R-release. The repeat binary is `06ac752071079030b70c09c32349d9f5418df0d86f7bbf8e8a0d78bf1e37513c`; reported differences are Mach-O UUID/signature metadata. Developer signing and notarization are outside the personal-use goal.
- Current terminal refresh: `/Volumes/Storage/CHIMERA/.floyd/ratatui-refresh/verified/chimera`, SHA-256 `61dc4ceedfbbe7421549fed7c0156fbef5466c04fff179c66db886a63a834b2f`. Current complete source fingerprint is `44abbfbd234769adc2183567a91a038fedb1004ccd25aa20cb6f8dfaea27c682`. Exactly 13 files under `crates/chimera-tui/` differ from R-release's source snapshot; backend, scripts, manifests and lockfile match. This debug artifact is not represented as an updated optimized package.

## C) Reconciliation verification receipts

Read-only Git used `git -c safe.directory=/Volumes/Storage/CHIMERA ...`; global Git settings were not changed. Initial `status --porcelain=v1 -uall`: 43 modified tracked files, 43 untracked files, zero staged files. Branch `main`, HEAD `4cef7422e392831bfe42412328eb5e02ae483978`, two commits ahead of the saved `origin/main` reference; no fetch or fresh remote comparison occurred.

Preservation baseline: 116 existing Git-visible regular files hashed in `/tmp/chimera-reconciliation-baseline-2026-10-12.json`. Original checklist SHA-256: `6c1dd1a15c376cf36b285f8f833e89999c7b13732fd80b0864ef2a6f18b02a1c`.

Final preservation check: 114/116 existing file hashes are unchanged from the initial snapshot. `BLUEPRINT.md` and `IMPLEMENTATION-NOTES.md` changed concurrently between that snapshot and subsequent reads, without writes by this reconciliation. Their current hashes match T-identity (`d13ee66aa8e0d5630d91013a0a28285e3af368bb57497ceab1d1cc952a18a5f7` and `f42d2949859d513bb5c2fa230f23664cf375c2736017ff619353fea67e774744`). Both versions' initial hashes remain in the baseline receipt; current files were left intact. All 72 tested source hashes and all 26 recorded artifacts still match. The original checklist and HEAD are unchanged. The only new Git status entry is `?? Issues/beta-release-reconciliation-2026-10-12.md`; no initial entry disappeared. All 25 IDs are represented once, with 13 checked and 12 open; all 19 Markdown file links resolve.

Current-session hash/result inspection output:

```text
retained release source matches True
lockfile matches True
release binary matches True
repeat binary matches True
archive matches True
public reports: 300/300 each, failure None
workspace totals (347, 0, 3)
REFRESH SOURCE HASHES 72 checked MISMATCHES []
REFRESH ARTIFACT HASHES 26 checked MISMATCHES []
LIFECYCLE 48 passed 48 pass True failure None
```

Actual access failure: read-only `ps -p 54669 -o pid=,ppid=,etime=,stat=,comm=` returned exit 127, `zsh:1: operation not permitted: ps`. The old test-driver PID/session is therefore **not currently verified live**. No stop or cleanup was attempted.

Prior reports about model-load approval rejection, `STATE_LOCK_TIMEOUT`, an external recipient, and driver cleanup remain historical agent reports. This session did not reproduce those service conditions. They are not blockers to marking this list. The inspected `SECURITY.md` directly confirms that the private reporting destination is still unconfirmed; the owner's clarification excludes that contact as a requirement. The retained model/install reports still limit what was actually tested. Protected records, keys and settings were not changed.

## D) Handoff and remaining concrete work

**Acceptance target:** the owner can use the current macOS app to complete the intended work with the intended model, understand progress and errors, control file changes, stop work, and resume safely. No licensing or public-release preparation is part of this target.

The next usability session should use the existing runnable current Mac build for a real intended task and the plan/approve/edit/check/inspect/cancel/restart/resume journey. Verify interrupted file-write recovery, long output/session responsiveness, readable text and the controls needed for that work. Check notifications at the owner's own authorized destination only if that feature belongs to the intended workflow. No mandatory model-provider matrix, external human recipient, private reporting contact, fresh account, fresh machine, public package, signing, notarization or license work remains. Android remains OUT OF SCOPE. This scope correction changes the acceptance criteria; it does not invent new passing results or expand this reconciliation into implementation.

| Requested action | Status | Evidence |
|---|---|---|
| Reconcile and mark actual work | DONE | All 25 original IDs accounted for in section A; exact retained result paths and hash checks in B/C. |
| Preserve original list/history and existing work | DONE | Separate new file; baseline, final preservation comparison and original checklist hash in C. Concurrent document updates were retained. |
| Give a usable handoff | DONE | Build identities and remaining personal Mac usability checks above; broader distribution gates explicitly excluded. |
| Reliable personal Mac use | NOT FULLY VERIFIED | Remaining real intended-task and recovery/control checks; public-release formalities are not blockers. |

No code changes, checkout, stash, deletion, commit, push, merge, production deployment, installation or license acceptance was performed. This new status document is intentionally untracked on the existing dirty `main`; `.git` writes are outside the current permitted editing scope.

Scope correction recorded 2026-10-11 America/Indianapolis: only this handoff was edited. Retained test results and evidence references are unchanged; no further project tests or implementation were performed. The Library copy is updated under the same identity, preserving its earlier version as history.
