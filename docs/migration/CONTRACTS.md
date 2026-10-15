# Shared implementation contracts

F02 establishes identities, coordinates, commands, immutable document snapshots,
intelligence DTOs/providers, clipboard/document I/O boundaries and typed errors in
`kode-core`. The remaining sections specify behavior for their owning tasks. A task may refine a signature, but must update
this document and its consumers before changing semantics.

The workspace uses the latest Rust stable channel at the user's request rather
than pinning a toolchain release; evidence records the exact tested version.
`Cargo.lock` fixes application dependencies. `TextRange::validate` checks both
UTF-8 scalar boundaries against a document. Reverse selections retain anchor/head
while `Selection::range` produces their normalized half-open edit range.

## Dependency direction and ownership

- `kode-core`: shared identifiers, text coordinates, document snapshots, editor
  commands, normalized navigation/completion DTOs, text and byte models.
- `kode-platform`: host services; can depend on core, never on UI or feature crates.
- `kode-workspace`, `kode-syntax`, `kode-git`, `kode-rest`, `kode-database`, and
  `kode-lsp`: depend on core and needed platform facilities, never on UI.
- `kode-intelligence`: depends on core and syntax; owns semantic persistence.
- `kode-tui`: reusable widgets/editor and panels; consumes feature services. Keep
  orchestration in `app/`, feature panels in their own subdirectories.
- `kode`: CLI, resource discovery, runtime construction, and application launch.

Keep editor logic split into `document`, `commands`, `layout`, `input`, `search`,
`folding`, `minimap`, `overlays`, and `popups`. Do not create another 3,000-line
editor file or put Git/SQL/REST rules into the editor widget.

## Identifiers and text coordinates

```rust
// Sketch: concrete serde derives/constructors are F02/E01 work.
struct WorkspaceGeneration(u64);
struct DocumentId(u64);
struct Revision(u64);
struct RequestId(u64);
struct ByteOffset(usize);          // boundary in UTF-8 source text
struct CharOffset(usize);          // Unicode scalar index, Ropey adapter only
struct LineIndex(usize);           // zero based
struct Utf16Column(u32);           // legacy/LSP adapter only
struct CellColumn(u16);            // terminal layout only
struct TextRange { start: ByteOffset, end: ByteOffset } // half open
struct Selection { anchor: ByteOffset, head: ByteOffset }
struct JobTag {
    workspace: WorkspaceGeneration,
    request: RequestId,
    document: Option<(DocumentId, Revision)>,
}
enum DocumentOrigin {
    File(PathBuf),
    Virtual { owner: String, key: String, label: String },
}
```

Canonical source spans are UTF-8 byte ranges at valid scalar boundaries. Convert
to Ropey's scalar indices at its boundary. Layout supplies forward and inverse
maps for source bytes ↔ logical lines ↔ rendered cells/rows. Tree-sitter gets byte
offsets and byte columns; LSP gets its negotiated encoding (UTF-16 default).

The editor's compatibility display renders a tab as `|-->` (four cells), and
non-renderable/wide/non-BMP/combining scalars as the existing explicit escapes.
Raw text is unchanged. Do not silently replace that presentation with native wide
glyphs or tab stops; the source/test disagreement is recorded in EDITOR-UX.md.
Surrogate-interior legacy positions are clamped to a documented valid boundary;
Rust must never create invalid UTF-8 to reproduce a Kotlin indexing defect.

Store an anchor and head so reverse selections remain distinguishable. A normalized
range is derived for edits; normalizing must not erase the direction. Show line
numbers and user-facing locations as one based, but keep internal indexes zero based.

Request/document allocators supply globally unique process identities. Independent
load/index/save gates must not each restart their own request counter, since they
share one worker queue. E04 also adds `FileFormat` to portable document snapshots:
strict UTF-8, an explicit BOM flag and observed LF/CRLF/mixed style, with raw
newline bytes retained. File save identities include canonical target path,
length, timestamp and SHA-256, preserving symlink target identity.

