# Code editor: functional and interaction parity contract

The editor is a first-class deliverable. Every row below needs an implementation
and evidence before E13/Z04 can pass. Rendering text and accepting typing is not
sufficient. Required robustness cases are identified explicitly where they go
beyond a demonstrated Kotlin test. No row is implemented by this planning work.

## Source of truth and audit anchors

Source paths below are under `../kode/src/main/kotlin/`; line anchors refer to the
audited commit and will change in future revisions.

| Source | Read for |
|---|---|
| `editor/lib/ITextBuffer.kt` | Full command surface; key normalization; Ctrl and Alt clipboard bindings |
| `editor/lib/TextBuffer.kt` | Navigation, newline indentation, selection, history, regex replacement, persistence |
| `editor/lib/BufferState.kt` | Cursor, anchor, dirty text, undo/redo snapshots |
| `editor/ui/CodeEditorView.kt:116` | Host hooks, file/virtual loads, captured/restored state |
| `editor/ui/CodeEditorView.kt:309` | Header, gutter, overlays, cursor, minimap, popup rendering |
| `editor/ui/CodeEditorView.kt:614` | Complete event priority and main keyboard/mouse dispatch |
| `editor/ui/CodeEditorView.kt:869` | Ctrl-click definitions/usages and lexical fallback |
| `editor/ui/CodeEditorView.kt:1040` | Completion insertion and popup selection |
| `editor/ui/CodeEditorView.kt:1166` | Popup wheel/drag/outside-click capture |
| `editor/ui/CodeEditorView.kt:1464` | Delayed definition preview, target source loading |
| `editor/ui/CodeEditorView.kt:1566` | Popup keyboard rules |
| `editor/ui/CodeEditorView.kt:1736` | Wrapped mouse coordinate mapping |
| `editor/ui/CodeEditorView.kt:1870` | Search opening/actions and selection prefill |
| `editor/ui/CodeEditorView.kt:1906` | Token/selection/search composition |
| `editor/ui/CodeEditorView.kt:1961` | Minimap width and Braille layout |
| `editor/ui/CodeEditorView.kt:2230` | Wrapping, brace/indent folding and expansion |
| `editor/ui/CodeEditorView.kt:2502` | Usage/completion/definition popup bounds and styles |
| `editor/ui/CodeEditorView.kt:2863` | Tab/control/wide-character source-to-cell map |
| `editor/ui/HighlightEdits.kt` | Highlight remapping while new analysis is pending |
| `editor/ui/SearchReplaceBar.kt` | Two fields, caret, validation, key/mouse focus, search actions |
| `editor/ui/ProjectSearchDialog.kt` | Embedded editor, result list, filter, draggable divider |
| `editor/app/Main.kt` | Global focus, viewer changes, recents, SQL/REST host dispatch, save/restore |

Existing regression suites are `TextBufferInputTest`, `TextBufferUndoTest`,
`TextBufferSearchTest`, `CodeEditorViewTest`, `SearchReplaceBarTest`,
`HighlightEditsTest`, `RestEditorViewTest`, and the semantic tests listed in
COMPATIBILITY.md. E13 adds event-sequence coverage where the source has no test.

## Text, cursor, selection, and history

