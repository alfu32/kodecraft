# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Kode (`kode-rs`) is a Rust terminal IDE (Ratatui/Crossterm), migrated from the Kotlin implementation in `../kode` (commit `7d3e5953`), which remains the behavioral reference. It has an editor, Git, SQL/JDBC, REST, LSP and native code intelligence.

## Commands

`default-members` is only `crates/kode`, so a bare `cargo test` or `cargo clippy` skips nearly everything. Pass `--workspace` or `-p <crate>`.

```sh
cargo run -p kode -- /path/to/project      # run the TUI

# The checks the release workflow gates on
cargo fmt --all --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
python3 scripts/credits.py --check         # omit --check to regenerate after dependency changes; --offline needs every target's crates cached
python3 scripts/tasks.py --check           # ledger and dashboard are in sync

# Single test
cargo test -p kode-core --test search                  # one integration test file
cargo test -p kode-tui --test git_panel <test_name>    # one test

# JDBC helper: needs Java 21 and Maven. Builds the JAR, then runs Maven and Rust bridge tests, including ignored ones
bash scripts/test-jdbc-bridge.sh
bash scripts/build-jdbc-helper.sh          # helper JAR only

# PTY acceptance tests drive the real binary; build first
cargo build -p kode
python3 scripts/test-git-terminal.py       # scripts/test-*-terminal.py, one per feature; KODE_TERMINAL_BINARY overrides the binary
```

The toolchain follows the latest `stable` (`rust-version` is 1.99), so run `rustup update stable` if the build rejects the compiler. `unsafe_code` is forbidden across the workspace.

## Architecture

Dependency direction is strict: `kode` (CLI and composition) → `kode-tui` → feature crates (`kode-workspace`, `kode-syntax`, `kode-intelligence`, `kode-lsp`, `kode-git`, `kode-rest`, `kode-database`) → `kode-platform` → `kode-core`. Feature crates never depend on `kode-tui`. `kode-workspace` stores feature session blocks as opaque JSON, so REST and database models evolve without dependency cycles; unknown persisted fields must survive round trips.

**Threading and state.** The main thread owns terminal output, focus, document edits and `AppState`. Reducers in `kode-tui/src/app/` consume terminal events, timers and typed worker results, and return effects (schedule work, save, navigate, persist, repaint). They do no blocking I/O. Blocking work (Git, filesystem, SQLite, parsing, images) runs on bounded workers and async process/network work runs on Tokio; both receive immutable snapshots.

**Stale results.** Every async operation carries a cancellation token and a `JobTag` (workspace generation, request ID, document ID and revision). `ResultGate` filters on these identities before publication, so an old result cannot overwrite a newer edit or a switched workspace. Save completions and failures must never be dropped for backpressure.

**Input routing.** One component consumes each event, in order: top modal → captured popup or drag → focused child/editor → containing panel → global binding. Help text is generated from the same command catalog the reducers use.

**Editor.** The editor in `kode-tui/src/editor/` is shared by file, SQL, REST and environment buffers. Keep Git, SQL and REST rules out of it. Text coordinates are typed in `kode-core` (`ByteOffset`, `CharOffset`, `Utf16Column`, `CellColumn`): UTF-16 columns exist only for LSP and legacy adapters and are never byte offsets.

**JDBC.** The Rust process embeds no JVM. `kode-database` supervises an on-demand Java 21 helper (`jdbc-bridge/`) over the stdio protocol defined in `protocol/jdbc-v1/`, whose wire fixtures are tested from both Rust and Java. Ordinary editor startup and indexing must work without Java. Failed helpers never replay SQL.

**Code intelligence.** LSP is optional. `kode-intelligence` is the native fallback, with Tree-sitter and regex extraction via `kode-syntax`, and a rebuildable SQLite cache at `.kode-rs/index.sqlite`. Non-empty LSP results take precedence; missing or failing servers fall back to native.

**Persistence.** Project state lives in `.kode-rs.json` (format in `docs/formats/session-v1.md`). Legacy `.kode.json` and `.kode/` are read as import sources and not modified.

## Migration ledger

Work is tracked as task cards with evidence, and these docs are authoritative:

- `docs/migration/CONTRACTS.md`: shared contracts. A change in semantics must update this file and its consumers.
- `docs/migration/TASKS.md` and `docs/migration/tasks/*.md`: the task index and cards. `docs/parity/<ID>.md` holds the evidence reports.
- `docs/migration/HANDOFF.md`: the implementation procedure. For card work, read it along with `PLAN.md`, `CONTRACTS.md` and the card.
- `docs/migration/EDITOR-UX.md` (editor changes) and `docs/migration/JDBC.md` (database changes).

Task status lives in the cards. Change it with `python3 scripts/tasks.py <ID> <state>` and run `python3 scripts/tasks.py` to regenerate `DASHBOARD.md`; don't hand-edit the dashboard. A task isn't done because it builds or a panel renders: its card's acceptance checks must pass and be recorded. Vendor databases that weren't available are recorded as untested. Commits use Conventional Commit summaries, e.g. `feat(editor): ...`.

## Releases

Releases run only through the manual GitHub Actions workflow (`.github/workflows/distribution.yaml`; `gh workflow run distribution.yaml --ref work -f source_ref=<version>`). The version must match in `Cargo.toml` and `jdbc-bridge/pom.xml`, and tags are the bare version with no `v` prefix. A version bump also touches `Cargo.lock` and the example in `packaging/README.md`. Build and qualification details are in `packaging/README.md` and `docs/migration/Z03-ARTIFACT-MATRIX.md`.

Store publishing is in `packaging/STORES.md`. The MSIX identity comes from `packaging/msstore-info.txt`; its package version is the release version with the major number plus 1, because the Microsoft Store rejects a leading 0. `packaging/msstore/` is imported into Partner Center as a folder, so it must keep its name and hold one CSV plus only the assets that CSV references. README screenshots live in `packaging/msstore/screenshots/`.
