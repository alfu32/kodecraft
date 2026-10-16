# R07 application integration map

Status: done; implementation and qualification tracked in
[the R07 report](../parity/R07.md). This file locates the composition paths and
does not narrow the complete migration goal.

- `app/rest_editor.rs`: tagged form/buffer/semantic/format completions, cached
  document identity, current-rope Send capture, derived rows, dispatch and paint.
- `app/rest_response.rs`: separate response-editor owner, modal attachment,
  bounded display jobs, tabs, divider and preferences, independent focus/scroll.
- `app/rest.rs`: environment/notices, attachment gate, tree lifetime/rebasing,
  selection execution and environment/current request capture.
- `panels/rest/editor/jobs.rs`: immutable worker DTOs; all rope materialization,
  header parsing, derived resolution and display/format preparation.
- `editor/drafts.rs` plus `panels/rest/state.rs`: owned raw request drafts,
  path/kind codec, strict execution merge and tolerant worker persistence.
- `panels/rest/run.rs`: structural generation, reconciled runtime keys, live
  bounded event prefix and authoritative response cache.
- `app/runtime/rest.rs`: document/form-owned jobs and real executor callbacks.
  Runtime queue/disposal releases cancelled Context maps without waiting for
  delivery; `runtime/rest_tests.rs` proves the retained Arc is dropped.

Application regressions include HTTP/SSE/WS, native search precedence, formatting
undo, restored invalid headers, explicit startup-file precedence, response owner
switches, all response tabs and the original header-arrow/Transaction case.
`test-rest-editor-terminal.py` qualifies actual CLI terminals and persistence.

R08 records completed runner presentation/history and workflow qualification.
D09 records actual Java/H2/REST isolation. E13 must measure the full application,
including collection copy-on-write during edits and any further hot paths;
component-level cache tests do not claim a universal frame-time bound.
