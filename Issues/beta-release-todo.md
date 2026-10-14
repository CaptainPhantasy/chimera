# CHIMERA: finish the code and test it with people

All 25 requirements below remain in scope. Boxes stay open until each full stated test is verified. The latest release run passed 347 code tests, 3 separate Chrome tests and 300 public-command checks on each of two builds. The terminal fixes, saved screen replays, release files and packaged guide were checked. Native Chinese and emoji appearance, real models, Android license acceptance, an external recipient, a private reporting contact, and a new user's trial from a fresh account remain unverified. Cleanup of an earlier test driver is still blocked. This is not a beta-ready claim. See the [current evidence and limits](../SSOT/beta-implementation-2026-10-11.md) and [original findings](../SSOT/beta-readiness-2026-10-11.md). Task numbers are unchanged; removed process items leave gaps.

## Protect the user's work first

- [ ] **B02 — Enforce planning limits and action permissions in code.** Plan mode must reject writes and unrestricted commands. Approval, denial, policy selection, autonomy, and leash controls must affect actual tool execution. Permission for one action must not authorize another.
  **Test:** force write attempts during planning; verify no files change. Approve and deny individual actions in both command-line and interactive use. Changed or expired approvals must fail.

- [ ] **B03 — Keep tools inside their allowed boundaries.** Check absolute paths, parent paths, symbolic links, command working directories, protected files, network access, and environment access. Connect execution to real isolation where promised.
  **Test:** try each escape route against disposable files outside the allowed folder. Those files must remain unread and unchanged while permitted project work succeeds.

- [ ] **B04 — Preserve edits and keep private data private.** Detect files changed by the user before overwriting them. Keep recoverable originals and use quarantine for removals. Protect saved settings and sessions. Keep `.chimera/` and secret environment files out of ordinary commits; remove secrets from exported diagnostics.
  **Test:** edit a file while the agent is working; neither version may be silently lost. Interrupt a write and recover the file. Use fake secret markers to check saved-file access and exports. Check that local session files do not appear as files to commit.

- [ ] **B05 — Make cancellation and timeouts stop the work.** Stop waiting model requests, running tools, and their child processes. Limit request waits and retries. Do not repeat file changes or other side effects on retry. Save interrupted progress and restore terminal settings, including after startup errors.
  **Test:** cancel a stalled model and a command with a child process. Neither may keep working afterward. A delayed test file must not appear after timeout. Exit and error paths must leave the terminal usable.

- [ ] **B06 — Report the real outcome.** A failed connection, rejected request, empty or cut-off answer, exhausted limit, missing tool, or unfinished task must not report successful completion. Preserve the useful error message.
  **Test:** exercise each failure through the actual program and check both the visible message and the result returned to the calling program. Repair the iteration-limit test so it runs repeated tool calls and proves the exact limit, including zero and one, instead of passing on an unrelated provider error.

## Make the main user experience work

- [ ] **B07 — Connect the interactive screen to the agent.** Both `tui` and `run --interactive` must load the configured model, submit the starting request once, accept follow-up messages, show real tool activity, and save the conversation.
  **Test:** complete a small file-editing task through each entry point. Ask a follow-up that relies on the earlier conversation. Confirm the displayed model, tool results, and saved conversation are real.

- [ ] **B08 — Fix typing, navigation, and exit.** Make `/quit`, `/exit`, advertised commands, and tool-detail controls work. Support ordinary question marks, paste, multiline input, history, long text, and international characters. Show the actual objective and session state. Explain unknown commands.
  **Test:** use every advertised control in a real terminal. Resize between 80×24, 100×30, and 160×45 cells. Check long input, long errors, dark/light display, and text clipping. Exit must stop owned work and restore the terminal.

- [ ] **B09 — Show model output as it arrives and handle broken responses.** Stop buffering the whole answer before displaying it. Handle characters and tool arguments split across network fragments. Detect malformed responses, provider errors, early disconnections, and missing or cut-off completion. Show accurate usage or say it is unavailable.
  **Test:** pause a controlled response between two parts; the first must appear before the second is sent. Split international characters and tool arguments across parts. Broken or unfinished responses must not become successful answers.

- [ ] **B10 — Make configuration and model connections reliable.** Reject unreadable or broken settings, invalid addresses and limits, and missing model names before sending a request. Do not silently switch to defaults or another service. Parse local addresses correctly. Make provider selection and diagnostics actually work.
  **Test:** try bad settings, bad credentials, an unavailable model, and a dropped connection. Then use a real local model and each advertised hosted service to complete text, tool, and streaming requests. Verify which settings take precedence.

- [ ] **B11 — Save runs and restore their context.** Save messages, tool calls/results, permissions, errors, and the run outcome under one session identifier. Connect list, resume, fork, replay, and export to those records. Fork means starting a separate conversation from an existing one.
  **Test:** run a task, close the program, reopen it, and continue from the right context. Fork without changing the original. Replay must not execute tools again. Export must be readable by another program. Success, failure, cancellation, and interruption must stay distinct after restart.

- [ ] **B12 — Prevent lost or misleading saved history.** Save sessions without overwriting the only recoverable copy mid-write. Prevent simultaneous writers from losing events. Load older saved formats safely and explain damaged files. Fix history that disappears when a reopened trace receives new events; do not claim an event was saved when storage failed.
  **Test:** interrupt writes; write from two instances; save event A, reopen, save B, and read both. Simulate full or unwritable storage. Old data must remain recoverable and failures must be visible.

