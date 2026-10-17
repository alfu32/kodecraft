# Finalized UAT batch (2026-10-16)

User requests are preserved in [the UAT document](../../UAT/UAT.md). These cards
extend the current implementation goal. [UAT tracking](../UAT.md) maps every
original bug/modification ID to its implementation owner and acceptance evidence.

## U01 — Finalized file and project search shortcuts

**Status:** done

**Depends:** Z02, E07, W04. **Owns:** shared shortcut catalog, application/editor
routing, Help/README and real terminal binding fixtures. **UAT:** B002.

1. Apply the finalized mapping: Ctrl+F file find/replace and Alt+F project search.
   This supersedes the earlier Alt+F file/Ctrl+Shift+F project preference. Retain
   Ctrl+Alt+F and Ctrl+Shift+F as optional project aliases; neither is required
   when the terminal host intercepts it.
2. Preserve search-field versus host execution priority and all editor hosts.
   Generate Help from the actual bindings and update user-facing documentation.

**Accept:** real PTYs at wide/narrow sizes open the correct scope with Ctrl+F and
   Alt+F, close without mutating the document, and verify aliases where delivered.
   File/project search remains usable without Ctrl+Shift+F reaching Kode.

## U02 — Readable database errors and diagnostics

**Status:** done

**Depends:** D09, Z02. **Owns:** database/SQL status and diagnostic presentation.
**UAT:** B003.

1. Keep a concise status summary and provide a discoverable wrapped, scrollable
   error detail view for connection/test/install/metadata/SQL/helper failures.
   Preserve the full useful message, SQL state/vendor code and causal context.
2. Support keyboard/mouse scrolling and copying details, resize/tiny panels,
   dismissal and focus restoration. Retain credential redaction.

**Accept:** deliberately long and multiline JDBC errors remain completely readable
   at 60/100 columns; users can reach the tail and copy it. The owning operation
   and request remain identifiable, and diagnostics do not freeze editing or expose
   datasource passwords. Test both synthetic helper failures and real invalid SQL.

## U03 — Optional downloadable Java runtime artifact

**Status:** done

**Depends:** D09. **Owns:** Java discovery/bootstrap, CI jlink artifact generation,
optional runtime installation UI and release metadata. **UAT:** M003.

**Read:** [integration checkpoint](../U03-INTEGRATION.md) and
[acceptance evidence](../../parity/U03.md). Production download/progress/cancel/
retry and actual no-Java H2 terminal/package separation are qualified on Linux
x86_64 GNU. Z03 qualifies the local distribution; Z04 retains final release
execution/publication qualification.

1. Build a separate, platform-specific Java 21+ jlink runtime artifact in CI and
   publish it with checksums alongside releases. Keep it out of the main app
   archive. Inspect the user's jlink-build work and integrate it without overwriting
   concurrent packaging changes. Retain flexible existing JDBC driver management.
2. Prefer configured/local Java. When none is usable, offer an explicit download
   action using the latest repository release's matching runtime artifact. Resolve
   release metadata once, verify its checksum, extract/install atomically in user
   storage, and use the installed external Java for the helper.
3. Keep download/progress/cancel/retry off the UI thread. Missing platform assets,
   unavailable networking and corrupt archives need readable recovery paths.
   Core editor startup remains independent of Java and network access.

**Accept:** isolated no-Java fixture accepts the offered runtime, launches external
   JDBC and executes SQL; decline/cancel/failure preserve the usable core. A working
   local JVM avoids downloads. Package size/manifests prove that the main app has
   no bundled JVM. Qualify actual generated artifacts, never infer platform support.

## U04 — Multiple cursors and selections

**Status:** done

**Depends:** E12, E07, E09. **Owns:** editor selection/mutation core, pointer and
keyboard reducers, search match commands and all shared editor hosts. **UAT:** B004.

**Continuation:** [integration contract and qualification](../U04-INTEGRATION.md).

1. Alt+left-click adds a cursor and toggles an existing cursor off, retaining a
   valid primary selection. Alt+mousedown/drag/release adds a selection.
2. Alt+Enter in file search creates cursors/selections for every found occurrence.
   Ctrl+Shift+Up/Down adds a cursor at the reached visual position, respecting
   wrapping, tabs, wide/combining characters and document boundaries.
