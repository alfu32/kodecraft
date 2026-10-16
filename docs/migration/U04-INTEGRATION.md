# U04 multiple-cursor integration checkpoint

Status: in-progress. U04 is not complete and interactive multiple-cursor support
is not yet claimed. Complete U04/U06 and the other Uxx cards before resuming Z03's
[expanded artifact matrix](Z03-ARTIFACT-MATRIX.md).

## Implemented core

`CaretState` retains a primary `selection` plus `secondary` selections, with empty
serde defaults. Normalization validates every UTF-8 byte boundary, merges
overlapping ranges and duplicate/contained carets, preserves the primary group's
direction and keeps adjacent nonempty replacements distinct. Existing primary
host execution/navigation APIs remain available.

`Document::apply_caret_transaction` extends the existing single-selection
transaction API. Typing/paste, per-cursor newline indentation and forward/backward
deletion operate on the original snapshot, commit once and restore the complete
normalized set on undo/redo. Read-only/stale/invalid-state failures do not commit
partial text, history or caret changes. Empty edits retain redo. Explicit-range
replacement remains a single target, preserving the full before set in history.

Runtime and persisted history keep all selections. History v1's optional
`secondary` fields default to empty for older JSON/legacy imports. Import checks
every coordinate before mutation. Session restore remaps secondary selections
with the primary through UTF-16 coordinates when disk content changes; the dirty
recovery retains the original set.

Source reference remains `../kode` at
`7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68` (clean); its TextBuffer/CodeEditorView
are single-cursor. B004 is an explicit extension, tracked by UX-111–115. The frozen
`../ttyd` reference remains unchanged.

## Validation

Nine new `kode-core/tests/multiple_selections.rs` tests pass, including an
independent streaming oracle for every triple of cursor positions on documents
up to seven bytes. Other cases cover overlapping/reversed/adjacent ranges,
duplicate/EOF/empty cursors, Unicode paste, one-revision edits, collision after
delete, CRLF, distinct indentation, undo/redo, history JSON/backward compatibility,
read-only guards and atomic rejection of corrupt secondary/after selections.

`cargo +stable test --locked -p kode-core -p kode-workspace -p kode-tui` passes
333 tests; four existing real Java/H2 qualification cases are opt-in and were not
enabled in that command. The final session-remapping change is additionally
checked by all nine `kode-workspace --test session` tests, including stale-file
dirty recovery. This is a core checkpoint, not new file/SQL/REST PTY evidence.
Affected all-target Clippy with warnings denied and format checks pass.

## Continue in this order

1. Wire Alt+click toggle and Alt+drag into EditorInput with a stable primary,
   correct capture/focus behavior and normal-click reset. Use shared visual
   positions for Ctrl+Shift+Up/Down, including wraps/folds/tabs/Unicode.
2. Render every selection/caret while keeping the actual terminal cursor at the
   primary. Preserve primary scrolling; update all shared host synchronization.
3. Add Alt+Enter to file search, publishing only a current bounded result set and
   restoring editor focus. Preserve existing Ctrl+Enter semantics and update Help.
4. Route bracketed/native paste through all selections and include the complete
   set in delayed-paste proofs. Define multi-selection copy/cut ordering and
   completion ownership so late work cannot affect a different set.
5. Qualify deterministic reducer/render/host tests and real file/SQL/REST PTYs,
   read-only guards, overlap/adjacency, empty documents and undo/redo. Finish U04
   evidence before setting done; U06 owns word double-click gestures.
