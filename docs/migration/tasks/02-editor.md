# Editor implementation tasks

Every task uses [EDITOR-UX.md](../EDITOR-UX.md) and [CONTRACTS.md](../CONTRACTS.md).
Tests named below refer to `../kode/src/test/kotlin/editor/`. The editable widget
must be reusable in file, search, SQL and REST hosts.

## E01 — Rope document and coordinate conversions

**Status:** done

**Depends:** F02. **Owns:** `kode-core/src/document/{text,positions,snapshot}.rs`.
**Read:** `editor/lib/{ITextBuffer,TextBuffer,BufferState}.kt`,
`editor/lang/TreeSitterSupport.kt`, `CodeEditorView.VisualLine`.

1. Wrap Ropey with document identity/revision and immutable snapshots. Define exact
   empty/trailing-line behavior and byte-range validation.
2. Implement byte↔scalar↔line/column↔UTF-16 adapters and valid-boundary clamping.
   Keep terminal cells out of document coordinates.
3. Supply iterator/slice APIs usable by layout/search/parsing without flattening
   the whole document on each render.

**Accept:** UX-001/017/018/051; round trips for ASCII, CRLF, tabs, emoji, combining
   and wide characters, end-of-line/file and legacy surrogate-interior offsets.
   Snapshot remains unchanged after editing its source document. Invalid ranges
   return errors; no partial UTF-8 bytes are exposed.

## E02 — Movement and selection commands

**Status:** done

**Depends:** E01. **Owns:** `kode-core/src/editor/{movement,selection}.rs`.
**Read:** `TextBuffer.kt` cursor/selection/word-boundary helpers and `ITextBuffer.kt`.

1. Implement left/right, logical up/down, Home/End and source-compatible word jumps.
2. Implement directional anchor/head selection, select-all, explicit selection,
   normalized extraction, and selection clearing on non-extending motion.
3. Provide a movement interface accepting layout-supplied page targets, so visual
   paging stays in E06 without duplicating source-position rules.

**Accept:** UX-008–013/017; reverse and multiline selection, short/empty lines,
   start/end edges, punctuation/whitespace words and Ctrl+Shift combinations. Do
   not change logical arrow movement to wrapped-row movement by accident.

## E03 — Editing, auto-indent, undo and redo

**Status:** done

**Depends:** E02. **Owns:** `kode-core/src/editor/{edit,history}.rs`.
**Read:** `TextBuffer.kt` mutate/history/newline/delete/restore helpers;
`lib/TextBufferUndoTest.kt`, `lib/TextBufferInputTest.kt`.

1. Implement insert/replace, selection deletion, backspace/delete joins and newline
   with copied leading indentation. Produce edit deltas and monotonic revisions.
2. Implement bounded history of edit transactions with cursor/anchor state; new
   edits clear redo. Add grouping for paste, completion and replace-all.
3. Expose versioned history export/import and a mutation capability guard used by
   read-only hosts. Document history limit behavior rather than silently dropping
   imported state.

**Accept:** UX-002–007/014–016; exact undo text and selections, EOF deletion bounds,
   multi-line replacement and indentation. Dirty state reflects current content
   relative to saved state; rejected/no-op edits do not create false history.

## E04 — File I/O, save state and clipboard

**Status:** in-progress

**Depends:** E03, F03. **Owns:** `kode-core/src/document/{encoding,save}.rs`,
`kode-platform/src/clipboard.rs`, `kode-tui/src/editor/loading.rs`.
**Read:** `CodeEditorView.kt:198–309`, `TextBuffer.saveToFile`, `SystemClipboard.kt`.

1. Implement async file read with request identity and explicit load failure.
   Define UTF-8/BOM/newline handling and optional encoding_rs use. Save snapshots
   through atomic file replacement while respecting permissions/symlinks policy.
2. Track saved revision versus current revision. Detect changed disk identity and
   keep dirty state on failure; never treat a virtual label as a file path.
3. Implement injectable arboard/internal clipboard service and both Ctrl/Alt
   shortcuts; handle bracketed paste as one edit. Optional OSC 52 copy must have
   a bounded, explicit capability path.

**Accept:** UX-019–025/031/032; late load/save completion, missing/denied files,
   CRLF/BOM/final-newline round trips and clipboard unavailable. Copy uses raw text,
   not tab/escape rendering. Dirty newer edits survive a completed earlier save.

## E05 — Visual layout and editor cell rendering

**Status:** todo

