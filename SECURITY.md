# Security Policy

Reporting a Vulnerability
If you discover a security vulnerability, please report it privately by opening an issue and marking it as a security report, or email security@chimera.local (placeholder). Include a clear description, reproduction steps, and an impact assessment.

Supported release: initial public release v0.1.0

Security model
- ExecutionScope and approval classes are primary safety mechanisms. Packs restrict network/commit permissions.
- Sensitive files are excluded by default from worktrees (see pack ExecutionScope denied_paths defaults).

Handling secrets
- Do not commit secrets into the repository. Worktrees and worlds default to read-only unless explicitly configured.

Third-party components
- Keep dependencies up to date. The workspace uses tokio, tracing, serde, clap, and other standard crates.

Responsible disclosure
- We will acknowledge reporters promptly and coordinate disclosure timelines.
