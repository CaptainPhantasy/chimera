# CHIMERA beta readiness review

Reviewed on 2026-10-11, America/Indianapolis.

**Verdict: not ready for beta.** The source builds and all 199 existing tests pass. Several central promises still fail or are not connected: enforced permissions, an interactive agent, truthful completion, saved runs, and the wider orchestration features. This review and its companion to-do list are complete; the product work is not.

Requested work: “analyze this project and create the to do list to complete it to the beta release standard.”

The actionable list is [beta-release-todo.md](../Issues/beta-release-todo.md). It preserves the advertised feature scope. Features can move beyond beta only through an explicit release decision, with matching help and documentation. This review does not silently reduce the project to a smaller product.

## What was inspected

- Folder: `/Volumes/Storage/CHIMERA`.
- Configured repository: `https://github.com/CaptainPhantasy/chimera.git`. Read from the local Git configuration; the remote server was not queried.
- Branch: `main`. Local Git reports two commits ahead of its saved `origin/main` reference. That is not a fresh comparison with GitHub.
- Commit: `4cef7422e392831bfe42412328eb5e02ae483978`.
- Existing edits in `FLOYD.md`, `Issues/CHIMERA_ISSUES.md`, and `SSOT/CHIMERA_SSOT.md` were preserved. Source files and dependency files were unchanged by this review.
- All 18 workspace packages were covered through source review, the full existing test suite, and compilation. Checks used macOS on Apple silicon, Cargo 1.98.0 and Rust 1.98.0, from `/Users/floyd/.cargo/bin/`.
- The source fingerprint is `176e640c73487b7453c6fe9a7c4e00c005bd2b14cacf0ee29b46837241635568`. It is SHA-256 of sorted JSON mapping each workspace/package manifest, Cargo.lock, and Rust source path to its SHA-256. It excludes generated output and these review documents.
- The freshly built executable was `/tmp/chimera-beta-target/debug/chimera`, SHA-256 `33bcc94546b653bfa7f9b8e0270d1c70719d66a53a5f1da855eaef9374b369de`. The older executable under the repository's `target/` was not used as proof.

Git initially rejected the folder's ownership. Read-only Git commands used a command-specific `safe.directory` setting; global Git configuration was not changed. Rust was outside the ordinary command search path and was found at the explicit path above.

## Beta standard used for this review

This is a proposed project release bar, not a claimed external certification. A beta tester must be able to install the named release, configure a supported model, inspect and perform an authorized task, see progress, stop it, recover saved work, and report a problem. The program must protect work outside its granted scope and must distinguish success, failure, cancellation, and unfinished work. Every feature advertised for that beta must work through its public command or screen.

Cosmetic polish can remain imperfect. Missing protections, fake success, data loss, disconnected core workflows, and unsupported features presented as working cannot. No release percentage or delivery date is assigned: the inspected project has no reliable basis for either.

## Checks run in this review

Commands ran from `/Volumes/Storage/CHIMERA`. Cargo checks used `PATH=/Users/floyd/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin` and `CARGO_TARGET_DIR=/tmp/chimera-beta-target`. Dependency versions were kept locked and cached dependencies were used without downloads. The tests set `CHIMERA_CONFIG=/tmp/chimera-beta-no-config.json` to avoid loading the user's configuration.

