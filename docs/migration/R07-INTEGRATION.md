# R07 integration work remaining

Status: in-progress. These notes locate working components; they do not narrow the
full migration goal or replace the task's acceptance requirements.

The mutation layer (`kode_rest::model::editing`), request form component
(`panels::rest::editor::form`), shared-buffer worker preparation (`editor::buffers`)
and formatter/header adapters have focused tests. AppState still opens R06's
read-only node snapshot. The next concrete action is application composition.

- Add active/cached Form state to `RestState`, keyed by the existing runtime item
  keys in `RunState.keys`, with tree intents rebasing/invalidation. Imported IDs
  may be absent or duplicate. Preserve form/buffer state when switching requests.
- Prepare `form::Prepared::from_model` on a worker when opening a node. Attach the
  form only when its tagged open is current and modals permit focus. A later file,
  environment, SQL view or workspace switch supersedes it. Reopening a cached form
  can use `refresh_fields`; `set_derived` publishes worker-resolved readonly rows.
- Give editable request buffers document metadata containing item key and
  BufferKind. Reuse cached document IDs/history/caret and configure the main
  EditorView from `buffers::Prepared`. Keep readonly previews replaceable.
  Current `attach_rest_document` prunes every non-environment REST document;
  update that policy before caching editable request buffers.
- Render/dispatch the active Form around the main shared editor, using its editor
  geometry. Form modals must join global modal priority and delayed attachment.
  Drain clipboard intents using their proofs, along with Changed/Send/Buffer/
  Cookies/Notices intents. A selected buffer is recorded in `Form.selected_buffer`.
- Route EditorWork::Execute by ExecutionAction: SendRequest belongs to REST;
  current runtime code always calls SQL. Keep native search Ctrl+Enter precedence.
- Parse request buffer changes on document-owned workers, coalescing by document
  and exact revision. Publish owned leaf changes against the current item key;
  unrelated field edits must survive. Do not parse an untouched freshly opened
  headers document into reconstructed JSON and lose opaque metadata.
- Capture current ropes for Send and raw preview even if a semantic parse is still
  pending. `buffers::Override`/`apply_override` provide worker merging. Restrict
  execution overrides to the selected request/subtree. Invalid draft headers must
  fail before I/O. Preserve comments/unfinished headers in owned project drafts;
  exclude generated raw previews/responses from persistence. Session capture
  already excludes virtual documents from generic openEditors serialization.
- Preserve unopened saved request drafts through path rebasing too. Runtime keys
  are not persisted collection IDs, so map captured keys to current paths when
  writing project drafts; root replacement invalidates the old collection owners.
- R06's run preparation currently validates the whole collection Arc and replaces
  the key map after preparation. Review this when adding concurrent request
  semantic publication: metadata changes must not retarget execution or erase keys
  assigned by a newly opened view. A structural generation and key reconciliation
  can distinguish those operations while retaining frozen request snapshots.
- Add response/transaction shared editor views, bounded live event capture and
  worker display preparation. Extend runtime view/owner/overlay routing for any
  additional editor slot. RunState currently discards incremental body events and
  retains final responses only. Keep raw bytes, preview formatting and request
  normalization notices distinct; WS defaults to receive-only.
- Add response divider/status/tabs, optional formatting and cookie inspection/clear
  through actual callbacks. Explicit body formatting should be an undoable editor
  action; opening/toggling a display must not silently change outgoing raw bytes.
- Qualify large inherited sections: the current form's local editor can be pushed
  below the viewport by many inherited rows. Its row strings are cached, but
  control layout still walks all rows. Finish accessible scrolling/layout and
  measure the complete application's interaction behavior before completion.
- Audit cancellation cleanup when adding frequent parse/preview jobs. Runtime
  `queue` cancels superseded scheduler requests, but retains their Context map
  entries; cancelled delivery can skip completion. Captured Arc proofs must not
  accumulate across superseded requests. Apply an actual resource-retention
  regression alongside the existing stale-result tests.

Then run R07's actual application/PTY HTTP/SSE/WS flows, including the focused
headers-arrow versus transaction-response regression and owner isolation. R08's
complete runner presentation/qualification and D09's final REST gate remain open.
