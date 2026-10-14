# CHIMERA terminal knowledgebase

Read date: 2026-10-11 (America/Indianapolis). All notes below are original summaries of version-specific documentation, matching cached published source, and explicitly labelled CHIMERA design rules. No training-data fallback was needed.

## Version boundary

| Package | CHIMERA Cargo.lock version | Treatment |
| --- | --- | --- |
| Ratatui | 0.29.0 | Preserve; no ratatui::run in this version. |
| Crossterm | 0.28.1 | Preserve current features; one input reader. |
| Tokio | 1.50.0 | Preserve existing runtime and channels. |
| anyhow | 1.0.102 | Preserve existing application error handling. |

Newer Ratatui 0.30.2 documentation is separate research context from this task, not the implementation target or authorization for a dependency upgrade. Recheck Cargo.lock before future work.

## Notes

SHA-256 values identify the saved bytes. This index deliberately does not hash itself. A source URL identifies the original reference; where web retrieval was unavailable, the listed matching cached source supplied the evidence.

| Note | Read date | Documented versions | SHA-256 | Source URLs |
| --- | --- | --- | --- | --- |
| [crossterm/events.md](crossterm/events.md) | 2026-10-11 | Crossterm 0.28.1; Tokio 1.50.0 | `abb1893d2c86fd518b6b55cac75e23ae0155f5c56ac9ca742ef1c63583f0c4cc` | [reference 1](https://docs.rs/crossterm/0.28.1/crossterm/event/index.html); [reference 2](https://docs.rs/crossterm/0.28.1/crossterm/event/fn.poll.html); [reference 3](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.EventStream.html) |
| [crossterm/keys.md](crossterm/keys.md) | 2026-10-11 | Crossterm 0.28.1 | `c314b9a91c1e66328f38ee9c64a4d4f86b9483315bbfc1eb63918d1a2645c456` | [reference 1](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.KeyEvent.html); [reference 2](https://docs.rs/crossterm/0.28.1/crossterm/event/enum.KeyEventKind.html); [reference 3](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.KeyModifiers.html) |
| [crossterm/lifecycle.md](crossterm/lifecycle.md) | 2026-10-11 | Crossterm 0.28.1; Ratatui 0.29.0 | `71ed4550bb6391487051284271f7c9ab5b08f466368496b88bb96b339ca81088` | [reference 1](https://docs.rs/crossterm/0.28.1/crossterm/terminal/index.html); [reference 2](https://docs.rs/ratatui/0.29.0/ratatui/fn.try_init.html); [reference 3](https://docs.rs/ratatui/0.29.0/ratatui/fn.try_restore.html); [reference 4](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.EnableBracketedPaste.html) |
| [crossterm/mouse.md](crossterm/mouse.md) | 2026-10-11 | Crossterm 0.28.1 | `1639e08670e0ea242033fe754bc52b5df18b984aaa690d48e30ac99ae4c835d0` | [reference 1](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.EnableMouseCapture.html); [reference 2](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.DisableMouseCapture.html); [reference 3](https://docs.rs/crossterm/0.28.1/crossterm/event/struct.MouseEvent.html) |
| [crossterm/styles.md](crossterm/styles.md) | 2026-10-11 | Crossterm 0.28.1 | `5afadd305ea08d70107b543b985f9637e9db23df700c16a8079b5f30337a9c3a` | [reference 1](https://docs.rs/crossterm/0.28.1/crossterm/style/index.html) |
| [patterns/patterns.md](patterns/patterns.md) | 2026-10-11 | Ratatui 0.29.0; Crossterm 0.28.1; Tokio 1.50.0; anyhow 1.0.102 | `c392834f4aabbf101f6a33c7c6c563c5c4b93c72058f8f58a254f43b356096da` | All preceding notes; [reference 1](https://docs.rs/anyhow/1.0.102/anyhow/); [reference 2](https://docs.rs/tokio/1.50.0/tokio/task/) |
| [ratatui/backends.md](ratatui/backends.md) | 2026-10-11 | Ratatui 0.29.0 | `7438e13a0caba9c31c3b1a858399d7062e2a41ae430d2f0fa93e00f27eec20c9` | [reference 1](https://docs.rs/ratatui/0.29.0/ratatui/backend/index.html); [reference 2](https://docs.rs/ratatui/0.29.0/ratatui/backend/struct.CrosstermBackend.html); [reference 3](https://docs.rs/ratatui/0.29.0/ratatui/backend/struct.TestBackend.html) |
| [ratatui/layout.md](ratatui/layout.md) | 2026-10-11 | Ratatui 0.29.0 | `840df7f91eb876bd7af7fff8173411f5c11e989046041cfb3c1483c7a4fb7542` | [reference 1](https://docs.rs/ratatui/0.29.0/ratatui/layout/struct.Layout.html); [reference 2](https://docs.rs/ratatui/0.29.0/ratatui/layout/enum.Constraint.html); [reference 3](https://docs.rs/ratatui/0.29.0/ratatui/struct.Frame.html) |
| [ratatui/rendering.md](ratatui/rendering.md) | 2026-10-11 | Ratatui 0.29.0 | `d8961a2f960d5b61338b895397cf6694afcfec2b169c0c96f3bf1fbb743082a3` | [reference 1](https://docs.rs/ratatui/0.29.0/ratatui/struct.Terminal.html); [reference 2](https://docs.rs/ratatui/0.29.0/ratatui/struct.Frame.html); [reference 3](https://docs.rs/ratatui/0.29.0/ratatui/) |
| [ratatui/styles.md](ratatui/styles.md) | 2026-10-11 | Ratatui 0.29.0 | `6ab894811784e90968d793f42a0f47cd3bbe5dcd7b906b76ef8e2b5a444c9798` | [reference 1](https://docs.rs/ratatui/0.29.0/ratatui/style/struct.Style.html); [reference 2](https://docs.rs/ratatui/0.29.0/ratatui/style/enum.Color.html) |
| [ratatui/widgets.md](ratatui/widgets.md) | 2026-10-11 | Ratatui 0.29.0 | `8d41efbf8f3800366e0f9c424345c709c401759e879a3561906bf1084e883553` | [reference 1](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Block.html); [reference 2](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.List.html); [reference 3](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.ListState.html); [reference 4](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Table.html); [reference 5](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.TableState.html); [reference 6](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Paragraph.html); [reference 7](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Tabs.html); [reference 8](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Gauge.html); [reference 9](https://docs.rs/ratatui/0.29.0/ratatui/widgets/struct.Chart.html) |

## Source evidence

[source-evidence.json](source-evidence.json) records cached source locations, source hashes, crate archive hashes, versions, canonical URLs, and checks against Cargo.lock. Read date: 2026-10-11. SHA-256: `1d8df664d5473a461d1cf547267706bdecb63b07a5584f14fbbb581771b4127f`.

The exact-version Ratatui crate overview, Terminal documentation, and Crossterm event documentation were also read from docs.rs. Other requested topics were read from the matching source cache. Archive bytes were checked against Cargo.lock; the source files used for these notes were compared with their archives.

## Application limits

These notes describe APIs and intended patterns. They do not certify that CHIMERA passes a build, that shell cleanup works in a real terminal, or that a model connection works. Source files are shared with another chat and can change; re-read affected paths before editing or claiming a result. Preserve existing chat controls and bracketed paste. Do not initialize a new workspace or require mouse capture merely to follow a template.