## Document and editor API

`Document` owns Ropey text, origin, language, current revision, saved revision,
newline/BOM metadata, and edit history. An `EditorViewState` owns cursor/selection,
preferred navigation column if applicable, scroll position in visual rows, folded
regions, search state, popup state, and focus. Multiple views of a file should use
the same document identity; per-view scroll/focus remains independent. Virtual
documents get distinct identities even when their labels match.

E01 supplies `Document::snapshot() -> TextSnapshot`, an O(1) immutable Ropey clone
with typed slice/line/chunk/coordinate access. Its portable intelligence DTO is
created with `to_service_snapshot()`; UTF-8 text materialization is lazy and shared
once per revision. Ropey is configured with LF-only line recognition so standalone
CR and Unicode separators do not silently change Kode's logical line count.
Raw source CRLF is retained; `line_content`/`line_end` exclude CRLF for UI movement,
while source scalar/UTF-16 columns retain byte round trips within the raw line.

Required operations:

| Contract | Inputs | Outputs / rules |
|---|---|---|
| `apply_edit` | document ID, expected revision, range, inserted text, edit kind | New revision and edit delta; reject stale edit. Group a replacement/paste/completion as one undo action. |
| `snapshot` | document ID | Immutable text/revision/origin; safe to send to workers. |
| `execute_editor_command` | view state, command, document, editor services | State changes plus effects; no raw terminal or filesystem access. |
| `layout` | snapshot, viewport size, folding, display policy | Visible rows plus source/cell maps shared by render and input. |
| `render` | layout, overlay spans, view state, Ratatui buffer | Cell output only. No file reads, parser work, network calls, or SQL. |
| `hit_test` | layout, local cell position | Text position, gutter/fold/minimap/popup/control hit, or outside. |
| `open_document` | origin, request ID | Asynchronous load result; late loads are discarded. |
| `save_document` | origin, snapshot, expected disk identity | Result for the saved revision; a newer edit remains dirty. Virtual saves go to their host. |
| `export_view_state` | document and view | Versioned persisted state; history and dirty text retained. |

Commands include character/word/line/page movement with selection extension,
insert text/newline, delete backward/forward, select all, copy/cut/paste, undo/redo,
save, find/replace, folding, completion and navigation actions. Terminal key names
are decoded once into commands, preserving raw character case and modifiers.

E03 routes all document mutation through `apply_transaction`: ranges refer to the
original revision, are validated together and applied from right to left, and
commit as one new revision/undo action. `apply_edit` uses that same guarded path.
Read-only capability also rejects history and history import. Runtime history
retains at most 200 transactions/64 MiB of changed text; discarded entries are
reported in `EditOutcome`. Versioned history export keeps at most 30 states on
each stack within a shared 64 MiB history-text budget, records omitted counts,
and preserves current/saved text. Import reports any capacity trimming explicitly.

`EditorHost` supplies `read_only`, `save_target`, `language`, `completion_provider`,
`intelligence_provider`, `on_change`, `on_navigate`, `warning_lines`, and
`transient_highlight`. The host may intercept its documented shortcuts, such as
SQL/REST execution, only after focused child controls and popups get their defined
priority. Read-only checks occur at the edit boundary, covering keyboard, mouse,
replacement, completion, programmatic insertion, paste, and history actions.

`EditorDocuments` keeps document/save lifetime separate from `EditorView` focus,
caret, search and popup lifetime. Response/raw/historical factories force read-only
documents. File and explicit owner-key saves serialize per document; a view closing
cannot discard a pending critical save. Document save owner identities are distinct
from view owner identities. The application broadcasts document changes to all
attached views before paint, drains work/events, and cancels only interactive owners
on view detach. Native clipboard jobs use the shared ClipboardService and bounded
blocking scheduler. Execution payloads retain snapshot/selection; actual SQL/request
resolution belongs to the owning feature card.