- [ ] **B13 — Make checkpoints restore real state.** A checkpoint is a saved state the user can return to. Store the conversation, task, and file state the command promises. Restore it without overwriting unrelated or later user edits; preserve the history of restoration.
  **Test:** save a checkpoint, make changes, restart, and restore in a disposable project. Verify the intended state returns, unrelated edits survive, and a failed restore leaves recoverable files.

- [ ] **B14 — Handle large output and long sessions without crashing.** Limit file reads, command output, retained messages, and in-memory history while collecting them. Cut text at valid character boundaries. Respect time, iteration, and usage limits.
  **Test:** use large files, continuous command output, emoji and accented text at size boundaries, and long conversations. Memory must stay bounded. The program must remain responsive or stop with a clear explanation and saved progress.

## Finish the remaining features

- [ ] **B15 — Make commands and options do what they say.** Complete initialization, diagnostics, status, tool checks, policy selection, and structured output. Keep machine-readable output separate from human messages. Replace success-looking placeholders with working behavior; unfinished commands must say they are unavailable until implemented.
  **Test:** invoke every public command and option. Check its actual state change, output, and success/failure result. A missing tool must fail. The selected policy must change what execution permits.

- [ ] **B16 — Make specialist agents and `swarm` perform real work.** Replace fixed tasks and invented completion with actual assignment, execution, checks, and results. Implement agent start, inspect, stop, permission changes, and pack execution. Handle conflicting edits and worker failures.
  **Test:** two agents work on a controlled objective. Inspect their actual activity, deny one an action, stop one, and make one fail. No forbidden action, lost edit, or invented success may result. A pack must apply its configured limits.

- [ ] **B17 — Finish isolated workspaces and shell controls.** Connect tasks to real execution environments with their intended network/resource limits. Track running shells, reject work after close, and report failed cleanup. Preserve uncommitted files when cleaning up a separate working copy.
  **Test:** create an environment, run work, inspect it, close it, and clean it up. A closed shell must reject commands. Failed cleanup must preserve files and explain the problem. Repeat through an agent, not only a library call.

- [ ] **B18 — Connect real browser and mobile control.** Replace fake screenshots and acknowledgements with actual browser and supported device/emulator actions. Track owned sessions and devices, report unavailable dependencies, and clean up on exit.
  **Test:** open and interact with a controlled web page and save a real screenshot. Boot a device, open an app, perform a permitted action, save actual evidence, and shut it down. Exercise cancellation and failures.

- [ ] **B19 — Connect external tool servers.** Replace the simulated client with real tool discovery, calls, resource reading, connection checks, and reconnect handling. Apply the same action permissions as built-in tools.
  **Test:** call a tool on a controlled server and inspect its real result. Try invalid input, disconnection, timeout, and revoked permission. Tool descriptions must not grant extra access.

- [ ] **B20 — Make memory, saved workflows, and evaluation real.** Preserve intended long-term memory and captured workflows across restart. Keep projects isolated. Reducing memory size must preserve needed information. Run actual checks against produced work and compare actual checkpoints; missing checks must not earn a passing score.
  **Test:** restart and reuse saved information and a workflow. A different project must not retrieve private records. Saved workflows must obey current permissions. A deliberately failing check must produce a failing evaluation with its real output.

- [ ] **B21 — Make background work survive restart and stop on request.** Implement service start/stop/status, protected control access, schedules, watchers, restart recovery, and cancellation. Prevent duplicate runs.
  **Test:** schedule a harmless task, restart the service, and verify it runs once. Pause, resume, and cancel it. Stopped watchers must remain stopped. Check startup failure, occupied ports, unauthorized control requests, and accurate status.

- [ ] **B22 — Deliver notifications and bind remote approval to the right action.** Replace simulated delivery with a real authorized destination and actual delivery results. Handle expired or duplicate replies and failed delivery. Only the correct decision may release the exact pending action once.
  **Test:** deliver to an authorized test recipient. Check approval, denial, expiry, duplicates, and failed delivery. An undelivered message must not be recorded as acknowledged.

## Check the program people will actually use

- [ ] **B24 — Make builds and functional tests repeatable.** Build from a fresh checkout with fixed dependency versions. Run the existing tests plus the behavior checks above automatically. Test the optimized release build on each operating system offered to users. Fix failures that affect behavior.
  **Test:** a clean machine can build and run the checks using the supplied command. A deliberately broken user behavior must make the checks fail. No style-only cleanup or coverage percentage is a completion requirement here.

- [ ] **B25 — Make installation and updates work.** Produce an installable program. Use the same version in the executable and screen so users can identify it. Support updating and removing the program without losing saved conversations or user files.
  **Test:** install the packaged program on a fresh user account, configure a model, and complete a task. Update it and resume an old session. Remove the program and verify user data remains recoverable.

- [ ] **B26 — Give users the instructions and errors they need.** Keep a short working guide and accurate built-in help for installation, model setup, normal use, stopping work, recovery, and reporting problems. Explain stored data and known limitations. Replace the fake security-reporting address with a working private contact.
  **Test:** someone unfamiliar with the project can follow the guide without developer coaching. Every shown command works; errors say what went wrong and how to recover. The reporting destination is real. Internal document housekeeping is not part of this task.

- [ ] **B27 — Exercise the complete human workflow.** Use the installed release on a disposable project: configure a real model, plan without changes, approve a small edit, run its check, inspect the result, cancel another task, restart, resume, and export diagnostics without secrets. Exercise the other completed features from their public controls.
  **Test:** run this journey with a first-time user. Include a bad setting, interrupted connection, long session, and failed command. Fix any lost work, unsafe action, misleading success, broken control, or confusing recovery that prevents the person from finishing.

Start with B02-B06. Build storage and model handling before connecting the full interactive flow. Add each task's test with its fix. Finish by testing the installed program, not just its individual pieces.
