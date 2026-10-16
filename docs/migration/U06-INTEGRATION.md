# U06 word-selection integration

Status: done. The 364 core/workspace/TUI tests, sixteen actual word-selection
PTYs and twelve multi-cursor regression PTYs pass. Format, affected check,
all-target Clippy with warnings denied and CLI build pass.

The implementation and acceptance details are in [U06](../parity/U06.md).
`EditorInput::with_clock` shares the host's injected clock and retains a monotonic
standalone default. Click pairs use a 350 ms interval and completed-release,
source/cell/context proofs; a third click begins a new pair. U04 single-click
fixtures explicitly separate clicks instead of bypassing production detection.

`VisualLayout::cell_source` maps the painted scalar under tabs/Unicode escapes.
The folded-prefix caret boundary fix is covered by exact renderer assertions and
narrow actual terminal editing. Worker-side UAX #29 segmentation uses pinned
unicode-segmentation 1.13.3 and original source bytes. Pending analysis preserves
the first click's selection; Alt publication uses the pre-toggle baseline to add
the word without losing prior cursors. Cancellation never restores obsolete state.

Word jobs use the bounded scheduler, snapshot/request/revision/focus/full-selection
and viewport proofs. Late results cannot consume newer work, and early quit/
workspace departure cancels pointer jobs before saving sessions. Publication marks
workspace state changed for autosave. All shared editable/read-only host guards
and U04's transaction/history behavior remain in place.

Next: finish U07 (path pickers), U09 (LSP repair) and U12 (database packages).
Finish the Uxx series before resuming Z03's [expanded artifact matrix](Z03-ARTIFACT-MATRIX.md),
as requested. E13/Z04 retain final combined editor/platform qualification.
