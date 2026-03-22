# Contributing to CHIMERA

Thank you for your interest in contributing. This guide covers the common workflows for developing, testing, and submitting changes.

Development setup
- Install Rust (stable) and Cargo
- Clone repository and build: `cargo build --workspace`
- Run the CLI locally: `cargo run -p chimera-cli -- <command>`

Testing
- Run unit and integration tests: `cargo test --workspace`
- Coverage artifacts are produced under `target/llvm-cov` in CI.

Code style
- Format code with `cargo fmt` (project uses gofumpt for Go; Rust uses `rustfmt` defaults)
- Write clear tests for behavior and edge cases

Making changes
1. Create a feature branch from `main`.
2. Implement change with tests.
3. Run `cargo test --workspace` and fix failures.
4. Create a concise PR describing the motivation and test plan.

Reporting bugs
- Open an issue with: reproduction steps, platform, Rust version, and a small repro if possible.

Security disclosures
- See SECURITY.md for reporting sensitive issues.

Maintainers
- Maintain a single active `main` branch. Use PRs for all changes.