| ID | Input / situation | Required visible and model behavior | Owner |
|---|---|---|---|
| UX-001 | Empty file/document | One editable logical line, cursor at start; no indexing/render panic | E01 |
| UX-002 | Type printable characters with Shift | Preserve actual character case; insert at caret; update dirty marker and revision | E03 |
| UX-003 | Enter / Return / CR / LF key variants | One newline plus current line's leading spaces/tabs; consistent in embedded editors | F04/E03 |
| UX-004 | Multiline paste over forward/reverse selection | Replace selected range once, preserve prefix/suffix, place caret after inserted text | E03/E04 |
| UX-005 | Backspace within line / at line start | Delete preceding character, or join preceding line; no-op at document start | E03 |
| UX-006 | Delete within line / at line end | Delete next character, or join next line; no-op at document end | E03 |
| UX-007 | Delete/backspace/cut with selection | Delete complete normalized range, join boundary lines, clear selection | E02/E03 |
| UX-008 | Left/Right | Move within line and across line boundaries; never split UTF-8 | E02 |
| UX-009 | Up/Down | Preserve original logical-line navigation and clamp to short lines; top goes to first-line start, bottom to last-line end | E02 |
| UX-010 | Home/End | Logical line start/end; Shift extends from anchor | E02 |
| UX-011 | Ctrl+Left/Right | Preserve source word-boundary rules for identifiers, punctuation, whitespace, and line boundaries | E02 |
| UX-012 | Shift+Arrows / Ctrl+Shift+Arrows | Extend/reverse selection without losing anchor; moving without expansion clears it | E02 |
| UX-013 | Ctrl+A | Entire text selected, including multiline boundaries; empty document safe | E02 |
| UX-014 | Alt+U / Alt+R | Undo/redo text and cursor/anchor state; new edit clears redo | E03 |
| UX-015 | Undo after newline, multiline paste, replace-all, completion | Each intentional compound edit is atomic; restores exact content | E03/E07/E09 |
| UX-016 | Undo/redo when history empty or at EOF | No panic, no phantom dirty edit, cursor remains valid | E03 |
| UX-017 | Tab/control/supplementary characters | Logical positions refer to source characters, not the multiple display cells used by escapes | E01/E02/E05 |
| UX-018 | Clone/snapshot/restored state | Independent mutable views/history; immutable worker snapshot remains stable | E01/W03 |
| UX-019 | Ctrl+C/X/V and Alt+C/X/V | Copy/cut selected text, paste at caret; read-only cuts/pastes rejected; shared clipboard in all hosts | E04/E12 |
| UX-020 | Clipboard unavailable / SSH / browser terminal | Internal fallback works; bracketed paste accepted; copying does not freeze input waiting for a terminal query | E04/F04 |

## Document loading, saving, and lifecycle

| ID | Input / situation | Required behavior | Owner |
|---|---|---|---|
| UX-021 | Open a text file | Async load, correct path/MIME/language/grammar header, initial cursor/scroll, repaint on completion | E04/V01 |
| UX-022 | Open file B before A finishes | A's result never replaces B; workspace switch likewise invalidates A | E04/F03 |
| UX-023 | Read fails or file disappears | Visible error; do not silently present an empty file that can overwrite the original | E04 |
| UX-024 | Ctrl+S on file document | Save intended snapshot; clear dirty only for that revision; save failure remains dirty and visible | E04 |
| UX-025 | Ctrl+S on virtual/read-only document | Route to host's defined save/export action or report unavailable; never write a label as a filesystem path | E12 |
| UX-026 | Switch files/viewers and return | Retain document text, cursor, selection, history, scroll, and language state | W03/W05 |
| UX-027 | Restart with unsaved state | Restore dirty text, undo/redo, position and view state from session | W03 |
| UX-028 | On-disk mtime newer than saved session | Apply documented stale-state handling; retain recoverable dirty content instead of silently destroying it | W03 |
| UX-029 | File target supplied at startup | Open requested file; its parent is root; do not replace it with last session's active file | W05 |
| UX-030 | Rename/delete/move an open file | Reconcile document identity, recent entries, index and save target; surface missing-file state | W02/W03 |
| UX-031 | File changes while buffer dirty / save fails | Keep recoverable user content; explicit conflict/error state, no hidden overwrite | E04/W02 |
| UX-032 | LF/CRLF/BOM/last newline/no final newline | Preserve documented file format; no extra newline introduced by repeated load/save | E04 |

UX-023, UX-025, UX-028, UX-031 and exact-byte saving strengthen failure handling
where the source is permissive. F01 records them as intentional corrections, not
as evidence that the Kotlin build already provides those guarantees.

## Rendering and viewport