`DocumentLoader` gates worker-prepared documents and exposes typed load failures.
`SaveController` serializes saves per document and coalesces later requested
snapshots. Its critical completion path accepts an older saved revision while
keeping later text dirty; it must not use preview-style revision rejection.
Load/save scheduling goes through the injectable bounded blocking-worker interface.
Virtual host saves route through the owner; native document I/O rejects virtual
origins regardless of their label. Clipboard copies remember local text/epoch
immediately; native I/O is serialized in workers and cannot overwrite a newer epoch.

## Event and effect model

The application owns `AppState` and consumes terminal events, timers, child-process
status, and typed worker results. Reducers return effects: schedule work, open/save,
request navigation, update clipboard, persist, or request repaint.

Every worker operation has a cancellation token and a `JobTag`. Before publication,
check workspace generation, document identity/revision where relevant, and current
request ID. Some semantic results also depend on an index revision. Disposing an
owner cancels its jobs. A dropped future is not proof a JDBC query stopped.

Use bounded queues. Coalesce repaint, progress, and superseded document jobs.
F03 supplies `Scheduler<T>` with 64 completion slots, 32 asynchronous slots and
4 blocking slots, plus independent owner/workspace cancellation. Critical
completions (especially saves/failures) wait for delivery; orderly shutdown drains
them while joining every worker. Blocking closures must check their cancellation
token at safe checkpoints; once launched they are joined rather than detached.
`WorkerScheduler<T>`, `Clock` and `ProcessFactory` are injectable test boundaries.
Repaint uses a coalescing watch channel; progress holds at most 64 owner values
with 512-character messages. Application `ResultGate` filters exact request,
workspace, document and revision identities, including dismissed popups.
Stream large results in bounded batches; cap previews with a visible truncation
state. Never drop a save completion or failure just to relieve queue pressure.
Use a separate control path for cancellation/shutdown. Define queue capacities in
F03 with fixtures for saturation and shutdown.

Only one component consumes an input event. Order: top modal → active captured
popup/drag → focused child/editor → containing panel → unhandled global binding.
See the exception details for usage, suggestion, and definition popups in the UX
contract; they do not all capture every key identically.

## Feature service contracts

Project scanning uses only configured canonical roots (new-project defaults are
existing src/lib/include). Root/nested Gitignore rules and hidden whitelisting are
project-local; explicit exclusions cannot be re-included. Scan batches cap at 128
files and carry workspace/request identity plus scan-only progress. The collector
stages publication and preserves the previous file list on cancellation/incomplete
reads. Folder/tree/search policies are separate from index traversal. See W01 for
symlink, metadata and error policy.

Editor layout is cached by document identity/revision, text width and collapsed
folds. Raw scalar byte ranges map to fixed tab/escape display runs; whole fitting
runs wrap atomically and oversized runs clip. Drawing and hits share this map.
Only prepared visible cells are painted. Caret at a full-row EOF preserves source
punctuation, and wide BMP emoji/enclosing marks use escapes to avoid cell drift.

Editor input owns per-document caret/scroll/focus state and mouse capture; a
release outside or terminal focus loss releases the original owner. Hooks receive
the unmodified event and shared absolute geometry. Logical arrows are separate
from visual paging. Clipboard effects carry raw text and delayed paste tickets
validated against request/document/revision/selection/focus before insertion.
Host composition remembers copies immediately and runs native I/O in workers.

Search scans immutable snapshots in bounded workers and publishes only current
workspace/request/document/revision/query results while its bar is open. Native
clipboard operations are effects with field paste identity guards. Replacement
uses Kode's custom numeric `$n` expansion and one grouped edit; empty matches
are excluded consistently. Unsupported Java syntax and resource limits report
explicit errors; see `docs/parity/E07.md` for the supported translation boundary.