| Check | Current result | What it proves |
|---|---|---|
| `cargo metadata --offline --locked --no-deps --format-version 1` | Passed | The workspace manifest can be read. |
| `cargo test --workspace --locked --offline -- --test-threads=1` | Passed: 199 tests, zero failures, zero ignored | The existing tests pass in this environment. Many exercise simulated components, parsing, and rendering helpers. |
| `cargo build --workspace --locked --offline` | Passed | The current workspace produces a development build. |
| `cargo clippy --workspace --all-targets --locked --offline` | Exit 0 with seven warning diagnostics | The code checker completed; this was not a warning-free result. |
| `cargo fmt --all -- --check` | Failed, exit 1; differences in 30 source files | The source does not meet its configured formatting rules. No automatic formatting was applied. |
| Fresh binary `--version` | `chimera 0.1.0`, exit 0 | Actual command-line version. |
| Fresh binary `doctor` | `[stub] chimera doctor`, exit 0 | Diagnostics are a placeholder. A stub is code that stands in for unfinished behavior. |
| Fresh binary `--format json status` | `[stub] chimera status`, exit 0 | The requested machine-readable output is not honored. |
| Fresh binary `tool healthcheck does-not-exist` | `does-not-exist: missing`, exit 0 | Missing tools still yield a successful command exit. |
| Fresh binary `run "read-only connection check"` with empty keys and `CHIMERA_BASE_URL=http://127.0.0.1:1/v1` | Network error, followed by `run complete`, `done: Error`, exit 0 | A failed model connection is reported to calling programs as success. No successful model call occurred. |
| Fresh binary `run --interactive "read-only beta check"`, 100-column by 30-row terminal | Shows `model-unset`; submitting `hello` shows `No agent backend connected`; `/quit` leaves the process alive; Control+C exits 0 | The interactive path is disconnected and its slash exit command does not exit the screen. |

The terminal check captured program output and process behavior. It was not a complete visual inspection across terminal sizes. Build and test results are separate from the failed product behavior above.

## Findings and their evidence

The locations below refer to the inspected source. Except where a live command is named above, these are findings from reading the source, not claims that an exploit or crash was reproduced.

| Finding | Evidence in this checkout | Why beta is affected | To-do items |
|---|---|---|---|
| Planning restrictions and approvals are not enforced in the real execution path. | `crates/chimera-cli/src/main.rs:509-527` offers the same tools and only changes the model's instructions. `crates/chimera-tools/src/lib.rs:174-180` dispatches directly. Approval and policy commands are placeholders at `main.rs:788` onward. | A model request can reach a write or command runner without the advertised permission check. | B02, B03 |
| File paths and shell execution are not confined to approved locations. | `chimera-tools/src/lib.rs:197-206` accepts absolute paths and parent traversal. Writes at `:263-269` directly overwrite files. Shell execution at `:358-367` runs the requested command. `chimera-sandbox` defines an interface but has no real isolation implementation. | An incorrect tool request can affect files outside the intended project. Folder joining is not access control. | B03, B04 |
| Timeout paths do not explicitly terminate child work. | `chimera-tools/src/lib.rs:365-383`; `chimera-shell/src/lib.rs:212` onward. | Reporting a timeout may leave work running. This consequence needs a controlled reproduction; no surviving child was claimed here. | B05 |
| Failure and unfinished work can look successful. | `chimera-core/src/agent.rs:247-254,273-279,330-337`; `chimera-cli/src/main.rs:570-579`. Reproduced through the fresh binary. | Scripts and users can trust an outcome that did not happen. Empty or cut-off responses also need explicit handling. | B06 |
| The interactive screen does not start the agent. | `chimera-cli/src/main.rs:724-735`; `chimera-tui/src/runner.rs:344-366,389-411`. Reproduced through `run --interactive`. | The principal interactive workflow cannot perform work. | B07 |
| Several terminal controls and displayed values are unfinished. | `/quit` breaks the inner loop at `runner.rs:393-395`; advertised controls in `help.rs:46-73` are absent from the active runner. `chat.rs:77` hardcodes `v0.2.0`; `:91-93` shows the mode as the objective. | Exit is unreliable, help misleads, and users cannot identify the actual run reliably. | B08, B25 |
| Streaming is buffered until the entire response ends. | `chimera-llm/src/lib.rs:335-462` returns a completed list; `chimera-core/src/agent.rs:196-206` then calls display callbacks. | The claimed live progress is delayed until the answer finishes. | B09 |
| Damaged model responses and configuration errors can be hidden. | `chimera-llm/src/lib.rs:372-395,460-462` silently skips malformed data and marks the closed response done; decoding individual network fragments can damage split characters. `chimera-config/src/lib.rs:104-137` discards file errors and falls back to defaults. | The wrong configuration can be used silently, and incomplete answers can be treated as complete. | B09, B10 |
| Runs do not connect to the real session and trace stores. | `chimera-cli/src/main.rs:494-579` does not create either store. Listing/fork/export helpers at `:599-703` use them independently. | Having storage code does not provide a recoverable run or a saved evidence trail. | B11 |
| Storage has interruption, concurrency, and trace-history risks. | `chimera-session/src/lib.rs:128-134,167-179` rewrites the sole file and uses unlocked read/change/write operations; snapshots record identifiers only. `chimera-trace/src/lib.rs:121-155` records in memory before disk and returns only the new buffer after reopen-and-append. | Interrupted writes can damage saved state; overlapping writes can lose events; trace queries can omit prior history or present unsaved events. These failure cases were not runtime-tested. | B12, B13 |
| Large or international text can crash or consume unbounded memory. | `chimera-tools/src/lib.rs:225-232,365-387,412-415` reads/captures all output before limiting it and truncates at arbitrary byte positions. | Cutting a multi-byte character can panic; a displayed output limit does not bound memory. No panic was triggered during this review. | B14 |
| Many commands and integrations remain simulated. | `chimera-cli/src/main.rs:710-834`; `chimera-core/src/lib.rs:259,340,371,385` constructs fixed tasks and successful results; mock implementations in browser/mobile/comms/eval/mcp and scheduler paths. | Advertised platform behavior is not achieved by the single-agent loop or by passing simulated-component tests. | B15-B22 |
| Local records can accidentally enter a commit. | `.gitignore` excludes `.env`, but not `.chimera/`; session and trace structures hold objectives, tool arguments, and metadata. | Future connected runs need deliberate retention, access, and export protections. | B04 |
| Release evidence and project guidance are stale. | Workspace version is `0.1.0`; terminal header says `0.2.0`; changelog leads with `0.3.0`. `FLOYD.md` and project state contain template fields. `SECURITY.md:4` gives a placeholder address. The file inventory has no build/release workflow or installer. | A new tester cannot rely on the claimed version, supported features, or setup and reporting paths. | B23-B27 |

