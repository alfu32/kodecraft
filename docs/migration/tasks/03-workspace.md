# Workspace and project tasks

## W01 — Source roots, exclusions and project scanning

**Status:** done

**Depends:** F03. **Owns:** `kode-workspace/src/{scanner,project,exclusions}.rs`.
**Read:** `editor/app/ProjectFileScanner.kt`, `editor/lib/ProjectFolderStatus.kt`,
Main's source-root/exclusion helpers; ProjectFileScannerTest and ProjectFolderStatusTest.

1. Port project configuration and default roots: only existing `src`, `lib`,
   `include`. Scan only configured roots, applying Git ignore and explicit file/
   folder exclusions. Deduplicate overlapping roots and canonical file identities.
2. Use `ignore` with explicit policy for hidden files, symlinks, unreadable
   directories, and metadata exclusions. Do not inherit crate defaults blindly.
3. Emit cancellable scan batches and separate scan progress from indexing progress.

**Accept:** original scanner/status fixtures plus nested ignore negation, relative/
   absolute roots, exclusions, unreadable paths, overlapping roots, cancellation,
   and generation discard on workspace switch. Tree/search policies remain distinct.

## W02 — File tree, file actions, refresh and project panel

**Status:** done

**Depends:** W01, F05, E04. **Owns:** `kode-workspace/src/tree.rs`,
`kode-tui/src/panels/{files,project}/`, `widgets/folder_picker.rs`.
**Read:** `FileTree.kt`, `IFileTree.kt`, `FileTreeView.kt`, `FilesTabView.kt`,
`ProjectSettingsView.kt`, `SourceFolderPickerDialog.kt`, `RescanProgressDialog.kt`.

1. Build lazy expanded-node state, sorted flattened entries, size/count labels,
   status classification and [+]/[-]/[=] markers. Hide Kode metadata appropriately.
2. Port root `[change][+d][+f]`, hovered row rename/create/delete actions and modal
   text entry. Keep recent-file controls (including removal indicator) distinct
   from the tree. Reconcile open document paths after file operations.
3. Implement notify/coalesced refresh plus roughly five-second fallback. Preserve
   expansion, selection and scroll. Add source-root/exclusion editing and rescan
   progress/cancellation controls.

**Accept:** FileTreeTest plus keys/mouse/scroll in tree and recents, filename spaces,
   failed rename/delete, open-file moves, external refresh and picker focus. User
   edits survive background refresh. UX-030/031/110.

## W03 — Legacy session import and Rust persistence

**Status:** done

**Depends:** E03, F03. **Owns:** `kode-workspace/src/session/`,
`fixtures/session/`; schema in `docs/formats/session-v1.md`.
**Read:** `ProjectSession.kt`, `BufferState.kt`, Main's restore/persist/mtime helpers;
ProjectSessionManagerTest; database and REST serialized state models.

1. Specify `.kode-rs.json` schema and defaults. Read version-6 `.kode.json` using
   camelCase DTOs with absent defaults and preserved unknown feature fields.
2. Convert legacy line/UTF-16 cursor and anchor positions in current buffer and
   every undo/redo snapshot. Import recents/viewer kinds, dirty text, grammar,
   visual scroll, roots/exclusions, REST document/environments and SQL sessions.
3. Implement three-second debounce, atomic replace/recovery, shutdown flush,
   stale-mtime handling with recovery copy, relative-path resolution and corrupt/
   future-version feedback. Never overwrite legacy session files automatically.

**Accept:** UX-018/026–028/108; clean/dirty history round-trip including emoji,
   missing/default fields and unsupported future versions. Corrupt input remains
   recoverable. Failed writes retain dirty session state. Unknown Postman fields
   survive even before the REST feature is implemented.

## W04 — Project search modal and embedded editor

**Status:** done

**Depends:** W01, E07, E12, F05, V01. **Owns:**
`kode-workspace/src/search.rs`, `kode-tui/src/panels/project_search/`.
**Read:** `ProjectSearcher.kt`, `ProjectSearchDialog.kt`, Main's search wiring.

1. Preserve whole-project search behavior and file-regex filtering separately from
   source-index scope. Use cancellable background traversal; never read binary
   files indiscriminately or block the UI. Record the exact scope in help.
2. Port centered modal with search/replace fields, extension-prefilled filter,
   syntax-colored hit list, hit markers and editor preview. Preserve divider drag,
   list keys/wheel and focus transitions.
3. Open selected hit using shared document identity, center match, save through
   editor service and update recents/results. Implement the actual source's replace
   actions; do not claim bulk cross-file replacement merely because a bar exists.

**Accept:** UX-073–076/100; invalid file regex, no results, unreadable file, late
   searches, match ranges after edits, mouse selection and clipboard in preview.

## W05 — CLI targets and workspace application shell

**Status:** done

**Depends:** F04, F05, W02, W03, E12, V01. **Owns:** `kode/src/main.rs`,
`kode/src/cli.rs`, `kode-tui/src/app/{state,workspace,dispatch}.rs`.
**Read:** Main startup parsing/layout/focus/session sections, WorkspacePickerDialog.

1. Implement `kode <file>`, `kode <folder>`, no-target help, explicit-file precedence
   and root selection. Use clap; retain room for `serve` implemented by Z01.
2. Compose sidebar tabs, right viewer, draggable splitter, splash/scan status,
   active focus and recents. Wire editor save/open/navigation and project picker.
3. Centralize workspace switching and shutdown: persist old state, cancel/close
   services, increment generation, restore new state, then launch new jobs.
   Feature panels attach through typed commands, not Main.kt-style coupling.

**Accept:** startup file/folder/missing argument/error fixtures, UX-026/029/108,
   no stale jobs from previous workspace, correct active viewer after navigation,
   and shell layout usable before optional services are available.
