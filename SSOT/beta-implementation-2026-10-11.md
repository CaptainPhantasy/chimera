# CHIMERA beta work — 11 October 2026

The latest automated release check passed, and the saved screens and release files were checked. **CHIMERA is not yet verified as beta ready.** Real-model, Android, external-delivery and first-time-user checks remain open. All 25 requirements remain in [the beta list](../Issues/beta-release-todo.md); no boxes have been closed.

The latest terminal fixes make scrolling follow text lines, prevent duplicate errors, show real tool results, apply the theme background, and keep console warnings from corrupting the full-screen display. The package now includes instructions for model setup, normal work, approval, cancellation and recovery.

## Latest release results

[The release evidence](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/verification.json) records both builds, commands, results, logs and package locations. The complete run exited successfully.

| Check | Observed result |
| --- | --- |
| Tests from a fresh source copy | 347 passed; no failures |
| Real Chrome tests, excluded from the ordinary suite | All 3 passed separately |
| Two independent optimized builds | Both passed |
| Public-command checks | 300 passed on each build |
| Startup interruption | 40 passed |
| Approval and restore in a real terminal | 11 passed |
| Local settings, memory and job controls | 33 passed |
| Extracted package: install, update, resume and remove | All 19 passed |

The checks use actual terminal input, file changes, saved context, restore, failing checks, background work and controlled external tools. Model responses and notification destinations come from controlled test services. Counts include repeated job-status polls and can vary; they do not prove complete coverage. Installation uses a separate home folder on the current account. Only this Mac's Apple-silicon build was tested.

Screen review after the previous automated pass [found a warning that damaged the screen](../.floyd/releases/verify-0c054351-6233-4f6f-8bca-cd8c11713961/visual-finding.json); the automated check had missed it. The new check rejects that raw warning output and passes. The lead agent [inspected all 18 saved terminal replays and the actual Chrome screenshot](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/visual-review.json), with no observed layout corruption. These replays do not prove native terminal font appearance: their font shows missing-character boxes for Chinese and emoji. Exact text and the tested character widths were checked; native appearance still needs human review.

The release report records matching executable content across both builds. Their 48 differing bytes belong only to macOS program identifier and signature metadata. Both operating-system signature checks passed. The package has no public developer signature or notarization. Exact byte-for-byte build identity is not claimed.

The lead agent's [final file checks](../.floyd/releases/verify-16cb7100-21b2-427a-bb32-b6584f68536b/root-hash-check.json) confirm that the source, program and archive match the release report. The packaged guide was also read and checked.

## Still required

1. Run the final installed package with the real local Bonsai model and the hosted services included in the beta. [An earlier debug build passed a real model read and streamed response](../.floyd/local-model-test/report.json). The current local-model load was rejected before execution by automatic approval review: “guardian review completed without an assessment payload.” No model or server was started by that attempt. Hosted credentials remain unconfigured.
2. Test an actual Android emulator and small application. No installed Android tools or device were found. The proposed local setup uses `.floyd/android-sdk` and `.floyd/android-avd`, with roughly 1.5 GB of downloads. Google's license still needs the user's acceptance. The question tool failed, so delivery of that question is unverified. Nothing has been downloaded or accepted.
3. Verify delivery to an authorized external recipient and supply a working private security-reporting contact. Controlled destinations and a removed fake address do not satisfy these requirements.
4. Have a person new to CHIMERA follow the packaged guide and complete the installed workflow from a fresh operating-system account.

[Progress for all requirements](../.floyd/beta-implementation-progress.json) retains earlier evidence and failed attempts. These detailed evidence files are local and ignored by Git. No commit, push or publication was made.

An earlier test driver is still reported live: execution session `65553`, process `54669`. Automatic approval review rejected the stop attempt without an assessment. Cleanup is not complete, even though the latest run's own cleanup checks passed.

The required receipt service still returns `STATE_LOCK_TIMEOUT` in the lead agent's check. Its protected records were not edited or reset. Actual results are saved above, but the service cannot currently produce its required final completion report.