Fold analysis is worker-prepared and revision-checked. Collapsed anchors cannot
hide text after edits/reload; navigation reveals every containing fold. Summary
cells/ellipsis and minimap source blocks share the input/render geometry. The
minimap uses raw UTF-16 source shape, four lines per block, and stores zero-based
targets explicitly. Host hook composition forwards fold-state/reveal methods and
reconciles asynchronous search/navigation before painting.

Completion requests retain document/revision/caret/selection and dismissal
identity. Host suggestions precede provider entries, labels deduplicate, and
only empty semantic results outside member context permit local fallback.
Provider replacement ranges/insertion text are authoritative; host/local fallback
replaces just the typed prefix. Accept is one guarded edit. Hook composition must
forward context changes after core input, focus disposal and explicit/automatic
completion effects; expensive provider work runs in bounded workers.

Navigation target DTOs carry optional line/UTF-16 metadata for unavailable-source
labels. Definition/usage jobs capture snapshot, identifier and selection; completed
popups retain context until dismissal. Preview reads/styles/width calculation run
in workers, current unsaved source uses its captured snapshot, and declaration
previews cap at 24 lines. The host calls the 500 ms hover deadline from its timer,
drains navigation effects, forwards popup mouse capture and reconciles resulting
navigation. Usage popups capture keys/wheel; definition-only popups capture Escape.

E11's `OverlayCache` holds prepared, viewport-adjacent lexical and semantic spans.
`OverlayJob` captures an O(1) rope snapshot, language, byte coverage and exact
workspace/request/document/revision tag. `OverlayWorker` reuses L02 syntax sessions
off the application thread and queries the registered intelligence provider there.
The application also checks L09's query epoch/index revision before attaching a
report; superseded reports reschedule current work. A dependent-file publication
invalidates the affected editor without inventing a text revision. Worker repaint
notifications wake the application directly; its timer still services input,
hover/drag and debounce deadlines.

Core edit deltas (one transaction, right to left) immediately shift untouched
paint; undo/shared-view changes use a scalar-safe contiguous rope diff. Touched
spans and identifier boundary joins are discarded, with successive edits remapped
against the most recent cached snapshot. No parsing, query or whole-file string
materialization runs in paint/input handlers. The documented style order is body,
lexical, semantic, warning background, hover underline, search, active search,
transient range, tab marker, selection, cursor. Endpoint sweeps produce disjoint
segments and source/cell maps draw each glyph once, including folded summaries.
Theme overlay rules patch only explicit properties. Declaration ranges feed
Ctrl-click routing; captured-source definition previews receive native styles.
The header places grammar/fallback status first so long paths cannot hide it.

Highlight sessions retain at most 32 cached documents / 64 MiB source-accounted
text (plus bounded running jobs); visible coverage includes 24 preceding/25
following logical lines. Each report retains at most 32,768 lexical and semantic
spans per layer, with explicit truncation notes. These are source/cache limits,
not a hard process RSS bound. Provider failure preserves safely remapped semantic
paint and shows a note; a ready empty result clears it. See `docs/parity/E11.md`.

| Service | Public operations and owned output |
|---|---|
| Workspace | discover/open, tree entries/refresh, create/rename/delete, source-root/exclusion updates, scans, file search, session load/save |
| Syntax | language registry, lexical spans, parser update, grammar availability; no UI colors in semantic facts |
| Intelligence | index/update/remove files, immutable snapshot, definitions, references, completions, semantic tokens, status, clear/rebuild index |
| LSP | catalog/install/start/stop, open/change/save/close documents, definitions/references/completion and supported enrichment |
| Git | status, branches/current branch/checkout, commits, staged/worktree diffs, files/content at commit, stage/unstage/commit/tag |
| REST | collection edit/import/export, resolve/preview request, execute/cancel/stream, cookies, ordered collection run |
| Database | datasource config and credentials, driver install/status, connect/test/disconnect, metadata, execute/cancel, autocommit/commit/rollback |

