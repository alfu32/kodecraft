# kode-rs

Planned Rust migration of the Kode terminal IDE. The implementation reference is
`../kode`, commit `7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68` (audited 2026-10-14).

Implementation is underway. [DASHBOARD.md](DASHBOARD.md) records all 65 task statuses,
ongoing work and links to acceptance evidence. Open a file or folder with
`cargo run -p kode -- /path/to/project`; Ctrl+S saves and Ctrl+Q saves workspace
state and quits. The shell includes file/project panels, search, text editing,
hex inspection, portable image viewing, syntax/semantic highlighting and native
completion/navigation, local Git operations and diff views, SQL/JDBC and REST.
Settings, final editor qualification and release work continue in dependency order.
`cargo check --workspace`, `--help` and `--version` work on stable Rust.
The Rust application is a TUI. JDBC runs in an **external JVM**;
the Rust process does not embed a JVM.

Ctrl+T opens your shell in the workspace directory. Type `exit` to return to the
editor; terminal modes, caret and unsaved buffers are retained. Browser terminal
mode is `kode serve [--port 11045] [folder] [-- <ttyd arguments>]`. It binds
`127.0.0.1` by default; use `--public` or `--host` for another bind address and
`--credential user:password` for Basic authentication. Install `ttyd` on PATH,
place its executable beside `kode`, or set `KODE_TTYD` to its executable path.
`python3 scripts/build-ttyd.py` builds a pinned standalone ttyd executable and
records provenance; native development libraries listed by its `--help` are
required. Native runtime libraries and release bundles are qualified separately.
`--resources DIRECTORY` within `serve` selects the child IDE's resource override.
Remaining arguments after `--` pass literally to ttyd; an unknown option also
starts forwarding, matching the original CLI.

For remote access, keep the loopback bind and use
`ssh -L 11045:127.0.0.1:11045 remote-host`, then open
`http://127.0.0.1:11045`. Browser tabs reserve shortcuts such as Ctrl+T/Ctrl+W;
`google-chrome --app=http://127.0.0.1:11045` is the existing app-window workaround.
Classic terminal keyboard encodings can also merge Ctrl+Enter with Enter; use
the SQL Execute and REST Send buttons in those terminals. Browser clipboard
paste and xterm selection copy remain available. ttyd notice logs are suppressed
when credentials are supplied because its notices include a reversible Basic
auth token; explicitly forwarding a higher ttyd `--debug` level enables those logs.

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
End-to-end JDBC qualification is recorded in [D09](docs/parity/D09.md).
After a helper crash, check the database outcome, then use Alt+6, `R` to restart
the service and `c` to reconnect. Previous SQL is retained and never replayed.

Open **REST** with Alt+7. `/` filters the collection; `n` adds a request, `f` a
folder, `m` opens actions, `i` imports Postman, `e` exports and `r` runs/stops the
selected request or folder. The actions menu also imports OpenAPI into a folder.
Tab selects environments: `n` creates, `c` copies, F2 renames and Space activates.
Environment values use the shared editor and survive restart, including comments
and unfinished lines. Drag the environment divider to resize it. Open a request
for method/URL controls and params/auth/headers/body/variables/settings/raw tabs.
Ctrl+Enter sends or stops; Ctrl+Tab switches request tabs from an editor. Header
names complete with Ctrl+Space. Request edits, comments and unfinished headers
survive restart. Responses have Body/Headers/Cookies/Raw/Transaction tabs, a
draggable divider and optional display formatting. Format body explicitly changes
the editable body and supports undo. Folder/collection runs show selectable
per-request progress and outcomes. F7 opens run history; click or Enter an outcome
to inspect it, and use Stop/Run again to control the selected run. Panel sizes and
Pretty preferences survive restart; runtime responses and cookies do not.
Postman scripts are preserved without execution. OAuth acquisition and additional
authentication types require extensions.

Ctrl+Space requests completion and Ctrl-click follows definitions/usages.
The Project panel provides `r` rescan, `b` rebuild index and `c` clear index.
Indexing runs on bounded workers, uses unsaved buffers and persists its cache in
`.kode-rs/index.sqlite` without Java or language servers. `KODE_CODEINTEL_PATH`
can select a custom definitions catalog. Kotlin/C have specialized extraction;
other languages use conservative AST or explicit regex fallback capabilities.

Open **Settings** with Alt+4 for language-server discovery and installation.
Tab or the LSP/Index buttons switch between language servers and native cache
controls. In Index, `r` rescans, `b` rebuilds and `c` clears the rebuildable Rust
cache. The panel shows scan/extract/resolve/persist progress and scrolls with
Up/Down/Page/Home/End or the mouse wheel. These controls leave user JDBC databases
and dirty editor buffers intact.
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