**Depends:** E02, F05. **Owns:** `kode-tui/src/editor/{layout,render,display}.rs`.
**Read:** `CodeEditorView.kt:309–613,1906–1958,2230–2296,2863–2991`;
`ui/CodeEditorViewTest.kt` wrapping/tab/control/wide-character cases.

1. Build one layout map from source bytes to display runs, wrapped rows, gutter
   cells and inverse hits. Preserve fixed four-cell tabs and escaped glyph defaults.
2. Render header/dirty/MIME/grammar labels, line numbers, text, selection and caret
   into Ratatui cells. Add extension points for overlays, folds, minimap and popups.
3. Cache by document revision, viewport width and fold/display state; limit work to
   visible rows plus required layout indexes. Handle cursor exactly at a wrap edge.

**Accept:** UX-033–035/040–042/048; empty lines, narrow widths, long lines, control
   escape wrapping and selections crossing wraps. Render never reads files or
   calls services. Drawn source has no missing/duplicated punctuation.

## E06 — Editor input, hit testing, scrolling and focus

**Status:** todo

**Depends:** E03, E05, F04. **Owns:** `kode-tui/src/editor/input.rs`.
**Read:** `CodeEditorView.kt:614–869,1715–1776`; page/wheel/focus regressions.

1. Route normalized commands and mouse events using the shared layout. Implement
   press/drag/release selection with captured owner and header/gutter boundaries.
2. Implement visual PageUp/PageDown with optional selection; 3/9-row wheel scrolling;
   ensure-caret-visible rules with search/header offsets.
3. Integrate focus/popup/search/minimap hooks without duplicating coordinate math.
   Preserve modifier flags through every nested viewport.

**Accept:** UX-035–038/048–053; page within one wrapped logical line, reverse drag
   across wraps, clicks on tab/escape cells, release outside pane, and tiny viewport.
   Mouse scroll does not unexpectedly move caret; popup capture prevents edits.

## E07 — Regex find/replace and search bar

**Status:** todo

**Depends:** E03, E05, E06, F05. **Owns:** `kode-core/src/editor/search.rs`,
`kode-tui/src/editor/search_bar.rs`.
**Read:** `TextBuffer.kt` search/replacement helpers, `SearchReplaceBar.kt`,
`CodeEditorView.kt:1870`; `TextBufferSearchTest`, `SearchReplaceBarTest`.

1. Implement regex matching with explicit Java-compatibility translation/errors,
   multiline spans, active match, counts and non-progress protection. Use regex
   fast path and fancy-regex only where supported constructs require it.
2. Implement `$n` replacement semantics, current/all replacement, grouped undo,
   invalid-pattern styling and match refresh after edits/history operations.
3. Port two-field bar, Ctrl+F escaped selection prefill, field caret/clipboard,
   Tab/mouse focus, Enter/Ctrl+Enter/Ctrl+R/Ctrl+Shift+R and `[x]` close.

**Accept:** UX-039/062–072; preserved uppercase, captures and literal dollar/escape
   cases, multiline matches, zero-width patterns, no matches, read-only host and
   stale search result rejection. Document unsupported Java syntax with useful errors.

## E08 — Folding and Braille minimap

**Status:** todo

**Depends:** E05, E06. **Owns:** `kode-tui/src/editor/{folding,minimap}.rs`.
**Read:** `CodeEditorView.kt:1961–2039,2230–2465`; folding tests.

1. Port brace fold detection excluding quoted/commented braces and current
   indentation-language rules; keep regions and collapsed state separate.
2. Render gutter toggles and first/ellipsis/last summary. Implement marker/ellipsis
   hits and expansion on navigation; invalidate anchors after edits/reloads.
3. Port minimap Braille masks, sizing, caret-centering/highlight and click jump using
   explicit source-block mapping. Cover the existing line-number off-by-one risk.

**Accept:** UX-054–061; nested folds, changed/removed blocks, hidden selection
   targets, minimap boundaries with search/wrap/resize. No source text mutation.

## E09 — Completion popup and insertion

**Status:** todo

**Depends:** E03, E05, E06. **Owns:** `kode-tui/src/editor/popups/completion.rs`.
**Read:** `CodeEditorView.kt:1040–1166,1273–1302,1566–1715,2579–2648`;
completion-related `CodeEditorViewTest` cases.

1. Use the shared intelligence DTO/provider interface with a fake provider first;
   support external host suggestions, explicit versus auto-show, details/dedup and
   local fallback rules. Real provider wiring is L09/P03.