Errors are typed at domain boundaries and rendered in the owning panel. Preserve
SQLSTATE/vendor code, HTTP status versus transport failure, Git failure context,
parse errors, and missing-resource information. A failed operation never yields a
fake successful empty result. Logs go to a file/stderr capture, never the TUI canvas.

## Intelligence publication

`FileSemanticDelta` holds file facts: symbols, scopes, occurrences, imports, types,
expression hints, lexical spans, relations, and file dependencies. Use the exact
facts needed by `SemanticModel.kt`; extraction and resolution are different stages.
Resolution builds an immutable `SemanticSnapshot`, published atomically only after
the corresponding persistence transaction succeeds. Readers retain their previous
snapshot while a new one is built. Record content revision, semantic revision,
public surface hash, and resolution generation separately.

`NavigationTarget` includes document/path, identifier range, optional declaration
range, language, symbol kind, and missing-source state. `CompletionItem` includes
label, insertion text, detail, kind, replacement range, and source; define a fallback
replacement range using only the typed identifier prefix. Unknown receiver types
must not fall back to unrelated workspace members.

## Persistence and resources

V01's `FileClassification` carries MIME/category/language/detection source and
Code/Image/Hex routing. Shell shebangs precede explicit Kode filename/extension
tables; signatures precede deterministic MIME guesses for unknown names. Only
capabilities supplied by the V03 decoder service permit Image routing. Generic
image MIME hints alone do not promise decoder support; decoding errors retain
their message and allow byte inspection. The F02 compiled table name remains
`mime-types.json`.

File browsing and index scanning use separate policies. `FileTree` publishes only
prepared worker listings; rendering/reducers never stat files. A successful file
move retargets document metadata/revision, tree expansion, recents and configured
roots/exclusions while preserving text/history/dirty baseline. Before scheduling a
file mutation, reserve its affected paths with `EditorDocuments::reserve_file_action`.
New saves wait until critical completion is applied with `finish_file_action`,
which releases the reservation on both success and failure. Directory deletion
keeps dirty documents owned for recovery; it never replaces them with empty buffers.

W04 project search scans all project text independently of indexing configuration,
skipping Git/session metadata, binary classifications and directory links. Its
prepared hits carry byte/UTF-16 ranges and document revision/disk identity. `OpenHit`
must reuse a registry document or load real file bytes before attaching the preview;
failed loads never substitute empty editable text. Preview edits invalidate old hit
coordinates and refresh against live snapshots. Child editor effects retain their
owner; physical saves remain registry-owned. Whole-project replacement is absent
in the source and is not inferred from its global replacement field.

`ProjectSessionV1` in `.kode-rs.json` contains version, recent files, open document
and view snapshots, roots/exclusions, and feature state blocks. W03 owns the exact
schema and legacy import. Prefer atomic replace with a recoverable previous state;
flush on orderly shutdown after the normal three-second debounce.

W03 specifies [session-v1](../formats/session-v1.md). Unknown feature JSON and
legacy fields survive imports; `HistoryStateV1.saved_baseline_known` defaults true
for older Rust snapshots and represents unknown legacy dirty baselines explicitly.
`DiskIdentity` is serializable. The shell uses `restore_with_recovery` so stale
dirty state reaches a unique recovery file before disk content replaces its view.
Drain active critical writes before `SessionPersistence::flush_shutdown`; newer
pending edits cannot be cleared by completion of an older session snapshot.

Use `.kode-rs/index.sqlite` for the rebuildable internal index. Retain the old
`.kode/db` untouched. Existing datasource/credentials/driver directories are import
sources; preserve driver references and unknown fields. Default Rust user config
is a separate `kode-rs` directory, with explicit legacy import and overrides.

Resource precedence: explicit CLI/config override → `KODE_RS_HOME` → executable
resource directory → compiled defaults. Recognize legacy `KODE_HOME` as a documented
fallback when no Rust-specific override exists. Compile a usable default theme,
keyword catalog, and MIME table into the executable. Optional installed resources
must not depend on the current working directory.

