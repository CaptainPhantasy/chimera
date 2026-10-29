# CHIMERA terminal refresh — implementation notes

Completed work dated 2026-10-11, America/Indianapolis. The user's instruction was **Both**: save the supplied guide as a reusable skill and apply it to the current terminal screen.

## Delivered

The reusable skill is installed at `/Users/floyd/.codex/skills/chimera-ratatui-expert/SKILL.md`, with its `agents/openai.yaml`. The bundled skill validator reports `Skill is valid!`. The guide keeps documentation → knowledgebase → patterns → blueprint → implementation → verification and includes the version and terminal-ownership corrections found during this work.

[Knowledgebase](knowledgebase/INDEX.md) contains 11 original topic/pattern notes, source evidence, dates, exact versions, URLs, source hashes, and note hashes. [BLUEPRINT.md](BLUEPRINT.md) was written and independently reviewed before source changes. Review corrections were recorded before implementation. The matching cached published crate source was checked against its archive and Cargo.lock where a documentation page could not be retrieved.

CHIMERA remains Rust with Ratatui 0.29.0 and Crossterm 0.28.1. Tokio remains 1.50.0 and anyhow 1.0.102. This task made no changes to Cargo manifests or Cargo.lock. The existing application version remains 0.1.0.

## Changes

The actual saved chat runner now opens with useful guidance, keeps input visible in small windows, fits status groups to available width, and shows keyboard hints. `Ready` means the application is idle; it does not claim that a model is connected. Unavailable usage and cost remain labelled unavailable. Both existing themes are preserved.

Conversation scrolling and rendering share the same rectangle, including its width. All allocated conversation rows are used. Help wraps on narrow screens, scrolls with arrow and page keys, and retains its close hint. Input handles multiline cursor width, command-history hints, and a one-row terminal. `q` remains chat text; Escape closes help; Ctrl+C stops work and leaves.

Terminal setup installs an owner-aware panic hook once and preserves the previous reporter. The real runner and test probe share `TerminalGuard`, which cannot move between threads. Cleanup restores terminal modes, alternate screen, paste handling, and cursor before reporting an owner-thread panic. Worker and later non-terminal panics do not tear down the active owner's terminal. Update processing is bounded to 128 events per frame; completion is checked after the producer finishes and its queued updates are applied.

Behavior changes are in `crates/chimera-tui/src/chat.rs`, `status.rs`, `input.rs`, `help.rs`, and `runner.rs`. Formatting was limited to the terminal package. Other touched files in that package were checked against the saved starting copy after normalizing both with the same formatter; those changes are formatting only. Unrelated workspace changes were preserved. No commit, push, deployment, or normal-shell installation was performed.

## Verified build and checks

Final executable: `/Volumes/Storage/CHIMERA/.floyd/ratatui-refresh/verified/chimera`.

SHA-256: `61dc4ceedfbbe7421549fed7c0156fbef5466c04fff179c66db886a63a834b2f`.

The saved executable is a byte-identical copy of the tested `.floyd/ratatui-refresh/target/debug/chimera`. Source hashes are recorded in [tested-source-hashes.json](.floyd/ratatui-refresh/tested-source-hashes.json). The starting terminal files remain under `.floyd/ratatui-refresh/before/`.

| Check | Result | Evidence |
| --- | --- | --- |
| `cargo fmt --package chimera-tui --check` | Pass | [format log](.floyd/ratatui-refresh/format.log) |
| `cargo clippy --locked --offline -p chimera-tui --all-targets --no-deps -- -D warnings` | Pass | [Clippy log](.floyd/ratatui-refresh/clippy.log) |
| `cargo test --locked --offline -p chimera-tui` | 70 passed; 0 failed | [test log](.floyd/ratatui-refresh/unit-tests.log) |
| `cargo build --locked --offline -p chimera-cli` | Pass | [build report](.floyd/ratatui-refresh/build-report.json) |
| Actual executable, `scripts/check-user-flows.py --tui-only` | 105 checks passed | [flow report](.floyd/ratatui-refresh/user-flows-report.json) |
| Actual terminal modes, error/panic ordering, ownership, narrow help/paste/resize/exit | 48 checks passed | [cleanup report](.floyd/ratatui-refresh/lifecycle-i7871tsw/check-report.json) |

The tests used isolated folders and a controlled local model service. Approval tests performed their allowed writes only inside the disposable test folder. No hosted model request was made. The cleanup probe used the actual final `chimera_tui` library; it did not replace the production cleanup code with a mock.

