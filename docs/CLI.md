# CHIMERA command reference

Start with the [user commands sheet](COMMANDS.md) for terminal commands, chat commands, keys, and current limitations.

For first-time setup, use the [quick start](QUICKSTART.md). For settings, permissions, saved conversations, recovery, teams, and optional connections, use the [user manual](USER_MANUAL.md).

The executable provides exact arguments for each command:

```sh
chimera --help
chimera run --help
chimera session --help
```

Developers can inspect command definitions in [main.rs](../crates/chimera-cli/src/main.rs). Commands listed by help can still require model setup, additional software, or explicit action approval; the commands sheet identifies unavailable variants.