## Binary viewer ownership

V02's `document::bytes::ByteIndex` is independent of UTF-8 `ByteOffset`.
`ByteBuffer` owns exact bytes, cursor/anchor and a local binary clipboard, with
borrowed viewport slices and half-open selection ranges. Binary paste never
passes through String. `PreparedViewer` carries worker-loaded code or binary
results; loads, classification and decode errors stay outside rendering/input.
The shell keeps byte views during workspace lifetime, reconciles their renamed
paths and persists HEX recents/active origin. Matching the actual source, binary
edits have no save/history/session-buffer format and remain temporary; the UI
states this explicitly. The binary loading limit is 64 MiB.

## Image viewer workers

V03 `DecodedImage` owns immutable RGBA pixels under explicit dimension/pixel/input
limits. `ImageJob` captures settings, viewport scaling and a fresh workspace tag;
`ImageView` accepts only its pending completion. Decode and color/glyph sampling
run in bounded workers, while painting only copies prepared cells. The shell
registers qualified decoder MIME capabilities and fills absent TGA/DDS/PSD/KRA
extension defaults without replacing explicit resource mappings. Unsupported data
falls back to hex with the original decoder error. Source alpha-ignoring RGB
sampling and active mode order are retained; optional Gray/Contrast controls are
available under `[more]`/M. See V03's qualified subformat matrix and bounds.

## Native semantic model and queries

L01 semantic FileId/SymbolId/ScopeId/TypeId/OccurrenceId are distinct signed i64
FNV identities, retaining source bit patterns. Hash offset inputs are ByteOffset,
never implicit legacy UTF-16. SourceRange aliases the guarded core TextRange;
frontends must validate every delta against its source text before publication.
All actual SemanticModel.kt fact/enumeration/type/confidence/knowledge distinctions
are retained in `kode-intelligence::model`, independently of the editor facade DTOs.

ResolvedSemanticProject separates immutable extraction deltas from enriched
records. SemanticSnapshot builds its private query tables on a worker, validates
identity/scope/reference integrity, collapses identical records and rejects
conflicting IDs. Range queries preserve nested overlaps. SnapshotPublisher swaps
prepared Arc snapshots with increasing revisions; it does not itself persist or
resolve facts. L05/L08/L09 must gate publication after transaction success and
current workspace/document/index tags. Readers may retain old snapshots safely.

L02 `GrammarRegistry` supplies pinned native grammars and metadata; `ParserSession`
owns reusable mutable parser state on a worker. Its immutable `ParsedDocument`
retains source/tree/lexical spans/diagnostics with workspace and document revision.
All offsets and InputEdit columns are raw UTF-8 bytes (rows count LF); CRLF and BOM
are retained. Reuse requires the same workspace, document identity and origin.
Adapters consume the selected grammar's recorded node/field fixtures, never
assumed JVM binding node names. They emit unresolved facts for later resolution.

`SyntaxSession` combines keyword resources and native lexical spans. Reports carry
the exact JobTag and visible grammar/fallback status. L09/E11 must validate current
workspace/request/document/revision before attaching results and show fallback
status; neither input handling nor drawing may parse. Syntax work uses F03 bounded
blocking jobs, cancellation, input/node/time limits. Parser errors retain useful
partial trees; unavailable grammars retain keyword highlighting.

L03 `KotlinSemanticAdapter::extract_parsed` consumes an immutable L02 Kotlin tree
and emits a source-validated L01 delta. Its convenience `extract` owns one mutable
parser per document worker; L09 must bound/evict adapters and gate completed facts.
Only declaration occurrences resolve during extraction. Member receiver links,
raw generic/nullable type names and inference hints pass to L06/L07. Completion
contexts retain raw byte receiver ranges; invalid scalar offsets return errors.

