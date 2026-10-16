# kode-rs

Planned Rust migration of the Kode terminal IDE. The implementation reference is
`../kode`, commit `7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68` (audited 2026-10-14).

Implementation is underway. [DASHBOARD.md](DASHBOARD.md) records all 65 task statuses,
ongoing work and links to acceptance evidence. Open a file or folder with
`cargo run -p kode -- /path/to/project`; Ctrl+S saves and Ctrl+Q saves workspace
state and quits. The shell includes file/project panels, search, text editing,
hex inspection, portable image viewing, syntax/semantic highlighting and native
completion/navigation, local Git operations and diff views. SQL/JDBC, REST and
remaining release tasks continue in dependency order.
`cargo check --workspace`, `--help` and `--version` work on stable Rust.
The Rust application will remain a TUI. JDBC will run in an **external JVM**;
the Rust process will not embed a JVM.

The external JDBC protocol/bootstrap/supervisor is available as a backend. Build
and test it with `bash scripts/test-jdbc-bridge.sh` using Java 21 and Maven; the
packaged helper needs only Java at runtime. `KODE_JAVA` selects Java and
`KODE_JDBC_HELPER` selects the helper JAR. Ordinary editor startup requires neither.
Driver resolution/loading now supports Maven, local multi-JAR and manual vendor
sources, versioned install manifests and isolated loaders. The backend supports
temporary connection tests, shared datasource connections, context changes and
commit/rollback through serialized JDBC owners. SQL execution streams typed,
bounded previews and supports independent cancellation; helper failure retains
received rows and never replays SQL. Open **Databases** with Alt+6: `n` creates a
datasource, F2 edits, `c` connects/disconnects, `r` refreshes metadata, `R` explicitly
restarts the database service and `s` opens a SQL editor. The connection dialog
uses Alt+T to test, Alt+D to install a driver and Ctrl+S to save. URL Down opens
Recents/Standard suggestions. Passwords are masked and stored separately from
public definitions. SQL consoles share the editor: Ctrl+Enter executes the current
statement or selection, Ctrl+Shift+Enter executes the script, Escape requests
cancellation, and Ctrl+Alt+A/C/R changes shared autocommit/commit/rollback state.
Ctrl+S exports a guarded `.sql` file; unsaved console buffers and context bindings
also survive workspace restarts. Click Catalog/Schema to bind a console. Results
keep their own focus; Alt+Arrows/PageUp/PageDown scroll and Alt+Tab switches result
sets/messages while the editor stays focused. Hover Execute to preview its target.
End-to-end JDBC qualification remains D09.
After a helper crash, check the database outcome, then use Alt+6, `R` to restart
the service and `c` to reconnect. Previous SQL is retained and never replayed.

Open **REST** with Alt+7. `/` filters the collection; `n` adds a request, `f` a
folder, `m` opens actions, `i` imports Postman, `e` exports and `r` runs/stops the
selected request or folder. The actions menu also imports OpenAPI into a folder.
Tab selects environments: `n` creates, `c` copies, F2 renames and Space activates.
Environment values use the shared editor and survive restart, including comments
and unfinished lines. Drag the environment divider to resize it. Request opening
currently shows a read-only JSON snapshot; editable request forms, response tabs
and full runner qualification remain R07/R08.

Ctrl+Space requests completion and Ctrl-click follows definitions/usages.
The Project panel provides `r` rescan, `b` rebuild index and `c` clear index.
Indexing runs on bounded workers, uses unsaved buffers and persists its cache in
`.kode-rs/index.sqlite` without Java or language servers. `KODE_CODEINTEL_PATH`
can select a custom definitions catalog. Kotlin/C have specialized extraction;
other languages use conservative AST or explicit regex fallback capabilities.

Open **Settings** with Alt+4 for language-server discovery and installation.
Use `f` to refresh, Enter to install/update, `y` to remove, and Escape to cancel.
`KODE_LSP_CATALOG` selects a custom catalog and `KODE_LSP_HOME` selects the install
root. Existing local executables and manifest command arrays are supported.
Use `s` to start/stop and `r` to restart a server. Running state follows the actual
initialized child; workspace changes and exit stop and reap owned servers.
Opening a file attempts to start its first matching local server in the background;
it never installs a package. Current non-empty LSP definitions, references and
completion take precedence; missing, crashed, empty or failing servers use native
intelligence. Highlighting and definition previews retain native styling.

Open **Git** with Alt+5. Space stages/unstages the selected status row; `a` stages
all rows and `A` unstages them. Tab enters the shared commit-message editor,
Ctrl+Enter commits, and Ctrl+Tab cycles sections. Outside that editor, `b` selects
a local branch, `t` creates a tag, and `T` explicitly creates or moves a tag on the
selected commit. Enter expands history; `m` copies its message into the draft.
Checkout preserves local changes and refuses dirty file or binary buffers.
Enter on a status row opens its diff in the main pane. `c` toggles context, `v`
switches side-by-side/unified presentation, and Enter/`r` or a center arrow restores
the selected unstaged text chunk. Previews support shared selection, copy and find;
staged/historical comparisons are read-only. Leaving Git restores the ordinary viewer.

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