3. Apply typing, deletion, paste and undo/redo consistently to normalized,
   non-overlapping selections in one edit transaction. Preserve read-only guards,
   completion ownership, scrolling and primary caret focus in embedded hosts.

**Accept:** deterministic edit/undo and actual PTY sequences cover add/remove,
   drag, all-occurrences, vertical cursors, overlapping/adjacent ranges, Unicode,
   folds/wraps and empty documents. Run through file, SQL and REST editable hosts;
   read-only hosts cannot gain a mutation path. U06 owns double-click gestures.

## U05 — Hex layouts and text or binary find

**Status:** done

**Depends:** V02, Z02. **Owns:** hex viewer layout, controls, search and fixtures.
**UAT:** M004.

1. Offer 8, 16 and 32 byte columns, defaulting to the current 16. Preserve selected
   byte offset, edit state and scroll position when changing layouts or resizing.
2. Ctrl+F opens find with plain-text and binary modes. Define UTF-8 text encoding
   and a clear hexadecimal byte input format, validate malformed input, and support
   next/previous matches with visible selection and no document mutation.
3. Keep narrow rendering bounded and large byte searches cancellable/responsive.

**Accept:** layout offsets, selection/edit behavior and searches match known binary
   fixtures, including NUL, UTF-8, matches across rows, EOF and no-match cases;
   invalid binary patterns have readable errors. Real terminal controls work for
   every layout and search mode without changing underlying bytes.

## U06 — Word selection with double-click and Alt+double-click

**Status:** done

**Depends:** U04. **Owns:** shared editor click timing, word boundaries and pointer
selection, including embedded editors. **UAT:** M005.

**Continuation:** [gesture/Unicode integration findings](../U06-INTEGRATION.md).

1. Double-click selects the entire word under the pointer using consistent Unicode
   word boundaries and existing cell-to-document mapping.
2. Alt+double-click adds that word selection and a cursor at its end. Coordinate
   the gesture with U04's Alt+click toggle so the first click does not discard or
   duplicate selections. Define punctuation/whitespace and repeated-click behavior.

**Accept:** fake-clock and real PTY pointer sequences cover ordinary/Alt double
   clicks, existing cursors, Unicode/tab/wrap/fold boundaries, resize and embedded
   editable/read-only hosts. Added selections participate in one undoable edit.

## U07 — File-tree picker buttons for path fields

**Status:** done

**Depends:** W05, D07, R07. **Owns:** shared picker widgets, asynchronous tree jobs
and path-field integration across file/project, database and REST dialogs.
**UAT:** M006.

**Continuation:** [completed path-field inventory and qualification](../U07-INVENTORY.md).

1. Inventory every TUI file/directory path input and add an adjacent picker button.
   Cover open/save/export/import, source roots, local driver JARs and REST body/
   multipart files where those fields exist. URLs and arbitrary command text are
   not filesystem paths. Record the complete inventory and any composite fields.
2. Extend/reuse the existing folder tree dialog for open-file, save-file and
   directory selection. Respect the field's base directory, existing-file versus
   new-target semantics, spaces/Unicode and current typed value.
3. Keep manual entry, keyboard access and cancellation; load trees through existing
   workers. Reject stale picker results after dialog/item/workspace changes.

**Accept:** every inventoried path input has a functional mouse/keyboard picker;
   selection updates the correct field and the actual operation uses that path.
   Cancel preserves typed text and focus. Test outside-root paths, errors, stale
   results, narrow sizes and save targets that do not exist yet.

## U08 — Project-relative paths in the open-file list

**Status:** done

**Depends:** W05, W03. **Owns:** open-file presentation and display preference.
**UAT:** B005.

1. Display physical open files relative to the project root by default; retain
   enough directory context to distinguish equal basenames. Offer full paths as
   an optional display mode. Define clear behavior for files outside the project
   and virtual documents.
2. Keep canonical file identity, open/save targets and persisted document ownership
   independent from display labels. Preserve the selected file across toggling.

**Accept:** nested files, duplicate names, outside-root files, virtual buffers,
   project switches and session restart show accurate labels; toggling full paths
   never opens/saves another file. Qualify the actual list in real PTYs.

## U09 — Repair language server installations

**Status:** suspend