The live read-only preview at `http://127.0.0.1:18762/` launches the identified saved executable. [Preview identity](.floyd/ratatui-refresh/preview-launch.json) records its process, hash, and intentionally unconfigured model. Launching without a request makes no model call. Use the saved executable with `tui` in a normal terminal for keyboard input; the browser preview intentionally does not accept input.

Visual inspection covered the live preview at 1440×900 and 390×844 pixels, and actual saved terminal output for light/dark chat, full and narrow help, long input, tool details, long answers, and errors. Screens are in [.floyd/ratatui-refresh/screens/](.floyd/ratatui-refresh/screens/), plus [desktop](.floyd/ratatui-refresh/desktop-1440x900.jpg) and [narrow](.floyd/ratatui-refresh/mobile-390x844.jpg) captures. The terminal tests also exercised 40×12, 66×15, 80×24, 100×30, and 160×45 cells. Automated cell checks include tiny input and cursor bounds.

Command suggestions were also captured from the final executable at 40×12, 66×15, and 80×24 cells in both themes. Visual review checked the narrow dark screen, compact dark menu, and full light menu: the smallest screen deliberately hides suggestions to preserve the message box; the other two show three and five complete choices. All three sessions exited cleanly and restored their terminal modes. [Capture report](.floyd/ratatui-refresh/suggestions-r_n8cqcf/capture-report.json).

## Corrections and limits

The baseline strict code check found four terminal-package warnings; all were fixed without suppression. The first source review also caught the one-row input title, compact welcome wrapping, and completion-check ordering; each was corrected before final tests.

Early cleanup-helper attempts revealed macOS terminal-descriptor behavior and differential terminal output. The helper now checks the master terminal descriptor after exit and forces a redraw before checking complete help text. Those failed helper reports are retained beside the final passing report. They were not treated as evidence of application failure.

The saved-output viewer originally used old Unicode width rules and rendered emoji incorrectly. It now uses the official `@xterm/addon-unicode11` 0.8.0 package, downloaded into `.floyd/ratatui-refresh/viewer-deps/` and checked against its registry SHA-512 integrity. This is a preview-tool dependency only; the Rust application dependencies did not change. Screens were checked again after that correction. The live preview also clears its inherited `NO_COLOR` setting so the application theme is visible. A browser connection error recorded during the deliberate preview restart was a temporary wrapper disconnect.

The supplied guide was adapted to this existing workspace: no `cargo init`, no dependency upgrade, no unsupported `ratatui::run`, no global `q` quit binding, and no unused mouse capture. The existing explicit event loop was retained. Context7 results were checked against exact-version documentation because some returned examples belonged to newer versions.

There is no unresolved documentation gap for the interfaces changed here. A live model account, physical keyboard behavior in every native terminal, other operating systems, forced-kill recovery, full-workspace diagnostics, and overall beta readiness were not tested by this task. The controlled test service and browser wrapper do not prove those results. The knowledgebase's source-level lifecycle note predates the final runtime tests; the passing runtime evidence is linked above.

The [final evidence check](.floyd/ratatui-refresh/final-verification.json) ties the passing reports to the unchanged final source, executable, library, documents, and saved screenshots. The blueprint retains its original review state as history and identifies the review that was completed before implementation.

## 2026-10-14: Guided provider setup

Feature 1 of the ten requested workflow changes: `/connect`, `/setup`, and F2 open the provider chooser. Presets supply service addresses; custom services use the same form. Key input is masked, stays out of conversation/history, and is saved with owner-only permissions. A cancellable background check reads the service's models, handles rejected keys and unavailable/empty services, and saves the chosen connection and model atomically while preserving previous settings. Existing settings, environment overrides, chat layout and exact-action approvals remain supported. No dependency versions changed; existing reqwest/uuid workspace packages are reused.

Verification: 363 workspace tests, 84 targeted config/interface tests, strict targeted Clippy, package formatting, 18 actual-terminal provider checks and 78 existing setup/reload checks passed. These use isolated local services; no paid model or hosted account was called. Rendered terminal output was inspected for masking and layout. Temporary harness observations are retained: differential screen output needed terminal-cell parsing, and the test must drain output while waiting between Escape and the next command.

Evidence: `.floyd/workflow-checks/` and retained workflow reports under the operating-system temporary directory. Final feature verification receipts identify the source and executable.


## 2026-10-29: Developer workspace UX — twenty corrections

Scope: the personal macOS developer application. Android toolchains, licensing, distribution agreements and public-release formalities are out of scope. This checkpoint addresses the twenty UI/UX findings; it does not declare the whole application beta-ready.