L04 `CSemanticAdapter` shares the immutable-tree/delta boundary. C unresolved type
names preserve tag prefixes and pointer stars, including synthesized anonymous tag
names; their range identifies the AST type specifier. L06/L07 must distinguish tag
and ordinary namespaces, exclude an alias's own identity when binding its target,
and normalize pointer decorations for the source's shared receiver model. No
preprocessing/include I/O happens in extraction. Receiver occurrence links denote
the receiver value/callee rather than call arguments or index operands.

L05 `StoreWorker::start/rebuild` opens the native cache on a dedicated thread.
`StoreClient` exposes snapshot reads plus bounded tagged load/commit/clear tickets;
SQLite connections never leave that thread. Await worker shutdown when disposing a
workspace. Accepted writes drain even when their reply receiver is dropped.
Commit accepts a prepared monotonic SemanticSnapshot and optional complete
compatibility replacement. Omission keeps compatibility only for unchanged source
content. Persistence canonicalizes duplicates, commits changes atomically, then
publishes; same-revision identical facts reuse the current Arc. A competing writer
requires reload. L09 must reject stale producer work before submission and gate
ticket results against the active workspace/document/index revision.

The cache has an application marker, schema version and grammar/byte-model format
signature. Only an identified owned cache may rebuild automatically; foreign or
unidentified corrupt files remain untouched. Typed errors permit L09 to select a
visible in-memory fallback. Compatibility facts retain native byte ranges plus
explicit zero-based lines/UTF-16 columns and source syntax metadata. They are native
cache records, not an implicit import of the old H2 database. Snapshot validation
checks referenced type records/symbols and raw hint/import/type scope ownership.

L06 `SemanticResolver::resolve` consumes source deltas and returns an enriched
project plus explicit unresolved occurrences. Lexical lookup, imports/packages,
visibility and C namespaces are distinct from workspace candidate search. Unknown
or ambiguous bindings retain no resolved identity. Declared type/alias/inheritance
relations and file dependencies are prepared without filesystem I/O. L07 extends
the same worker state with expression inference; L08 owns incremental generation
changes. The immutable raw deltas remain available separately from enriched facts.
Kotlin uses the requalified ast-grep 0.4.1 binding and regenerated L02 ABI fixtures;
upstream recovery markers are not compiler validation.

L07 adds defaulted unresolved `ExpressionSyntaxRecord` facts to each native delta.
References, call/member operands, casts, parentheses, literal shape, explicit type
arguments and binding targets use validated raw byte ranges/semantic identities.
They survive SQLite reload under the v2 semantic format signature; enriched
ExpressionTypeRecords remain separate. Inference is bounded, union-aware and
returns explicit partial-inference notes. Unknown AST calls do not invoke a naming
heuristic. Ambiguous union member declarations retain no arbitrary resolved ID.

`BestEffortExpressionTypeResolver` reads prepared snapshot facts; the widest
contained expression (including an explicit unknown result) takes precedence over
argument literals. No query reparses source or scans unrelated workspace text.
Scope/member/C-type lookup indexes remain worker-owned. L09 must surface inference
notes and validate the document/index revision before using range results.

For incomplete Kotlin access, parser input may mask dangling operators with ASCII
spaces while preserving byte length/positions. Published source/snapshots remain
raw, recovery diagnostics are explicit, and InputEdit compares prepared inputs.
Adapters still derive text and semantic coordinates from the original snapshot.

L08 `IncrementalSemanticIndex::prepare` accepts a previously resolved project,
replacement raw deltas and explicit removed file IDs. Its affected-file plan
distinguishes content/surface/removal changes; unchanged extraction Arcs and
enriched facts remain available while affected files resolve again. Export changes
invalidate reverse dependencies and matching name facts, including definitions
appearing without prior dependency edges. Path rename is remove plus insert.
Resolution generations are separate from raw document/parser versions and advance
only for affected surviving files. `SemanticCacheKey` includes both kinds of
freshness. Cancellation, conflicting input or generation exhaustion cannot mutate
the previous project. Preparation performs no I/O or publication. L09 must persist
the prepared snapshot through L05 before delivering affected/removed files to
token/completion consumers, and must still apply workspace/document revision gates.