**Depends:** P01, P02, Z02. **Owns:** shipped LSP catalog, download/package install
adapters and Settings installation diagnostics. **UAT:** B006, images 5–8.

**Continuation:** [catalog/installer audit and qualification plan](../U09-INTEGRATION.md).

1. Repair the broken JDT download and provide functional installation paths for
   TypeScript, VSCode HTML/CSS/JSON and clangd on supported targets, replacing the
   observed manual/no-download dead ends with actual install workflows.
2. Verify current upstream artifacts, versions, checksums, executable discovery
   and Node/Java/native prerequisites. Keep installed roots isolated, cancellable
   and recoverable; retain explicit local-executable overrides.
3. Wrap/scroll errors and expose actionable install/start failures in Settings.

**Accept:** isolated installs of the reported servers resolve actual executable
   paths and complete LSP initialize/shutdown; failing HTTP/package/dependency cases
   are readable and leave no partial installation. Qualify supported platform assets
   instead of treating a catalog row or manual instructions as a working installer.

## U10 — Panel shortcuts follow visual tab order

**Status:** done

**Depends:** Z02. **Owns:** global panel shortcuts, shared Help and terminal cases.
**UAT:** B007. This supersedes preserving the earlier semantic Alt+number mapping.

1. Map Alt+1..8 to Files, Project, Git, Database, REST, About, Settings, Help,
   matching the current horizontal tab order. All eight must work regardless of
   horizontal overflow, selected panel and normal editor focus.
2. Update generated Help, documentation and affected fixtures together.

**Accept:** actual terminal sequences activate every visual tab, including 5–8,
   at wide/narrow sizes. Mouse and keyboard select/focus the same panel; no shortcut
   inserts text or activates an unrelated host command.

## U11 — Connection-type JDBC URL template dropdown

**Status:** done

**Depends:** D07. **Owns:** datasource URL field, template catalog and suggestions.
**UAT:** M007, image 9.

1. Add an obvious mouse/keyboard dropdown beside JDBC URL, populated with common
   formats for the selected connection type and relevant recent URLs.
2. Changing type updates available templates without silently overwriting a typed
   URL. Selecting a template fills the correct field and preserves editable parts;
   support connection types with multiple URL syntaxes and meaningful placeholders.

**Accept:** real datasource dialogs select templates by mouse and keyboard for
   common catalog types, retain manual entry, cancel without changes and show the
   current type's suggestions after switching. Narrow layouts remain usable.

## U12 — Database package listing and DDL

**Status:** suspend

**Depends:** D09. **Owns:** JDBC helper metadata/DDL providers, bridge protocol and
explorer package nodes/actions. **UAT:** M008.

1. Inventory catalog engines with database package support and implement package
   listing, package members and source/DDL retrieval where applicable. Use explicit
   provider capabilities rather than inventing package nodes for unrelated engines.
2. Integrate lazy tree expansion, schema/catalog ownership, refresh/cancellation,
   readable errors and shared read-only DDL editors. Keep queries off the UI thread.

**Accept:** package-capable provider fixtures verify package names/members and
   actual DDL/source contents through the external helper and TUI; unsupported
   engines show accurate capability state. Record real vendor qualification and
   never count an unavailable database or fabricated DDL as a passing execution.

## U13 — Oracle refresh and SQL statement terminators

**Status:** done

**Depends:** D09. **Owns:** metadata refresh invalidation, SQL selection/script
boundaries and Oracle JDBC execution qualification. **UAT:** B009.

1. Reproduce creation of a table from another connection, then refresh the current
   explorer. Invalidate the relevant cached subtree and reject older in-flight
   completions so committed external DDL becomes visible without reconnecting.
2. Qualify single statements, explicit selections including their terminating
   semicolon, multiple statements and PL/SQL blocks against Oracle. Preserve
   internal block semicolons, comments, quotes and the user's exact editor text;
   adapt only the statement sent to the driver where required.
3. Cover schema/session ownership, refresh during loading, cancellation, errors,
   transaction state and the existing non-Oracle statement behavior.

**Accept:** actual Oracle JDBC and explorer replays show refreshed external DDL
   and correct single/selected/script execution, including PL/SQL blocks. Failed
   statements retain readable errors and are never automatically replayed.

**Evidence:** [U13 Oracle qualification](../../parity/U13.md).
