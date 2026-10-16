# REST / Postman / OpenAPI tasks

This is the Postman-like API module, separate from SQL databases. Preserve one
Postman v2.1 collection plus project-local environments, with runtime responses
and cookies kept outside the exported collection. Reference `SPEC.HTTP_API_TEST_MODULE.md`
and `REP.HTTP_API_TEST_MODULE.md` for context; executable source determines baseline.

## R01 — Lossless collection and environment model

**Status:** done

**Depends:** F02. **Owns:** `kode-rest/src/model/` and `fixtures/rest/model/`.
**Read:** `rest/model/{RestModels,RestWorkspaceModel,RestEnvironmentCodec}.kt`;
RestModelTest.

1. Use a Serde JSON value tree, preserving arbitrary fields, object values,
   disabled entries, scripts, saved examples and `x-kode-*` extensions. Typed
   accessors mutate only the intended subtree.
2. Port path addressing, traversal, create/rename/delete/duplicate/move, request IDs,
   folders and selected path repair. Copying assigns new request IDs as the source does.
3. Port environments, enabled/type/description/secret metadata, active selection,
   create/copy/rename/remove and escaped `key=value` codec. Supply session import/export.

**Accept:** all original model tests plus nested moves/deletes and arbitrary unknown
   fields survive import-edit-export. Runtime results/cookies never enter document.
   “Lossless” preserves JSON data, not byte-identical formatting. No script execution
   is implied by preserving script fields.

## R02 — Variables, auth, headers, URLs and request bodies

**Status:** done

**Depends:** R01, F03. **Owns:** `kode-rest/src/resolve/`.
**Read:** `rest/resolve/RestResolver.kt`; RestResolverTest.

1. Port collection→ancestor→request→environment precedence, recursive variables,
   disabled variables, cycle/unresolved reporting, path variables and query encoding.
2. Port inherited auth with explicit noauth/basic/bearer/apikey and unsupported-auth
   errors; inherited headers via `x-kode-headers` and request override semantics.
3. Resolve raw, urlencoded, multipart text/files, file body and GraphQL query/variables.
   Produce an immutable resolved request and logical raw preview with timeout,
   redirect and TLS-validation settings. Separate body I/O from pure resolution.

**Accept:** existing resolver cases plus every body mode, duplicate/disabled keys,
   file-not-found, Content-Type overrides, encoded path/query values and `sse://`
   normalization. Validation fails before any network request. Do not send empty
   bodies silently for unsupported modes; report an explicit limitation.

## R03 — Postman and OpenAPI import/export

**Status:** done

**Depends:** R01. **Owns:** `kode-rest/src/import/` and import fixtures.
**Read:** RestOpenApiImporter.kt, RestWorkspaceModel import/export,
RestModelTest OpenAPI JSON/YAML cases.

1. Preserve Postman v2.1 documents and imports into a chosen folder without losing
   extension fields. Validate shape with useful file/path errors.
2. Parse JSON with serde_json and YAML with the selected maintained parser. Port
   OpenAPI 2/Swagger and 3.x path operations, base URL/server selection, tag folders,
   parameters, body examples/schema samples and supported authentication mapping.
3. Inventory `$ref`, security schemes and 3.1 constructs against the original
   importer. Implement existing cases and report unsupported constructs explicitly;
   a crate accepting a document is not proof the conversion retained its meaning.

**Accept:** JSON/YAML paired fixtures, nested destination, 2.0/3.x, query/header/path
   parameters, body/media examples, auth, missing/invalid document and unknown Postman
   fields. Export does not include runtime cookies/responses accidentally.

## R04 — HTTP execution, cookies and cancellation

**Status:** done

**Depends:** R02, F03. **Owns:** `kode-rest/src/http/`.
**Read:** `rest/http/RestHttp.kt` ordinary HTTP/cookie/request-builder paths;
RestHttpTest.

1. Use reqwest async clients with correct per-request settings, method/body,
   duplicate header representation, redirect and TLS policy. Preserve transport-
   managed-header handling and notify the UI which preview fields are normalized.
2. Use a cookie store, retaining session scope and inspection/clear operations.
   Capture status/final URL/headers/cookies/duration/received bytes and bounded body.
3. Stream response bytes into capped previews, distinguish transport errors from
   non-2xx responses, cancel without blocking and tag execution identity. Never
   replay a non-idempotent request implicitly.

**Accept:** local HTTP fixtures for duplicate headers, cookies, redirects, all body
   modes, timeouts, invalid TLS and truncation; original transport-managed-header
   regression. User can keep typing and switch requests while a response runs.

## R05 — SSE and WebSocket execution

**Status:** done

**Depends:** R04. **Owns:** `kode-rest/src/stream/{sse,websocket}.rs`.
**Read:** RestHttp.executeSse/executeWebSocket and incremental stream tests.

1. Compose reqwest byte streams with eventsource-stream for SSE, using request
   headers/body/settings and `sse://`/Accept selection. Preserve event/data boundaries
   and incremental output even when bytes/UTF-8 sequences split across reads.