2. Implement scrolling, keys, hit targets, mouse wheel/track and dismissal behavior.
   Associate requests with caret context and dismissal generation.
3. Apply replacement range or typed-prefix fallback as one edit; preserve receiver
   operators and text after caret. Honor insertion text when distinct from label.

**Accept:** UX-077–084/098/099; entries beyond first viewport selectable, empty prefix
   after dot, typed-prefix-only replacement, stale response, read-only rejection and
   auto-show disabled. No semantic engine prerequisite for widget tests.

## E10 — Definitions, usages and hover preview

**Status:** todo

**Depends:** E05, E06, F03. **Owns:** `kode-tui/src/editor/popups/{definition,usages}.rs`.
**Read:** `CodeEditorView.kt:869–1040,1166–1273,1302–1715,2502–2579,2649–2862`.

1. Implement Ctrl-click reference/declaration routing and lexical identifier fallback.
   Use fake navigation/intelligence services to prove all interaction branches.
2. Implement 500 ms hover candidate timer; current-buffer/async-file preview sources,
   24-line preview, multiple targets, navigation arrows/location links and missing source.
3. Implement usage list keyboard/wheel/scrollbar-drag behavior, compact labels,
   popup boundary clipping and distinct keyboard capture rules. Dismiss immediately
   and invalidate queued requests on outside click/Escape/context changes.

**Accept:** UX-085–099; all existing popup regressions, every returned reference
   reachable, hover-to-popup travel, list longer than viewport, missing targets,
   and old response after dismiss. No sync source file reads during render.

## E11 — Highlight overlays and edit remapping

**Status:** todo

**Depends:** E05, E07, L02, L09. **Owns:** `kode-tui/src/editor/overlays/`.
**Read:** `HighlightEdits.kt`, `CodeEditorView.kt:375–613,1906–1958,2039–2230`,
`HighlightEditsTest`, `SemanticHighlightingTest`.

1. Compose lexical/semantic styling, hover, search/active match, tab marker,
   warning/transient ranges, selection and cursor in a documented order.
2. Apply edit deltas to cached spans; drop touched spans including token-joining
   edits at boundaries. Re-map again across successive edits before analysis ends.
3. Refresh on current semantic revision and dependent-file publication. Reject
   old text/index versions; repaint through events without polling input.

**Accept:** UX-043–047; non-overlapping draw segments, source punctuation preserved,
   tab/Unicode offsets, inserted/deleted lines and references changed in another file.

## E12 — Reusable editor hosts and virtual/read-only documents

**Status:** todo

**Depends:** E04, E07, E08, F05. **Owns:** `kode-tui/src/editor/{host,mod}.rs`.
**Read:** public CodeEditorView hooks; ProjectSearchDialog, SqlConsoleManager,
RestEditorView and RestKeyValueDialog integration sites.

1. Expose the EditorHost API with fake/test hosts for file, SQL, search, REST and
   response documents. Completion/navigation hooks may initially return no result.
2. Centralize save target, mutation capability, warning lines, transient selection,
   language and host callback handling. UI label is never a filesystem identity.
3. Define scoped shortcut ownership, focus transfer and coordinate translation for
   nested widgets. Later feature cards attach their actual models/services.

**Accept:** UX-025/100–106 using fake hosts; response cannot change through replace,
   cut, paste, completion or programmatic APIs. Shared clipboard and modifier
   routing work; Ctrl+Enter respects focused search field before host execution.

## E13 — Complete editor interaction qualification

**Status:** todo

**Depends:** E09, E10, E11, E12, W04, W05, P03, D08, R07, G03.
**Owns:** `kode-tui/tests/editor_interactions/`, `docs/parity/editor-results.md`.
**Read:** all UX rows and original editor/input/search/highlight/embedded-editor tests.

1. Convert remaining recorded interactions to deterministic event replays using
   test backend, fake clock and controlled service completions. Assert state,
   effects, focus, view/cell maps and cells, not screenshots alone.
2. Run identical sequences through every real editor host. Exercise popup/search
   priority against host shortcuts, stale loads, resize, and background publications.
3. Run PTY/manual terminal matrix, record performance on the fixture corpus, and
   close every UX row with evidence or a named blocking defect.

**Accept:** all UX-001–110 covered; no implemented Kotlin behavior missing and no
   silent UX substitution. Rendering/typing remains responsive while Git, SQL,
   REST and indexing jobs run. E13 is a release blocker, not optional polish.
