# Security and private reports

The private reporting destination has not yet been confirmed. The old `security@chimera.local` address was a placeholder and must not be used. Do not post a private vulnerability or credentials in a public issue. A working private contact is an open beta requirement.

CHIMERA limits built-in file tools to the selected project, rejects protected state and symbolic-link escapes, requires exact approval for file changes and commands, and keeps recoverable originals. Shell commands use the implemented macOS isolation boundary and deny network access. Unavailable isolation must return an error rather than run on the host without it.

External tool programs and browser/device sessions require explicit review. Browser origin rules are not an operating-system network sandbox. Android network-disabled operation is unavailable. A tool description, role prompt or model response cannot grant permission.

Saved conversation and recovery files can contain private project content. Keep `.chimera/` and model settings private. Use `session export` for diagnostics; ordinary replay is private conversation output. Environment keys must not be committed or included in reports.

This repository is under beta preparation. A passing controlled test is not a claim that every provider, device or user workflow has been verified.