2. Use tokio-tungstenite for ws/wss; preserve the source's receive-only connection
   opening (the audited source sends no initial request body). Any explicit initial
   message mode is an extension. Reproduce incoming text/binary fragment behavior,
   close/error reporting and flow control. Verify
   authentication/cookies/TLS independently from the HTTP client.
3. Provide one transport-neutral execution event stream and cancellation handle;
   cap retained output, retain received counts and update the UI incrementally.

**Accept:** loopback SSE/WS fixtures for fragmented frames, multiline SSE data,
   slow/infinite streams, cancel, normal/abnormal close, stale request switch and
   bounded memory. No automatic reconnect/replay unless explicitly designed later.

## R06 — REST tree, environments and management dialogs

**Status:** done

**Depends:** R01, R03, R05, F05, E12, W03. **Owns:** `kode-tui/src/panels/rest/sidebar/`.
**Read:** RestPanelView.kt, RestKeyValueDialog.kt, Main REST management wiring.

1. Port collection/folder/request tree, keyboard and mouse expansion/selection,
   `/` filter, context menus, add request/folder, rename/copy/move/delete,
   import/export and run-selection actions. Preserve shortcuts in source handlers.
   Show structured import notices and file errors; unsupported OpenAPI conversion
   must remain visible when its generated request is marked incomplete.
2. Port environment list in same sidebar, resizable horizontal divider, activation
   and create/rename/copy/delete actions. Use the shared virtual editor for values.
3. Persist selected path and divider state; repair selection after model changes.
   Implement bounded dialog fields, visible cursor and separate OK/Cancel targets.

**Accept:** model mutation matches visible tree, filter typing owns focus, all menu
   actions work by keyboard/mouse, drag persists and environment clipboard/editing
   passes RestEditorViewTest/UX-102. No hidden operation modifies another request.

## R07 — Request/response editors and complete REST UX

**Status:** done

**Depends:** R02, R04, R05, R06, E09, E12, L02.
**Owns:** `kode-tui/src/panels/rest/editor/`, `kode-rest/src/format.rs`.
**Read:** RestEditorView.kt, RestCodeFormatting.kt, HttpHeaderCatalog.kt;
RestEditorViewTest and RestCodeFormattingTest.

R06 provides application-owned environment buffers, rebased runtime item keys,
the sidebar's real execution callback and bounded response owners. Extend these
owners for editable request/response views; preserve unknown fields and associate
results by captured runtime identity even when imported IDs are absent/duplicated.

1. Port method/URL/Send controls and request tabs: params, auth, headers, body,
   variables and settings as actually exposed; enabled/inherited rows, secret
   reveal/masking, key/value dialogs and body modes. Reuse the complete editor.
   Incomplete OpenAPI imports require correction/explicit acknowledgement before
   their blocking marker is cleared; show the retained conversion notices.
2. Implement header-name completion and duplicate-line warnings, resolved raw
   request preview, language/content-type selection and optional JSON/XML pretty
   formatting that leaves invalid raw content intact.
3. Port draggable response pane and body/headers/cookies/raw/transaction tabs,
   status/time/size/error/truncation feedback and incremental updates. Keep raw bytes
   distinct from formatted display. Transaction is a logical request/response view.

**Accept:** UX-102–106, focused header arrow-key regression, Ctrl+Enter/Send/cancel,
   every field/tab/button mouse target, response scroll and virtual read-only guard.
   No claim of captured encrypted/decrypted TCP packet bytes. Host state survives
   request switches; old responses are associated with their original request ID.

## R08 — Collection runner, persistence and REST qualification

**Status:** done

**Depends:** R07, W03, W05. **Owns:** `kode-rest/src/runner.rs`,
`kode-tui/src/panels/rest/runner.rs`, REST integration fixtures/report.
**Read:** Main's REST collection/folder run selection/state, RestRunSummary and
all source REST test suites.

R06 introduced the small immutable `runner::execute` boundary so the sidebar Run
action executes real requests. This card still owns the complete runner UI/history,
remaining persistence controls and end-to-end HTTP/SSE/WS qualification; backend
availability and completion counts alone do not satisfy it.

1. Port selectable sequential collection/folder runs, per-request outcomes,
   progress/summary, cancellation and original ordering/selection rules.
2. Integrate full REST project persistence including response/environment panel
   heights, pretty-print choice and active environment, excluding runtime state.
3. Replay create→edit→resolve→send→inspect→export→restart workflows with HTTP/SSE/WS
   fixture servers and embedded-editor event sequences.

**Accept:** every source REST test scenario and all R07 controls have evidence;
   cancellation prevents later selected requests from starting, invalid variables
   do not reach transport, unknown collection fields remain intact. Postman scripting,
   OAuth token acquisition and unsupported auth are explicitly labeled extensions
   unless the baseline audit finds actual execution support.