L10 `SemanticAdapterSession` selects canonical language aliases and owns one
document's mutable parser. `ExtractionReport` carries a raw delta, explicit
Kotlin/C/generic/heuristic capability and fallback notes. Native cancellation never
silently becomes fallback. Generic AST facts retain scopes and unresolved binding
identities; they do not claim specialized Kotlin/C type semantics. Secondary source
parser registrations merge configurable heuristic definitions behind native
declaration ranges. Unsupported grammars use low-confidence regex facts.

`DefinitionRegistry::load` is worker-only, regular-file/size bounded, and accepts an
explicit definitions path ahead of ResourcePaths. L09 supplies KODE_CODEINTEL_PATH
and surfaces catalog diagnostics plus extraction fallback notes. Registry objects
may be shared immutably; adapter sessions may not be shared mutably across document
workers. L09 owns cache bounds/eviction and workspace/document result gates.

L11 `CompletionContext::at` computes raw UTF-8 prefix/receiver/replacement ranges
from the captured source. CompletionEngine reads a matching-content prepared
snapshot and returns bounded, identity-preserving ranked candidates; candidate
conversion supplies core CompletionItems directly to E09. Import candidates insert
qualified names over the complete typed path; member/plain candidates preserve
receiver/operator/suffix. Unknown receivers have no workspace-text member fallback.
Union suggestions require accessible common members and omit ambiguous identities.
Optional rankers only score candidates. L09 executes this work off the UI thread,
handles explicit stale/limit/cancellation errors and gates completed DTOs against
workspace/document/request/index freshness. No parser is needed in E09 insertion.

L09 `CodeIntelService` is a main-thread coordinator. `observe` captures O(1) Rope
snapshots and coalesces at most 128 live file updates with a 200 ms debounce.
`next_job` returns scan/extract/resolve preparation for the existing F03 bounded
blocking scheduler. `prepared` gates workspace/source epoch; `IndexCommit::run`
checks the epoch again at actual SQLite submission. Accepted transactions drain as
critical work. `committed` publishes affected/removed-file notifications after L05
success; a newer edit remains queued. UI queries additionally carry index/source
epoch gates, including changes in other unsaved documents. Explicit shutdown stops
queries, joins F03 work, drains accepted writes and awaits the SQLite worker.

Intelligence now depends on kode-workspace to compose the W01 scanner and V01 MIME
boundaries. Rescan prunes missing files only after a complete scan. Rebuild resets
frontend sessions; clear replaces only the owned native cache facts. User sources
and legacy H2 files remain untouched. Rescan cancellation keeps independent live
edits and the modal waits for file/index acknowledgement. Unavailable/foreign cache
files remain intact while a visible in-memory fallback serves the workspace.

`NativeFacade` implements core definitions/references/completions/highlights from
immutable snapshots. Current unsaved captures, including other open buffers, may
produce a private query snapshot without persistence/publication. The hot source
catalog and parser sessions are bounded to 32 / 64 MiB of source text each (not a
hard process RSS guarantee). Preview reads prefer captured source over disk.
Virtual file identities serialize owner/key, independently of display labels.
`NativeTokenReport` carries document ID/revision, index revision and dependency
generation. E11 composes its exact byte spans with lexical and interactive layers.

Startup requalifies persisted extraction against current resource overrides once.
Disk captures receive fresh document IDs; editor captures retain actual IDs and
revisions for tree reuse. ParserSession additionally checks raw bytes before an
equal-revision reuse, preventing stale revision-zero disk trees. Source watcher
invalidation ignores cache/session/build metadata to prevent SQLite refresh loops.
Partial parse/fallback/inference notes remain explicit and bounded in index status.
