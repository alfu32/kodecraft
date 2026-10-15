# kode-rs

Planned Rust migration of the Kode terminal IDE. The implementation reference is
`../kode`, commit `7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68` (audited 2026-10-14).

Implementation is underway. [DASHBOARD.md](DASHBOARD.md) records all 65 task statuses,
ongoing work and links to acceptance evidence. Open a file or folder with
`cargo run -p kode -- /path/to/project`; Ctrl+S saves and Ctrl+Q saves workspace
state and quits. The shell includes file/project panels, search, text editing,
hex inspection and portable image viewing. Native language intelligence, Git,
SQL/JDBC, REST and remaining release tasks continue in dependency order.
`cargo check --workspace`, `--help` and `--version` work on stable Rust.
The Rust application will remain a TUI. JDBC will run in an **external JVM**;
the Rust process will not embed a JVM.

Use the latest Rust **stable** toolchain (`rustup update stable`). This workspace
selects `stable` through `rust-toolchain.toml`, as requested. Dependency versions
are captured in `Cargo.lock`; validation reports record the exact compiler used.

## Start here

1. Read [scope and architecture](docs/migration/PLAN.md).
2. Read [shared contracts](docs/migration/CONTRACTS.md) before implementing a component.
3. Pick a ready task in the [task index](docs/migration/TASKS.md), then read its linked card.
4. For editor work, also read the [editor interaction contract](docs/migration/EDITOR-UX.md).
5. For database work, also read the [external JDBC protocol](docs/migration/JDBC.md).

Supporting documents:

- [Crate research and selection](docs/migration/CRATES.md)
- [Compatibility, fixtures, and release acceptance](docs/migration/COMPATIBILITY.md)
- [Complete Kotlin source allocation](docs/migration/SOURCE-MAP.tsv)
- [Agent handoff procedure](docs/migration/HANDOFF.md)

The task index is the execution ledger. No task is complete merely because a crate
was selected or a screen can render. Completion includes the behavior and UX
checks in its card and the relevant acceptance matrix.
