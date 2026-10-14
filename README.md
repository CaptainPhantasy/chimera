# CHIMERA

**One shell. Many creatures. Actual work.**

CHIMERA 0.1.0 · Documentation reviewed 14 October 2026.

Built in **Floyds Labs**. Fueled by caffeine, stubbornness, and the belief that your tools should earn their place on your machine.

CHIMERA is a terminal assistant for working on a project with a language model. Ask it to read the code, explain a problem, make a change, or run a check. It keeps the conversation with the project and asks before changing files or running commands.

Bring your own model. Keep your hands on the controls. Make the machine show its work.

![CHIMERA showing its built-in tools in a real terminal session](docs/images/chimera-tools.png)

*The actual CHIMERA app, showing `/tools`. Captured from the built program; no generated interface or invented conversation. These screenshots show local controls, not a model connection test.*

## Pick your starting point

| You want to… | Open this |
| --- | --- |
| Get it running and send your first request | [Quick start](docs/QUICKSTART.md) |
| Find a command without digging | [User commands sheet](docs/COMMANDS.md) |
| Learn settings, approvals, saved work, and recovery | [User manual](docs/USER_MANUAL.md) |

## Get it on the bench

This version targets **macOS**. Building from source needs Rust and Cargo; the current build uses Rust 1.98. Use the checked-in dependency versions:

```sh
git clone https://github.com/CaptainPhantasy/chimera.git
cd chimera
cargo build --locked --release -p chimera-cli
./target/release/chimera --version
./target/release/chimera tui
```

The chat screen can open before a model is configured. Type `/setup` for the settings-file location. Follow the [quick start](docs/QUICKSTART.md) to connect your local model or hosted service, then use `/reload`.

Run CHIMERA **from the project you want it to work on**. Conversations and project records live in that project's `.chimera/` folder.

## Put it to work

Once `chimera` is installed or available by its full path:

```sh
chimera plan "Read this project and explain one useful improvement."
chimera run --interactive "Help me make that improvement and check it."
chimera session list
```

Planning can read the project. File changes and shell commands need approval for the exact action. In chat, use `/approve` or `/deny`. Use `/cancel` to stop a request and `/quit` to leave.

![CHIMERA keyboard and chat command help](docs/images/chimera-help.png)

*Help inside the running app. F1 opens it; Escape closes it.*

## More tools in the garage

- **Saved conversations and checkpoints:** return to a discussion, fork it, or restore files you explicitly captured.
- **Specialist teams:** give a larger job to separate workers with saved scopes and results.
- **Project memory and workflows:** keep useful facts and repeat a completed objective under today's permissions.
- **Background jobs:** schedule work or watch files, then inspect, pause, resume, or cancel it.
- **Optional connections:** run reviewed browser steps, attach external tool programs, or send reviewed notifications.

The [manual](docs/USER_MANUAL.md) covers these features and their limits. Optional browser and Android controls need their own installed software. A command being listed does not mean its outside service is configured.

## Straight talk about this build

CHIMERA is under active development. A saved answer is the model's answer; check the changed files and the results of the commands it ran. `doctor` checks local settings and records. A small real request checks the model connection. Those are different checks.

For problems, collect `chimera --version`, the command, the visible error, and a reviewed `chimera session export SESSION_ID`. Keep keys and raw private conversations out of bug reports.

[Development notes](CONTRIBUTING.md) · [Architecture](ARCHITECTURE.md) · [Security](SECURITY.md) · [MIT license](LICENSE) · [Screenshot capture record](docs/images/capture-record.json)

**Floyds Labs — less ceremony, more working software.**