In this table, package-shortened paths such as `chimera-tools/src/lib.rs` mean `crates/chimera-tools/src/lib.rs` under the project root.

## What is real and worth retaining

The working foundation includes an actual model request client, a tool-calling loop, seven built-in file/search/command tools, file-backed session and trace implementations, a process runner, built-in specialist profiles, and terminal rendering/input components. Existing tests cover useful basic behavior. These pieces need connection and hardening, not an unrequested wholesale rewrite.

The separate `Harness::run_objective` path is still simulated. The ordinary command-line `run` uses the real agent loop instead. The review does not combine those two paths into a claim that the full platform works.

## What remains unverified

- No successful live model conversation or paid provider request was made. Listed provider brands are not a verified compatibility matrix.
- Permission escapes, timeout survivors, large-output crashes, interrupted storage writes, and overlapping writers were not executed. Their source evidence and required acceptance checks are recorded in the to-do list.
- No real browser, mobile device, external tool server, notification delivery, daemon, scheduled job, or multi-agent run was exercised.
- No optimized release build, installer, fresh-machine installation, Linux/Windows build, or dependency advisory scan was completed.
- Test coverage was not regenerated. Historical “100% behavioral coverage” and completion-percentage claims were not accepted as current evidence.
- No source fixes, commits, pushes, deployment, or publication were performed. This task delivered the review and release to-do list only.

## Review conclusion

Do B02-B06 before allowing a beta agent to change real user work. Connect the terminal and saved-run paths next. Complete the promised platform features, or explicitly decide and document their later release, before packaging. Then run the final user-flow checks against the exact release executable. Passing today's 199 tests is useful evidence of the foundation, but it does not remove the blockers above.
