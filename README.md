# CHIMERA

CHIMERA is a terminal assistant for reading and changing a project with a configured language model. Conversations are saved in the project. File changes and commands need approval for the exact action.

This is a beta candidate under active testing. The remaining work and real-user checks are in [the beta list](Issues/beta-release-todo.md). Controlled test services do not prove a hosted model or a human workflow works.

## Build and start

The current supported execution system is macOS. Other systems must reject unavailable isolation features; they are not claimed as tested releases. Building requires Rust 1.98 or later and the dependencies in `Cargo.lock`. Chrome control also needs Node.js, Chrome and Puppeteer. Android control needs an installed Android development kit and an existing emulator profile.

```sh
cargo build --locked --release -p chimera-cli
./target/release/chimera --version
./target/release/chimera --help
```

Run from the project you want the assistant to work on. In the examples below, `chimera` means the installed executable or its full path.

## Build, package and install checks

```sh
python3 scripts/check-beta.py --repeat-build
python3 scripts/package-release.py --binary /absolute/path/to/release/chimera --output-dir /absolute/path/to/packages
python3 scripts/check-local-flows.py /absolute/path/to/installed/chimera
```

The first command uses a fresh source copy, fixed dependencies, an optimized build, controlled user checks and an isolated installation check. It stores its output under `.floyd/releases/`. It needs the dependency cache for an offline build. Its tests do not call a hosted model or replace a first-time human trial. Use the installer included in the resulting package; its update and removal paths preserve saved user data.

The last command checks saved memory, job controls, settings preservation and disabled notifications through the installed program. It needs no model or local server. It preserves its temporary projects and prints the report path. It does not test background execution or message delivery.

## Set up a model

`chimera init` creates `~/.chimera/config.json` if it does not exist. It never replaces existing settings. Set the model address and exact model name for your service. The address must offer the OpenAI-compatible chat completion format. For example, a locally configured service can use:

```json
{
  "provider": {
    "name": "local",
    "base_url": "http://127.0.0.1:11434/v1",
    "model": "YOUR_INSTALLED_MODEL",
    "request_timeout_secs": 120,
    "idle_timeout_secs": 30
  },
  "agent": { "max_iterations": 40 }
}
```

`YOUR_INSTALLED_MODEL` must be replaced with a model actually available on your server. The example does not install or start a server. For a hosted service, use its correct address and model, and put its key in `CHIMERA_API_KEY`. Do not paste a key into a shared terminal transcript or commit it.

Settings are chosen in this order: `CHIMERA_*` environment values, the selected settings file, then defaults. `CHIMERA_CONFIG` selects a different JSON file. `OPENAI_API_KEY` is a fallback only when no explicit key or file key is set. Bad explicit settings produce an error; the program does not silently choose another service.

```sh
chimera doctor
chimera run "Reply with a short greeting. Do not use tools."
```

`doctor` checks settings and saved records. The small `run` checks the actual model connection. Tool and streaming support vary by server and require their own checks. Usage is shown only when the server supplies it. Cost is not estimated.

## Work on a project

```sh
chimera plan "Read this project and explain a small useful change."
chimera run "Make the agreed change and run its check."
chimera run --interactive "Read this project and explain it."
chimera tui
```

Planning rejects file changes and shell commands. Normal mode asks before each change or command. Read the full action and type `yes` or `no`. Approval expires in five minutes and cannot be reused for different arguments. Files read earlier are checked again before being replaced, and originals are saved for recovery.

The interactive screen accepts follow-up messages. Use `/approve`, `/deny`, `/cancel`, `/quit` or `/exit`. F1 opens help. Shift+Enter or Alt+Enter inserts a new line. Up and Down browse input history. Tab expands the latest tool result. `/theme` changes the display. `/clear` clears the screen, not saved history. Unknown commands explain the error.

Control+C cancels active work. A failed tool, an empty or unfinished model answer, and a work limit all produce an unfinished or failed outcome. A model's final answer is saved as an answer; it is not independent proof that its claims are correct.

## Continue and recover

```sh
chimera session list
chimera session resume SESSION_ID
chimera session fork SESSION_ID
chimera replay SESSION_ID
chimera session export SESSION_ID
chimera checkpoint create --session SESSION_ID --file src/example.rs
chimera checkpoint list
chimera rollback CHECKPOINT_ID
```

Replace the uppercase identifiers with saved identifiers shown by the program. Replay displays saved conversation without running its tools. Replay can contain private project text; use export for private-data-free diagnostics. Export deliberately omits conversation bodies, tool arguments and file paths.

A checkpoint captures the conversation and only the files named with `--file`. Restoration asks before changing captured files, checks for concurrent edits, and preserves replaced copies and restoration history. Unlisted files are left alone. `session recover SESSION_ID` restores the previous saved record when the latest one is damaged and preserves the damaged copy.

