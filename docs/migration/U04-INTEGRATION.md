# U04 multiple-cursor integration

Status: done. Eleven core multiple-selection tests, 349 ordinary core/workspace/TUI
tests, all four opt-in Java/H2 qualification cases and twelve CLI PTYs pass.
Format, affected check and all-target Clippy with warnings denied pass.

Implementation and detailed acceptance evidence are in [U04](../parity/U04.md).
The shared editor now connects complete selection sets to Alt+click/drag,
Ctrl+Shift+Up/Down, Alt+Enter file find, painting, typing/deletion/clipboard,
undo/redo, native paste/completion/navigation proofs and versioned file sessions.
The primary owns scrolling, terminal cursor and host execution. Explicit provider
completion edits remap the other selections rather than applying an invented
replacement range at every cursor. Copy/cut joins source-ordered selections with
LF; paste repeats the complete text at each selection. All mutation guards remain
at the core boundary.

U06 continues with Unicode word double-click and Alt+double-click. Coordinate
its first click with U04's existing Alt+click toggle and additive drag state;
do not discard or duplicate the prior selection set. Inspect the current input
reducer and use deterministic click timing with real terminal fixtures.

Finish the Uxx series before resuming Z03's [expanded artifact matrix](Z03-ARTIFACT-MATRIX.md),
as requested. E13/Z04 retain final combined editor and platform qualification.
