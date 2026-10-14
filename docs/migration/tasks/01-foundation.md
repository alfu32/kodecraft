# Foundation tasks

All paths and shared API rules follow [CONTRACTS.md](../CONTRACTS.md).

## F01 — Freeze behavior and fixtures

**Status:** done

**Depends:** none. **Owns:** `fixtures/`, `docs/parity/`, source/test inventories.
**Read:** source `README.md`, `AGENTS.md`, `src/test/kotlin/`, `editor/app/Main.kt`,
and the source anchors in [EDITOR-UX.md](../EDITOR-UX.md).

1. Record source commit and toolchain; inventory all executable commands, dispatch
   branches, dialogs, external resources and catalog entries. Link each to a task
   and acceptance row. Read both code and assertions when a test name is misleading.
2. Copy/synthesize minimal licensed fixture texts, session JSON, Postman/OpenAPI
   documents, JDBC catalogs, MIME/image samples and semantic projects. Never copy
   live `.kode.json`, `.secrets`, personal paths or credentials.
3. Record baseline outcomes for the existing Kotlin regressions. Separate existing
   behavior from proposed features and intentional corrections (newline duplication,
   invalid Unicode boundaries, failed loads, virtual saves, read-only edit bypass).
   Save named event sequences for editor focus and mouse/key interactions.

**Accept:** every production source file and original test suite has an owner;
every implemented UI event branch maps to an acceptance case. Fixture manifest
records origin/license and expected output. Known baseline failures are recorded,
not silently copied into Rust expected behavior. This task is the evidence input
for all later task checks; it does not require porting production code.

## F02 — Cargo workspace and shared types

**Status:** done

**Depends:** F01. **Owns:** workspace manifests/toolchain, crate skeletons,
`crates/kode-core/src/{ids,coordinates,commands,intelligence}.rs`.
**Read:** `editor/lib/ITextBuffer.kt`, `IByteBuffer.kt`, `BufferState.kt`,
`editor/codeintel/EditorIntelligence.kt` and CONTRACTS.md.

1. Create the planned crate graph and fixed stable toolchain; select compatible
   crate versions and minimal features from CRATES.md. Commit the lockfile when
   implementation is committed; document native libgit2/SQLite/Tree-sitter needs.
2. Define newtypes, half-open ranges, document origins, job tags, editor commands,
   navigation/completion DTOs and typed error boundaries. Use byte offsets only
   where the type explicitly says bytes.
3. Provide dependency-free domain interfaces and test fakes. Keep a minimal CLI
   help/version command buildable; do not label empty feature crates implemented.

**Accept:** workspace checks compile, dependency graph has no cycle, one compatible
   Crossterm backend is resolved, shared serialization fixtures pass. No UI types
   appear in core services. Run fmt/check/clippy for the foundation packages.

## F03 — Jobs, processes, paths, logging and shutdown

**Status:** in-progress

**Depends:** F02. **Owns:** `kode-platform/src/{jobs,process,paths,logging}.rs`,
`kode-tui/src/app/{events,effects}.rs`.
**Read:** `editor/app/Logger.kt`, asynchronous sections of `Main.kt`, `CodeEditorView.kt`.

1. Implement bounded job/event channels, cancellation, coalesced repaint/progress,
   immutable request inputs and generation/revision gating. Use Tokio for I/O and
   bounded blocking workers for file/Git/parser/image/index work.
2. Implement resource/config path precedence and child spawn/drain/terminate/reap
   helpers. Redirect logs away from terminal output; retain bounded child stderr.
3. Define disposal and shutdown order. Inject clock, process factory and worker
   scheduler for tests; stale jobs must not touch replaced workspace state.

**Accept:** delayed A/fast B job ordering, workspace switch, full queue, cancellation
   and child EOF/exit all terminate without deadlock. Completion triggers repaint
   without keyboard input. Runtime starts with no Java/LSP/ttyd installed.

## F04 — Terminal lifecycle and normalized input

**Status:** todo

**Depends:** F02, F03. **Owns:** `kode-tui/src/terminal/`, `input/terminal.rs`.
**Read:** `react/AnsiCanvasRenderer.kt`, `react/UIEvent.kt`, `react/util/*.kt`,
`react/CanvasRenderer.kt`; `AnsiCanvasRendererTest`.

1. Implement Ratatui/Crossterm terminal ownership with raw mode, alternate screen,
   cursor, mouse/focus and bracketed paste setup; restore on error/panic/normal exit.
2. Normalize key press/repeat/release, modifier variants, Enter/Return/CR/LF,
   PageUp/PageDown, NUL completion, resize and pointer events. Preserve typed case.
   Use exactly one Crossterm input source; suspend it before a shell child reads stdin.
3. Provide suspend/resume and redraw hooks. Request enhanced keyboard reporting
   only when supported; retain fallback bindings in classic terminals.

**Accept:** UX-003/020/077/107 plus PTY restoration after deliberate error; no delayed
   PageDown requiring another byte; no duplicate edits from release events; resize
   to tiny areas is safe. Linux first, Windows/macOS input adapters documented.

## F05 — Common widgets, style mapping, layout and focus

**Status:** todo

**Depends:** F04. **Owns:** `kode-tui/src/{widgets,theme,focus,layout}/`.
**Read:** `react/{Component,ContentBox,TabView,ClippedCanvasRenderer,StyleSheet,StyleSet,Color}.kt`,
`editor/ui/{ModalDialog,FormFieldRenderer,SliderControl}.kt`, `styles/app.css`;
`TabViewTest`, `SearchReplaceBarTest`, database/REST form views.

1. Port current style names/properties/default merging to typed Ratatui styles and
   a compatibility loader for shipped CSS-like resources. Use standard widgets
   for lists/tables/borders and custom widgets where exact cell control is needed.
2. Implement clipped panel coordinates, vertical tabs, draggable splitters,
   scrollbars, sliders, bounded text fields, buttons and modal stack. Keep focus
   state separate from hover. Support shrinking/overflowing tab strips.
3. Implement event-consumption priority, drag capture and release, focus restoration
   after modal dismissal, field→OK→Cancel traversal and explicit cursor display.

**Accept:** UX-040/048/052/053/105/106; clicking a control never activates its
   neighbor, typing into a modal cannot reach the editor, drag ends outside bounds,
   all controls remain usable at small sizes. Cell/state snapshots cover focus and
   style merging; no raw terminal writes inside a widget.