## Permissions and background work

```sh
chimera policy show
chimera policy set read-only
chimera policy set supervised
chimera autonomy set low
chimera autonomy set medium
chimera tool disable bash
chimera tool enable bash
chimera approve list
chimera approve action ACTION_ID
chimera deny ACTION_ID
```

`none` stops agent tools. `low` requires approval for every tool. `medium` requires approval for changes and commands. Higher automatic permissions are unavailable. `leash all` selects review of every action. No setting grants tools unrestricted network or access to private project state.

```sh
chimera schedule hourly "Read the project and report changed source files."
chimera watch "Review changed source files." --pattern "src/**"
chimera up
chimera daemon status
chimera jobs list
chimera jobs pause JOB_ID
chimera jobs resume JOB_ID
chimera jobs cancel JOB_ID
chimera down
```

The service uses a private local control socket. It does not open a public network port. Scheduled changes still need exact approval in this project. Failed jobs pause. After a crash, ambiguous work is marked interrupted and paused, so side effects are not automatically repeated. Inspect the saved run before resuming. `daemon serve` runs the service in the current terminal for diagnosis.

## Other working controls

`swarm "objective"` runs saved specialist sessions. `pack ship-hotfix --objective "objective"` runs the built-in team. `creature list` shows real team members; inspect, stop, leash and promote use their saved identifiers. Specialists have file scopes and separate results. Their role names do not prove a check was run. Unsupported pack permissions fail before work starts.

`world spawn local_shell` creates a tracked command environment. Use `world list`, `world inspect ID`, `world exec ID "command"`, `world pause ID`, `world resume ID` and `world close ID`. Commands require review. `run "objective" --world ID` connects an agent to that real environment in ordinary terminal mode; closing or pausing it stops owned commands. External tool programs cannot be combined with this isolation option. Working copies with edits are preserved when closed.

`memory remember KEY CONTENT`, `memory list` and `memory show ID` store facts for this project. New and resumed requests read a limited selection of those facts as reference text. Stored text cannot grant permission. `memory forget ID` archives a fact. `skill capture NAME SESSION_ID` captures a completed objective as a workflow. `skill run NAME` runs it anew under current permissions. `eval define NAME --file PATH --command COMMAND` saves a real check; `eval run NAME` reports its actual result. Missing or failed checks never earn a passing result.

`browser open TARGET --actions actions.json --output-dir evidence` performs an explicitly approved browser sequence, saves a screenshot and closes its Chrome process. Page requests to web addresses need origins named with repeated `--allow-origin`; with none, the page stays offline, but Chrome process network access still requires approval. This is a browser request policy, not an operating-system network boundary. Android uses `phone boot PROFILE --app app.apk --package APP_PACKAGE --network`; network-disabled Android execution is currently unavailable and will be rejected. No Android device has been verified on this machine.

`run "objective" --mcp-config servers.json` attaches explicitly approved external tools to the agent in ordinary terminal mode. Each later call needs its own approval. A failed connection requires fresh startup approval; uncertain actions are never automatically repeated.

`mcp --config servers.json tools SERVER` discovers tools from an explicitly selected external tool server. `call`, `resources`, `read` and `healthcheck` perform real protocol operations. Starting a configured external program requires review. Server descriptions cannot grant approval. See each command's `--help` for its arguments.

## Notifications

`notify destination NAME --terminal` creates a local inbox. `notify destination NAME --url HTTPS_URL` selects a real webhook destination. `notify send NAME SUBJECT BODY` shows a message for review before sending it once. Use `notify list`, `notify inspect ID` or `notify inbox` to inspect delivery. A successful server response means the server accepted the message; it does not prove a person read it. `notify acknowledge ID` records an explicit acknowledgement.

`notify approval ACTION_ID --destination NAME` sends an existing pending action for review. The receiver must use an authenticated terminal for this project to run `approve action ACTION_ID` or `deny ACTION_ID`. Receiving a webhook response never approves work. Expired and duplicate decisions fail. No external recipient has been verified for this beta yet.

## Saved data and reporting

Project records live in `.chimera/`. They include conversations, actions, approvals, recovery copies, jobs and artifacts. Settings live in `~/.chimera/config.json` unless selected elsewhere. Keep both private and backed up. The repository ignores `.chimera/` and secret environment files; use the same ignore rules in other projects. Removing the executable must not remove this data.

For a problem report, include the executable version, the command used, the visible error and a redacted session export. Check an export before sharing it. Do not post keys, private source, or raw conversations. The private security reporting destination still needs owner confirmation; see [SECURITY.md](SECURITY.md).