Each checked item below has one direct acceptance test, named `ux_NN`, that states what is being tested, why a developer needs it and an observable pass/fail result. These tests drive real input/state/rendering and isolated local file/Git fixtures. They do not establish hosted-model reliability or physical keyboard behavior in every terminal. Only these twenty cases were executed for this change; existing broad suites were not run.

| Verified | User-visible result | Direct case |
| --- | --- | --- |
| [x] 01 | `/plan` persists for ordinary follow-ups; `/run` explicitly selects execution | `ux_01` |
| [x] 02 | Ctrl+C requests cancellation while keeping the workspace open | `ux_02` |
| [x] 03 | Approval shows the actual file change with Deny selected | `ux_03` |
| [x] 04 | `/diff` reviews staged and unstaged patches and lists untracked files without changing Git | `ux_04` |
| [x] 05 | Busy-chat follow-ups queue in order, retain PLAN/RUN intent and can return safely to the composer | `ux_05` |
| [x] 06 | `/jobs` is accessible while foreground chat runs | `ux_06` |
| [x] 07 | `/tools-view` selects earlier tool results from the current transcript page | `ux_07` |
| [x] 08 | Full tool inspection scrolls beyond the compact 400-line patch preview | `ux_08` |
| [x] 09 | Conversation search finds matching conversation output | `ux_09` |
| [x] 10 | `/load-older` retrieves evicted screen output | `ux_10` |
| [x] 11 | `/new` starts fresh model context while preserving saved conversation and unsent draft | `ux_11` |
| [x] 12 | Saved conversations sort by recent activity | `ux_12` |
| [x] 13 | Session search finds later conversation text beyond the opening preview | `ux_13` |
| [x] 14 | Keep is the archive confirmation default; archived conversations can be restored | `ux_14` |
| [x] 15 | Typing `d` in session search searches; Delete is a separate archive action | `ux_15` |
| [x] 16 | Input history restores the unsent draft and insertion point | `ux_16` |
| [x] 17 | Up/Down move within multiline drafts before reaching history boundaries | `ux_17` |
| [x] 18 | Ctrl+A/E/K/U/W edit the current line | `ux_18` |
| [x] 19 | The header identifies the current project and Git branch | `ux_19` |
| [x] 20 | Context shows approximate usage and a known limit, or explicitly unknown capacity | `ux_20` |

Ancillary controls were source-reviewed, rather than separately executed: Ctrl+Q exits, `/approve` reopens the pending approval review, Ctrl+F opens search, `/latest` returns to live output and Ctrl+G launches the external editor. The external editor process was not exercised.

Context capacity is a per-chat setting: `/context N` uses a limit supplied by the user; `/context unknown` clears it. Usage is a byte-based estimate, not a tokenizer measurement, and tool schemas/project facts may add context. The interface labels this limitation. Scrollback is private, bounded to 128 MiB per screen; storage failures are visible. Saved sessions remain the durable conversation source. Archiving the currently open session is refused; switch conversations or use `/new` first. Cancellation pauses queued work until explicitly resumed.

Independent Git Steward review found and corrected approval-shortcut bypass, queued mode loss, history loss on archive failure, incorrect historical-page mouse scrolling, current-session archive failure and stale resumed status. No dependency versions changed. A pre-existing unused CLI import was removed so strict workspace compilation succeeds.

Local publication checks passed: exact twenty acceptance cases (20 passed, zero failed), terminal-package formatting, workspace/all-target strict Clippy and the actual CLI application build. Staged Gitleaks scanned 106,183 bytes with no leaks; whitespace and file hygiene checks passed. The first Unicode draft case exposed a test-screen observer error: the observer was corrected to skip wide-glyph continuation cells, retaining the original draft/caret assertion. Its rerun and the final twenty-case run passed.

Commands used (build/static checks do not execute extra test cases):

```sh
cargo test --locked --offline -p chimera-tui --lib ux_ -j 2 -- --test-threads=1
cargo fmt --package chimera-tui -- --check
cargo clippy --workspace --all-targets --locked --offline -j 2 -- -D warnings
cargo build --locked --offline -p chimera-cli -j 2
```

Implementation checkpoint: `b6196d82add8c3dd1dc0e6d7836bd5abb61a3715`, branch `feat/developer-workspace-ux`. macOS remote validation: [implementation run](https://github.com/CaptainPhantasy/chimera/actions/runs/40916010011). The [branch workflow history](https://github.com/CaptainPhantasy/chimera/actions/workflows/developer-ux.yml?query=branch%3Afeat%2Fdeveloper-workspace-ux) identifies each subsequent documentation checkpoint by its exact SHA. The workflow runs the same twenty acceptance cases, formatting, strict compilation and application build. A run link alone does not assert success: confirm green on the final pushed revision before treating publication as finished.