| ID | Input / situation | Required behavior | Owner |
|---|---|---|---|
| UX-033 | Normal render | Dirty marker, path, MIME, language/grammar label, line numbers and gutter align | E05 |
| UX-034 | Long logical line | Soft wrapping with a single source/cell map shared by drawing and hit testing | E05 |
| UX-035 | PageUp/PageDown on wrapped line | Move cursor and viewport by visible rows; retain column within wrapped segment | E05/E06 |
| UX-036 | Shift+PageUp/PageDown | Same visual-row movement while extending selection | E06 |
| UX-037 | Mouse wheel / Alt+wheel | Scroll 3 / 9 visual rows per step, clamp, and preserve cursor until another navigation action | E06 |
| UX-038 | Cursor leaves viewport by navigation/edit | Scroll just enough to reveal it; obey header/search heights | E06 |
| UX-039 | Open/close search bar | Two-row find/replace area shrinks/restores editor viewport; no coordinate drift | E07 |
| UX-040 | Resize narrow/tiny terminal or splitter | Recompute wrap/gutter/minimap/popups safely; zero content rows/columns do not underflow | E05/F05 |
| UX-041 | Tab in source | Display `|-->` in four cells with tab style; all four map to one logical source tab | E05 |
| UX-042 | Control/wide/non-BMP/combining scalar | Existing escaped presentation, safe clipping; typed/pasted raw content survives save | E01/E05 |
| UX-043 | Overlapping lexical and semantic spans | No overlapping draw segments, duplicated punctuation, shifted source, or invalid byte slicing | E11 |
| UX-044 | Selection, search, hover, warning, transient overlays | Deliberate precedence; selection remains clear over tokens; warnings/transient SQL highlight remain visible | E11/E12 |
| UX-045 | Semantic job finishes without new input | Repaint immediately; no keypress required | F03/E11 |
| UX-046 | Type while semantic job pending | Shift unchanged spans, discard edited/intersecting spans, then replace with current result | E11 |
| UX-047 | Only a referenced file changes | Semantic highlights/completion refresh for affected dependent editor | L08/L09/E11 |
| UX-048 | Focus leaves editor | Correct cursor/selection/focus appearance; keystrokes go to actual focused target | F05/E06 |

The test named `tabsRenderAtFourColumnStopsWithoutShiftingTokens` does not settle
the tab policy: the current `VisualLine` code explicitly draws a fixed four-cell
marker. F01 captures actual cells and E05 keeps the observed default. Native
Unicode glyph display, alternative tab stops, and different arrow-navigation modes
may be later options, but must not silently replace the baseline.

## Mouse, focus, and folding

| ID | Input / situation | Required behavior | Owner |
|---|---|---|---|
| UX-049 | Click source text | Focus editor, leave search-field focus, set caret using wrapped row and rendered-column mapping | E06 |
| UX-050 | Drag selection in either direction | Anchor on press, extend on move, stop on release; bounds clamp and header clicks do not edit | E06 |
| UX-051 | Click inside a tab/escape or after line end | Deterministic valid source boundary; same location used for edit/navigation/highlight | E01/E06 |
| UX-052 | Mouse modifiers inside nested panels | Preserve Ctrl/Alt/Shift/meta and translate coordinates exactly once | F05/E06/E12 |
| UX-053 | Drag across pane boundaries then release | Mouse capture belongs to original drag until release; another pane must not keep dragging | F05/E06 |
| UX-054 | Click brace-block gutter marker | Toggle matching multiline fold; ignore braces in strings and comments | E08 |
| UX-055 | Python/YAML/Bash/PowerShell/Ruby indentation fold | Preserve existing detection rules and empty-line handling; nested blocks deterministic | E08 |
| UX-056 | Collapsed region render/click ellipsis | First-line + ` ... ` + last-line summary; ellipsis and marker both expand | E08 |
| UX-057 | Navigate into hidden line | Expand containing folds until caret is visible; preserve text and selection | E08/E06 |
| UX-058 | Edit/remove a fold region or reload file | Invalidate stale fold anchors; no hidden unrelated lines or obsolete region retained | E08 |
| UX-059 | Braille minimap | Source-shape preview centered on caret; highlight caret block; hide below 40 total columns | E08 |
| UX-060 | Click minimap | Jump to represented source block/line, update caret and viewport; verify zero/one-based boundary | E08 |
| UX-061 | Minimap with search/wrap/folds/resizing | Click positions stay consistent; minimap width follows existing clamp (8–20 columns, roughly one-third) | E08 |

## Find and replace

| ID | Input / situation | Required behavior | Owner |
|---|---|---|---|
| UX-062 | Ctrl+F with/without selection | Open/focus find field; prefill escaped selected text when present; preserve existing query otherwise | E07 |
| UX-063 | Type/edit find/replace text | Preserve uppercase; Left/Right/Home/End/Backspace/Delete and clipboard operate on focused field | E07/F05 |
| UX-064 | Tab / mouse field click | Switch between find and replace; bounded field, explicit in-field caret and focus styling | E07 |
| UX-065 | Enter | Find next, cycle through matches, select/reveal current match | E07 |
| UX-066 | Ctrl+Enter | Find all; show all match spans and count | E07 |
| UX-067 | Ctrl+R / Ctrl+Shift+R | Replace current / all using capture groups (`$1`, `$2`, etc.); one undo unit per action | E07/E03 |
| UX-068 | Invalid regex | Error styling/message and no destructive replacement; editing fixes error without reopening | E07 |
| UX-069 | Multiline regex match | Highlight correct segments on each logical/wrapped line | E07/E11 |
| UX-070 | Empty/zero-width/no-match regex | No infinite loop; preserve source's exclusion of zero-width search highlights and define replacements consistently | E07 |
| UX-071 | Edit/undo/redo while search state active | Refresh count/spans/active index; never apply a stale match range | E07 |
| UX-072 | Click `[x]` | Close bar and restore editor focus/space; preserve buffer-owned search state | E07 |
| UX-073 | Alt+F project search | Centered modal, file-regex filter seeded from extension, result list above full embedded editor | W04 |
| UX-074 | Select search hit via keys/mouse/wheel | Correct active/marked hit, syntax colors, match highlight, centered editor location | W04 |
| UX-075 | Drag result/editor divider; edit/save result | Both regions resize and keep focus; Ctrl+S updates file and recents | W04/E12 |
| UX-076 | Search/filter error, no results, unreadable file | Bounded feedback; editor remains usable; no stale hits from previous query | W04 |

Do not assume Escape closes the find bar: its current explicit close control is
`[x]`, whereas Escape dismisses editor popups. Any added shortcut must be recorded
as an addition without removing the existing binding or changing focus priority.

## Completion, navigation, and popups

| ID | Input / situation | Required behavior | Owner |
|---|---|---|---|
| UX-077 | Ctrl+Space / terminal NUL variant | Open context completion list; empty result closes it cleanly | F04/E09 |
| UX-078 | Completion sources | External host suggestions first; provider results with details; dedup labels; cap baseline list at 50 | E09/L09 |
| UX-079 | No semantic result outside member context | Local identifier fallback; no unrelated local/workspace fallback after a receiver dot | E09/L07 |
| UX-080 | External provider auto-show disabled | Suggestions appear only on explicit request; REST/header contexts keep their configured behavior | E09/E12 |
| UX-081 | Completion Up/Down/Page/Home/End | Selected item stays visible through list scrolling; long list entries remain reachable | E09 |
| UX-082 | Completion Enter or row click | Insert selected suggestion, replace only typed prefix, preserve `.`/`?.`/receiver and suffix | E09 |
| UX-083 | Completion wheel/scrollbar click | Scroll inside popup; valid hit bounds and selection; wheel outside follows editor behavior | E09 |
| UX-084 | Completion Escape/outside click/edit/scroll | Dismiss at correct time; outside click consumed, no accidental edit beneath it | E09/E06 |
| UX-085 | Ctrl-click reference | Navigate to definition using source identifier fallback if semantic coloring is not ready | E10/L09/P03 |
| UX-086 | Ctrl-click declaration | Show every returned reference in usage popup, with project-relative compact path and line snippet | E10 |
| UX-087 | Hover an identifier 500 ms | Bounded syntax-colored Definition preview; timer fires even without more mouse movement | E10/F03 |
| UX-088 | Hover preview multiple targets | `<< n/T >>` arrows with exact hit boxes and hover feedback; clamp target index | E10 |
| UX-089 | Click preview location | Navigate selected target at correct line/column; one-based label and project-relative filename | E10 |
| UX-090 | Preview source missing | Show `source unavailable`, retain target navigation and controls | E10 |
| UX-091 | Preview source current unsaved file | Use current buffer; declaration-range preview, line fallback, maximum 24 source lines | E10 |
| UX-092 | Move from identifier into popup | Popup owns pointer events; editor selection/hover must not steal them | E10/E06 |
| UX-093 | Usage Up/Down/Page/Home/End/Enter | Select/page all results and navigate; viewport follows selection | E10 |
| UX-094 | Usage wheel/scrollbar track/drag | Capture wheel while usage popup is open; track click repositions then drag starts from new offset | E10 |
| UX-095 | Usage popup other typing | Consume keys while it is active; do not edit underlying buffer | E10 |
| UX-096 | Definition-only popup keyboard | Escape dismisses; other keys retain original editor routing; distinguish from usage popup capture | E10 |
| UX-097 | Outside click / Escape | Dismiss all relevant popup state immediately, including before next repaint; no click-through | E09/E10 |
| UX-098 | Popup near right/bottom edge or tiny viewport | Clip/reposition controls and source preview; keep valid scrolling and navigation | E09/E10 |
| UX-099 | Late definition/completion response | Drop result after cursor/context/revision/workspace changes; do not resurrect dismissed popup | E09/E10/F03 |

## Embedded editors and application integration

| ID | Host / situation | Required behavior | Owner |
|---|---|---|---|
| UX-100 | Project-search preview editor | Full selection, movement, typing, clipboard, search, save and syntax behavior | E12/W04 |
| UX-101 | SQL console | Same editor; selection/current statement/all execution; transient SQL target highlight; results stay independently focusable | E12/D08 |
| UX-102 | REST body/headers/environment editors | Same editing, modifier propagation and clipboard; header completion and duplicate warnings | E12/R06/R07 |
| UX-103 | REST response/raw/transaction and Git historical previews | Read-only mutation guard covers all paths; navigation, selection, copy and scrolling work | E12/G03/R07 |
| UX-104 | Header editor focused while transaction/result visible | Arrow keys move editor caret; response/result pane must not steal them | E12/R07/D08 |
| UX-105 | Dialog field → OK → Cancel | Distinct keyboard/mouse targets, visible bounds/caret and focus; buttons do not edit field | F05 |
| UX-106 | SQL/REST Ctrl+Enter vs search Ctrl+Enter | Focused find bar gets find-all; owning console/request executes only when host owns the key | F05/E12/D08/R07 |
| UX-107 | Ctrl+T shell escape and return | Suspend terminal input, release raw/mouse/alternate mode; resume/redraw with editor state intact | F04/Z01 |
| UX-108 | Global Ctrl+Q / workspace change | Persist pending session state and dispose jobs/hosts; dirty content remains recoverable | W03/W05 |
| UX-109 | Background index/Git/HTTP/SQL refresh | No typing stalls, cursor jump, focus theft, or invalidated selection | F03/E13 |
| UX-110 | Source roots/exclusions or index clear/rebuild | Progress/status distinguish scanning from resolution/publication; editor remains usable | W01/L09/Z02 |

## Qualification method

For each row, record its fixture, event sequence, expected text/cursor/selection,
focus owner, viewport, emitted effects and representative Ratatui cells. Test at
80×24 and 120×40, then stress at 20×6 and 1×1. Relevant rows also run with tabs,
control escapes, Unicode, reverse selections, wraps, and collapsed nested folds.
Use fake clock/clipboard/intelligence/I/O services for deterministic state tests.

E13 must replay the same core sequence in a file editor, search preview, SQL
console, REST body/header/environment editor, and a read-only response editor.
Backend state tests are complemented by PTY tests and real-terminal checks for
mouse drag, bracketed paste, modifiers, SSH/tmux, resize, and shell suspension.
Screenshots alone cannot prove text preservation, event capture, or stale-job safety.

F01 additionally inventories every event branch in the audited source and links
it to a UX row. Any newly discovered branch gets a new row before migration work
is called complete. Tab insertion variants, word boundaries, trailing-newline
handling, and browser shortcut limitations deserve explicit baseline recordings.
Do not infer unimplemented IDE features such as multi-cursor editing, Vim modes,
debugging, or a complete refactoring engine from the word “IDE.” Preserve all
existing functionality and label additions separately.
