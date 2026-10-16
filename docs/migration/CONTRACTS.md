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

## Git reads

G01 uses git2 0.21.0 and vendored libgit2 via libgit2-sys 0.18.8+1.9.7. Local
operations enable no SSH/HTTPS transport or JVM. `ReadJob` carries a root, typed
request and limits into F03 blocking workers; real repository/index/tree/blob
handles are opened and dropped there. Only owned models cross back to the UI.
Cancellation is checked before/after native calls and while iterating. A native
libgit2 call already in progress is not forcibly interrupted.

Discovery supports nested roots, file starts, linked worktrees and bare repositories.
Missing repositories are distinct from malformed metadata at the requested root.
HEAD distinguishes unborn/full branch reference/detached commit. Branch names retain
their full hierarchy, correcting the source's last-segment ambiguity. Status rows
retain separate index/worktree changes, untracked/deleted/type-changed/unreadable
files and merge-stage conflict codes. Renames remain add/delete status rows, as in
the source service; status does not enable content-based rename detection. Index
soft refresh/stat updates are disabled, and status never calls the blob-content
reader. Repository bytes and mtimes remain unchanged by reads.

History walks reachable HEAD commits in topological/time order and reports explicit
truncation. Commit labels choose the highest numeric-scored annotated tag, matching
the source's digit extraction, 10-bit slots, signed Long shifts and first-tie rule;
this is not SemVer sorting. Raw tag listing includes lightweight/non-commit tags
and preserves full names. File lists compare the first parent, or an empty tree
for a root commit. Bounded candidates use libgit2's 50% rename similarity; otherwise
exact object renames are detected and edited large renames retain add/delete paths.
Limits are checked via object headers before enabling similarity content loads.

Staged contents compare HEAD/index; unstaged contents compare index/worktree.
Unborn, missing and deleted sides are explicit. Content kinds distinguish text,
binary, oversized, symlink and submodule values. Blob headers are checked before
loading explicit content. Worktree reads are bounded, regular-file guarded and
check metadata for concurrent change; symlink values are read without following
their leaf target, and known parent escapes/absolute/parent/Git-metadata paths are
rejected. Unix paths preserve non-UTF-8 bytes. Raw content bytes retain Git's stored
data and worktree line endings; configured clean filters can intentionally make
index bytes differ from worktree bytes. Read operations do not normalize either.

Defaults: 8 MiB per content side, 100,000 listed entries and 5,000 history commits;
hard maxima are 32 MiB/1,000,000/50,000. Similarity candidates total at most 32 MiB;
ignore-pattern reads cap at 1 MiB. Larger data is visibly classified/truncated.
These bounds do not claim a hard RSS limit inside native libgit2 status/revwalk.
Unsupported non-UTF-8 ref/metadata fields produce contextual errors. Snapshot
refresh preserves per-section failures in its report, allowing other sections to
remain available, including bare/corrupt-object cases.

`GitReadPanel` owns no repository handles. Tagged refresh acknowledgement preserves
selection by path/layer, keeps earlier rows on a whole-job failure, renders section
errors/limits, and rejects disposed/workspace-stale results. Paint never performs
Git/filesystem work. The original throwing-status startup/render regression is
covered by its standalone widget test. G02 owns application-tab routing, staging,
commit editor, write actions and history expansion; G03 owns interactive diffs.

## Git writes and application controls

G02 runs every local write as an F03 critical blocking job. Cancellation is checked
during preparation; after publication acknowledgement reports the actual outcome.
Index operations hold the repository's standard `index.lock`, prepare a private
attached index, then sync and rename the owned lock into place. Failed preparation
keeps the original index; an external lock is never removed. Literal path validation
rejects metadata/parent escapes and unsafe parent links. Files, deleted paths,
unborn HEAD and conflict-stage removal have real repository fixtures.

Commits require a nonblank message (128 KiB maximum), configured nonblank identity,
staged changes, no unresolved conflict and no ongoing repository operation. They
commit the captured staged tree and leave the worktree and on-disk index intact.
Tags are annotated; moving an existing tag requires the explicit move action and
resolves the new commit before updating its ref. No shell, hooks or JGit is used.

Checkout uses safe libgit2 options with ignored-file overwrite disabled. It refuses
tracked local changes, branches used by another worktree, dirty text/binary buffers,
active workspace persistence and changes to this open workspace's session/cache
paths. It locks the index and affected references and never silently forces. A
checkout is not an atomic transaction across all filesystem paths/index/refs: an
I/O failure after worktree publication reports that partial changes need inspection.
Unix lock ownership uses file identity; non-Unix fallback/actual native execution
remain release qualification work. Native calls are not forcibly interrupted.

Alt+5 opens the Git panel. Status changes only after write acknowledgement and a
fresh read. Its virtual commit document uses the shared editor, local completion,
find/history and clipboard. Only a successful commit whose captured document and
revision still match clears the draft; refresh and errors preserve it. Drafts are
not persisted across restart. Commit buttons arm on mouse-down and submit once on
mouse-up inside; leaving the panel cancels drag/armed state. History expansions are
bounded to 32 commits/20,000 cached paths; menus retain immutable snapshots.

Successful checkout invalidates only clean captures in the repository, reloads the
active text/hex/image view and reindexes/rescans without stealing sidebar focus.
Dirty edits made after submission stay in memory. A newer explicit file-open request
keeps precedence and its focus intent; clean hidden caches bypass old session bytes
on next open. Project search waits for checkout acknowledgement to avoid attaching
a stale preview. G02 emits typed status/history diff requests; G03 owns displaying
them and restoring chunks.

## Git diffs and chunk restoration

G03 builds immutable `kode_git::diff::DiffPlan` values in F03 workers. Libgit2's
default xdiff algorithm with zero context/interhunk context supplies one set of
line/byte ranges. Both presentation and restore retain that set; actions never
recompute a different diff and reuse an old ordinal. Its repeated-line alignment
can differ from JGit HistogramDiff: `fixtures/git/diff-algorithm.json` records an
actual JGit 7.4.0 oracle counterexample, pinned by Rust range assertions.

Sources are bounded to 8 MiB each/200,000 combined lines. Missing content is empty
on its corresponding side. Binary/oversized/submodule/symlink content displays a
typed notice and cannot restore. Native diff calls cannot be interrupted midway;
the same native RSS/cross-platform qualification limits as G01 apply. Presentation
is a separate cancellable worker job, bounded to 100,000 rows/16 MiB, with visible
limit errors instead of silent truncation or repeated automatic retries.

Side-by-side projection pairs insertion/deletion/replacement/context rows, original
line numbers, per-side colors and wrapping. Both sides scroll as one prepared
document. `c` toggles context and `v` switches to unified +/- rows; footer controls
also accept mouse clicks. A common Historical editor host supplies the read-only
mutation guard, raw clipboard service, selection, search, navigation and scrolling.
Copy contains the displayed projection, including its numbers/markers. Source-point
maps retain selection and viewport position across reflow where source rows remain
visible. Reflow and replacement acknowledgements reject old tags, including a new
capture at the same width. Failed presentation disables restore until a new valid
presentation is prepared. All paint is prepared data; it performs no Git/file I/O.

Only normal unstaged text comparisons allow restore. Capture records worktree disk
identity; execution checks that identity, exact old index/new worktree contents,
safe repository paths, the original open-document revision and dirty text/binary
buffers. It holds the standard index lock, reserves the file against concurrent
saves/file actions and uses E04's guarded atomic save. Unselected bytes, BOM, CRLF,
final-newline state and existing permissions stay intact. A deleted regular file
can be recreated; removing all added text retains an empty regular file, matching
the source chunk action. Staged and historical comparisons cannot restore.

Accepted restores are critical jobs and drain before quit/disposal. Edits made
after submission remain in memory and retain their old disk baseline, so a later
save cannot silently overwrite the changed physical file. Successful publication
invalidates clean captures of only the changed file, reloads its ordinary viewer
without taking preview/sidebar focus, refreshes Git and reindexes. Leaving Git or
opening another ordinary file dismisses the preview and late diff acknowledgements;
the previous text/hex/image view remains available. A filesystem writer racing the
final identity-check/rename boundary has the same documented limits as E04 saves;
no cross-process filesystem transaction or hard RSS guarantee is claimed.

## LSP management

P01 supplies `kode-lsp::{catalog,discovery,install}`. Catalog/manifest structs retain
unknown fields and command argument arrays. Catalog order chooses the first matching
version/download; platform aliases normalize OS/architecture names. Configured
commands precede manifest commands, then preferred commands; executable discovery
checks unpack subdirectory, install directory/root and PATH. It preserves the invoked
symlink name for command multiplexers. Discovery never chmods external commands or
creates the install root. Installed, manual/update/error state and actual running
state are distinct.

LSP catalog precedence is explicit `KODE_LSP_CATALOG`, F02 resource roots, then
compiled defaults. Reads are regular-file guarded and limited to 1 MiB; manifests
cap at 128 KiB. Explicit Rust resource roots exclude legacy roots. Install root is
`KODE_LSP_HOME`, Rust resource home/lsp, legacy home/lsp, then Rust config/lsp.
Catalog and discovery run on workers; settings consume owned reports.

`InstallJob` runs on a bounded F03 blocking worker. `HttpFetcher` drives cancellable
Reqwest futures on the existing Tokio runtime, without a private runtime/thread
pool or shell. Downloads cap at 256 MiB, extracted data at 1 GiB, individual files
at 512 MiB and archive entries at 100,000. ZIP/TAR/TGZ/GZIP/raw packages preserve
command arguments, honor unpackSubdir and supplied SHA-256, and validate a staged
command before publication. TAR metadata gets a 64 KiB preflight bound. Traversal,
links/devices/sparse entries, duplicate files and reserved manifests fail explicitly.
Installation publishes a complete staged directory, retains the old directory and
rolls back failed replacement; uninstall removes only the selected server directory.
Root locks serialize operations. A crash-retained lock/stage/previous directory is
preserved for inspection, not silently removed.

LSP settings actions are acknowledged F03 jobs. Install/removal/lifecycle work is
critical; its own cancel token aborts preparation, but a started publication finishes
or rolls back before acknowledgement/shutdown. `ServerLifecycle` delegates
start/stop/restart and running state to P02's process registry; settings never fake
running flags. A missing process service is an explicit error. Workspace changes
drain jobs and stop registered services; old owner/request/workspace results cannot
populate new settings. See `docs/parity/P01.md` for evidence and limits.

P02's `LspService` owns at most 16 ordered server clients per workspace. Discovery
does not auto-start them. Command arrays launch directly through F03's managed
processes. Settings only show running after initialize/initialized succeeds; failed
starts and later crashes retain errors, trigger repaint and cannot masquerade as
running. Restart and workspace disposal await child shutdown/reaping.

Each client has tracked reader, writer, document and supervisor actors on the
existing Tokio runtime. Frames use UTF-8 byte-counted Content-Length, strict CRLF,
8 KiB headers and 16 MiB bodies. The writer queue holds at most 32 frames/32 MiB;
63 user requests leave a reserved internal shutdown slot. IDs are unique and
responses can arrive out of order. Timeouts, cancellation and dropped request
futures remove pending entries and attempt $/cancelRequest. Server errors are
request failures; malformed transport terminates the client. Stderr keeps a 64 KiB
tail; notifications keep 32 bounded events. Configuration/workspace requests have
real replies; unknown client methods return -32601. Server-initiated edits and
window navigation explicitly decline, and dynamic registration is not advertised.

Position encoding is negotiated (UTF-16 by default; UTF-8/UTF-32 supported). The
protocol mapper recognizes CR/LF/CRLF without normalizing document bytes, excludes
newline characters from columns and never sends half a CRLF edit. Client document
versions are separate from editor revisions. Actor-serialized open/change/save/
close supports negotiated full/incremental sync; older revisions are rejected,
document identity changes reset the replica, and accepted writes finish even if
the caller drops its reply future. Limits are 64 documents, 8 MiB per source,
1 million protocol lines and 64 MiB for all source/index captures per client.
Virtual documents remain a native-provider responsibility. Typed definition,
reference and completion requests check server capabilities. P03 composes editor
attachment, current-query gates and native fallback below.

Normal shutdown closes documents, sends shutdown then exit, and forces process
group termination/reaping when deadlines expire. A failed close cannot race a late
open against shutdown. Actor failure or dropped ownership also cancels the
supervisor. Defaults are 10 s startup, 800 ms request, 2 s pipe write, 1 s shutdown
and 500 ms exit grace; no UI input or render path waits for a server response.

P03's `EditorLsp` receives O(1) immutable rope captures from file/search hosts and
the workspace document registry. Captures prioritize attached hosts and cap at
64 documents/64 MiB, with 8 MiB per source. Unsupported/overflow/virtual sources
use native intelligence. A coalesced F03 job attempts the first matching local
server on a new file attachment, then synchronizes selected replicas. Discovery
alone does not launch processes; editor attachment never installs packages. Manual
stop/crash does not cause an immediate restart loop; a later file attachment or
explicit start can retry. Server language aliases use the same protocol mapping.

Worker queries synchronize selected captured buffers before the request, so a
target in another unsaved file has the same replica revision as its byte mapper.
Negotiated coordinates convert Locations/LocationLinks to exact editor bytes and
UTF-16 navigation locations. Other sources use bounded regular-file reads with
change checks. Non-file locations are excluded. Missing UTF-16 sources retain their
portable location/source-unavailable state; UTF-8/32 locations without readable
source fall back rather than inventing byte offsets. Target conversion caps at
1,000 locations/64 sources/64 MiB. Definition previews retain native highlighting.

Current non-empty primary results win. Empty/error/unsupported/stale primary
results call the native provider; lists are not merged. Host suggestions still
precede the composed completion provider per E09. LSP completion preserves server
order and details, deduplicates labels and caps at 50. Plain insertion and text
edits matching the typed prefix work; snippets, additional edits, commands and
insert/replace extensions are not advertised or applied. Unusable items permit
native fallback. Unicode insertion preserves receivers, suffixes and saved bytes.

Source changes cancel in-flight LSP requests; scheduler cancellation and workspace
disposal also cancel them. Identity/revision, server identity/state and replica
version are checked before/after conversion. Application publication checks source
epoch and registry revision again, covering changes after a worker queues its
reply. Existing owner/focus/selection/context gates remain authoritative. An
entirely primary operation does not depend on native index publication; mixed or
fallback operations retain the native epoch/index gate. New captures are observed
before stamping a query, and newly installed cached hosts receive providers before
the next input event. A physical save reports actual saved bytes even if a newer
unsaved replica already exists; LSP failure cannot invalidate a successful save.

Diagnostics remain bounded transport events; the frozen facade's diagnostic,
document-symbol and prepare-rename methods are empty/default interfaces, not
implemented editor features. Protocol hover, symbol/diagnostic decoration,
rename/refactoring and server edits are not advertised. Existing delayed
definition previews use the real composed definitions provider instead.

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

## Database models, catalogs and private configuration

D01 supplies native datasource/driver/artifact/session/status/metadata/result models
in `kode-database::model`. Known fields retain Kotlin spelling/defaults on disk;
unknown configuration fields survive import/export unless they contain reserved
credential fields. Local multi-JAR paths remain unchanged and are split with an
explicit platform separator by the resolver. Models/configuration never start Java.

`resources/database/drivers.json` freezes all 59 source registry definitions in
declaration order, including aliases, classes, classifier/artifact coordinates,
custom Maven repositories and vendor page-only sources. No vendor artifact URLs
are invented. URL morphology is copied exactly from the frozen source. The catalog
validates coverage, unique IDs, inheritance/provider cycles, effective template
IDs, JDBC schemes, sensitive placeholders and unsafe TLS defaults. Effective lists
share immutable templates, including inherited aliases. Registry lookup preserves
source prefix order; provider artifacts and recents use their source families.
`scripts/database-catalog.py --check` compares against the frozen source tree.

`DatabasePaths` discovers paths without loading or creating configuration. Rust
user data defaults to `<Rust config>/database`, separate from the legacy Kode root.
`KODE_RS_DATABASE_HOME` overrides it. `KODE_CONFIG_HOME` identifies the legacy import
root; XDG/APPDATA/home fallbacks match the source. Plugin paths use
`KODE_RS_JDBC_HOME`, explicit legacy `KODE_PLUGINS_HOME/jdbc`, then the Rust plugin
root; legacy default plugin locations remain available for D03 reuse. Import is
explicit and never edits source files or replaces existing Rust configuration.

Rust files are `datasources-v1.json`, `credentials-v1.json` and
`jdbc-url-recents-v1.json`. Writes hold an OS advisory lock, use E04's guarded atomic
save and retain errors/corrupt/future files instead of replacing them with defaults.
Known symlink traversal and nonregular lock/data targets are rejected. File locking
uses stable Rust APIs (available since 1.89), with real contention distinct from I/O
failure. Configuration methods are synchronous worker services; D07 must schedule
them through F03, not call them from paint/input.

Credential values live only in the separate private UTF-8 JSON store. New files
use mode 0600 on Unix, created directories use 0700, and reading a credential file
with group/other permissions fails explicitly. `Secret` has redacted Debug/Display
and no Serialize implementation; connection wire code must explicitly expose it.
Errors do not echo URLs, property values or malformed credential bodies. This is
restricted-access local storage, not encryption or a claim of memory zeroization.
Windows inherited ACL behavior/native execution remains D09/Z04 qualification.

Legacy properties import follows actual `java.util.Properties.load(InputStream)`:
Latin-1 bytes, logical continuations, comment rules, escaped separators/whitespace,
duplicates/unknown escapes, and UTF-16 Unicode escapes. Valid surrogate pairs are
preserved; unpaired units cannot be represented losslessly and are rejected with
the original retained. A shared synthetic fixture is checked by Rust and an actual
JDK oracle (`scripts/test-java-properties.py`). No real credential/session file is
used for qualification.

Legacy driver properties identified as credentials move to deterministic private
references in `credentialProperties`; existing `credentialReference`, identifiers,
URLs/classes/coordinates/JAR lists and safe properties remain unchanged. Reference
collisions reject before publication. Credential-bearing URLs are rejected rather
than rewritten. Private data publishes before public definitions: a late public
I/O failure may leave private orphan refs, but cannot publish lost auth values.
This is not a cross-file filesystem transaction. D04 must inject referenced secret
properties/credentials only into ephemeral connection requests in source order.

Recents preserve trimmed, exact case-sensitive URL values, deduplicate exactly,
keep 12 entries per family in MRU order and retain source seed order. Alias families
share the same store. Known credential parameters/userinfo, Oracle embedded auth
and encoded credential keys are rejected; safe connection parameters remain intact.
Legacy recents import filters unsafe entries without modifying the legacy file.

The application composition layer applies the database feature's export validator
before capture and before any session save/backup. `SessionStore::with_validator`
keeps the generic workspace crate independent of database models and retains opaque
feature JSON. Unsafe payloads fail visibly with original files intact; they are not
silently stripped or exported. Typed datasource/project export also enforces the
policy. D07 owns database import/dialog integration and explicit correction of
unsupported credential-bearing definitions; D02–D09 own JDBC process/execution UX.

## External JDBC bootstrap and process supervision

D02 implements the [v1 wire contract](../../protocol/jdbc-v1/README.md), envelope
and operation schemas, shared Rust/Java fixtures and query value DTOs. Numeric,
decimal and temporal previews remain strings, binary remains base64, and SQL NULL
is a separate cell kind. Feature services enforce method semantics in D03–D06;
a listed schema does not advertise a capability before its handler exists.

The Java 21 helper is a separate shaded JAR with pinned, relocated Jackson. It
captures protocol stdout and redirects System.out to stderr before constructing
the server or loading a driver. Input reading, ordinary dispatch, control dispatch
and frame writing are independent and bounded. Cancellation acknowledgements have
priority; a query's final response shares its events' FIFO. Full ordinary output
queues wait with cancellation checks; control overflow disconnects visibly.

`bridge::runtime::Runtime` is constructed without I/O. Its first explicit get
checks Java (configured executable/KODE_JAVA, JAVA_HOME, then PATH), requires 21+,
finds KODE_JDBC_HELPER or the packaged `jdbc/kode-jdbc-bridge.jar`, and validates
hello before exposing Running. One workspace owner reuses one helper. Startup,
probe, writer, reader, stderr drainage and reaping are owned even when a future is
dropped. Workspace shutdown joins startup and live actors. Explicit restart creates
a new epoch; no SQL, commit or rollback is ever replayed after failure.

Rust reserves control request slots/bytes separately from ordinary work. IDs are
fresh within an epoch; stale replies cannot satisfy later calls. A queued request
can atomically become abandoned before writing. Once writing may have begun,
failed/cancelled/timed-out mutating requests have unknownOutcome unless the helper
explicitly proves they did not start. Dropping a wait cannot prove JDBC stopped:
query.cancel/driver.cancel operate independently by operation ID. Typed SQL result
and transaction services own final outcome interpretation in D04–D05.

The reader drains independently of rendering, retains bounded notifications, and
disconnects explicitly on notification overflow while keeping the received preview.
Query consumers must drain independently of paint; progress can be coalesced by its
owner. Public errors and Debug never include request bodies or remote error data.
Remote structured data is available only through an explicit feature decoder path.
The 64 KiB stderr tail is suppressed in public diagnostics because drivers and JVM
launchers cannot be trusted to redact their own output. Shutdown has bounded grace,
then kills/reaps the owned process group through F03. Native process-tree behavior
on Windows/macOS and actual JDBC/vendor interactions remain D09/Z04 qualification.

Database configuration guards explicitly unlock before closing. A briefly inherited
or duplicated open description must not retain a completed writer's flock, and
closing that old duplicate must not release the next writer's lock. Lock paths
remain persistent; guards never unlink another writer's lock file.

## JDBC artifact resolution and loader ownership

D03 adds `driver.resolve`, `driver.cancel` and `driver.inspect` plus the native
installer/progress model and inert install modal. Full connection/dialog/panel
composition follows in D04–D09. The complete 59-entry catalog remains selectable.
Bundled H2 2.2.224 preserves the source's visible default; it never substitutes for
a different explicitly requested version.

Maven Resolver 2.0.23 supplies the real runtime graph (parent POMs/BOMs/scopes,
optional exclusions/classifiers). Source stable-version ordering is retained and
selected versions/classpaths are pinned. Only the configured repository or Central
is consulted; user Maven settings and POM-declared extra repositories are not
implicitly imported. Required descriptor failures remain visible. Runtime users
need Java, with no Maven CLI. Local multi-JAR and legacy provider/version sources
remain supported; page-only vendor entries stay manual.

Installs stage privately, hash/flush complete JARs, publish an immutable directory
and atomically select it under a provider lock. Manifests/pointers retain ordered
paths/digests/version/source, share alias families and verify bytes on reuse. A
family/version lookup recognizes an initial latest install after the dialog pins
its resolved version. Old/in-use and original local/legacy artifacts are retained.
Normal failure/cancellation cleans only owned temporaries; a late cancel cannot
undo a published success. Hard-kill recovery remains D09/Z04 qualification.

Driver loaders isolate dependencies under the platform parent, cache by provider,
ordered digest and location, and support concurrent versions of one provider.
Connection leases defer closure; capacity eviction selects only unused loaders.
Cleanup code defined in the child unregisters only that loader's DriverManager
registrations. D04 uses its intended Driver.connect and retained context loader.
Native linkage errors require explicit helper restart; Java close does not claim
native-library unloading. Driver inspection can locate installed specs without
download or directory creation and reports actual classes/load-error types.

Rust progress subscriptions are separate from query consumers and coalesce by
resolution ID. Transport budgets remain shared and bounded; query data/final
outcomes are not coalesced. Cancellation waits for the original install final
response. The modal has no I/O in paint/input, gates completion by workspace/job,
and never treats progress or a cancellation acknowledgement as completed work.

## JDBC connections and transaction state

D04 owns Java `connections/` and native `connections::{Connections,SessionInfo,
Snapshot,Context,TestResult,Error}`. Construction/status reads perform no I/O.
Explicit test/open inspect the selected installed driver without downloading,
resolve all credential references from one bounded private snapshot off the
application thread, and send only ephemeral properties. Public properties precede
private property references, nonblank username and password override/reference.
No prepared credential-bearing payload implements a persisted settings contract.

One connection per datasource has its own serialized owner; consoles share its
transaction. Direct Driver.connect retains its loader lease and thread context.
Test uses a temporary connection and closes it before replying. Context setters
verify actual driver state, apply autocommit last and expose partial failures.
Explicit null catalog/schema restores login defaults; omitted fields stay put.
SQL failures in manual transactions conservatively require rollback before commit.
Read-only is a driver hint; ignored setters fail rather than claiming enforcement.

Close has an independent control route, reserved owner slot and bounded abort
executor. It cancels queued work before start; JDBC close stays on its owner.
Unconfirmed close retains the lease and requires helper restart. Confirmed close
with a driver exception still returns that error with `data.closed: true`.
An attempt ID/tombstone prevents priority cancellation overtaking open dispatch
and creating a hidden connection; close by attempt cannot affect a newer session.
Bounds and wire fields are documented in `protocol/jdbc-v1/README.md`.

Connection state is tagged with helper epoch and connection ID; context/state
revisions prevent stale context or independently delivered replies from restoring
older state. SQL errors retain bounded redacted message/SQLSTATE/vendor/chains.
Unknown context is explicit and invalidates context assumptions. Helper death
invalidates session IDs and marks uncertainty without starting another helper.
Abandoned transaction waits require explicit disconnect before further commands.
No SQL/commit/rollback replay exists. Workspace shutdown cancels/drains tracked
operations, stops installer consumers and shuts down/reaps the helper.

D05/D06 must submit all execution/metadata work through `ConnectionService.submit`
and its owner-only Session accessors. Execution cancellation hooks must enqueue
independent statement cancellation without blocking control. D06 keys metadata
by epoch/connection/context revision and invalidates on unknown context/failure.
D07 must await `Connections.definition_changed` before saving even an equal
definition, because private values may change under an unchanged reference.
Composition supplies process-state repaint notifications; paint never starts Java.

## JDBC query streaming and cancellation

D05 owns Java `queries/{QueryService,CellPreview}` and native `execution::{Executor,
Snapshot,Preview,ResultSet,Options}`. SQL work uses D04 Session ownership, including
statement creation/execution, result iteration, warnings, value conversion and
closure. Statement.cancel runs independently against the registered execution's
own statement; its lifetime and loader extend through the cancel call. Owners wait
before reusing the connection so a late cancel cannot target the next query.
Requested/unsupported/alreadyFinished acknowledges control only; the original
execution final is authoritative. Hanging cancellation offers explicit disconnect
or helper restart. Neither cancellation nor process death implies rollback.

Notifications preserve execution ID/sequence and typed nullable text/binary cells.
JDBC large update counts remain i64; decimals/temporal values remain strings.
Result batches split by rows/encoded bytes, cells/rows/metadata have explicit bounds
and LOB/XML/array previews close/free on the owner. Later SQL failure retains prior
received results, warnings and bounded SQLSTATE/vendor/chained errors. Detailed
bounds and fallback semantics are in `protocol/jdbc-v1/README.md`.

Final `eventCount` is a notification barrier. Native collection waits for those
events even if the RPC future wakes first or the helper exits after its valid
final. Query notifications have a dedicated consumer, not a paint-time drain.
The native state machine validates order/shapes/bounds; malformed events fail
visibly with existing rows retained. Per-preview/global retention limits remain
charged across immutable clones; exhaustion requests cancellation and marks an
explicit incomplete/limited preview. Cell contents and SQL errors never enter
Debug/Display diagnostics. Actual query values remain available to result UI.

`Executor::execute` captures SQL and shared datasource session, publishes identity,
chunks and cancellation status, and waits for its own final. Dropping the UI wait
routes cancellation; executor shutdown drains its native tasks. Explicit cancel
retry keeps that identity. Helper loss/stale session never reconnects or replays.
Owner connectionState/stateRevision updates native transaction/context state only
for the matching current epoch/ID; unknown context invalidates use. D06 owns cache
invalidation after relevant executions. D08 owns SQL targeting, retained editor
text and result presentation; no JDBC terminal UI is claimed by this backend.
Workspace disposal shuts down the executor before Connections/runtime teardown.

## JDBC metadata and cache identity

D06 supplies Java `metadata/MetadataService` and native `metadata::Metadata`.
All introspection and ResultSet cleanup uses D04's serialized connection owner.
Normalized D01 request/result/object/capability DTOs retain source names, category
attributes, leaves and catalog fallback behavior; table children also expose
imported foreign keys. Unsupported accessors are visible messages/capabilities,
not successful empty placeholders. Packages report the standard JDBC limitation.
Routine expansion reads parameters without asking for unrelated table keys.

`Metadata::new`, `cached`, `is_current`, `invalidate` and `refresh_all` never start
Java or touch credentials. `introspect` requires an explicitly connected session,
uses a bounded tracked worker and caches immutable results. Helper epoch,
connection ID, context revision, native connection revision and complete request
identify entries. Explicit refresh rejects outstanding pre-refresh replies; even
an unrelated simultaneous refresh may conservatively force a pending read to
retry, while already cached unrelated nodes remain available. Every SQL submission
and final bumps native connection revision, conservatively invalidating metadata
for statements/procedures whose effects cannot safely be inferred from SQL text.
Metadata SQL failures apply authoritative transaction state from their owner.

D07 must check `Metadata::is_current(source, request, result)` alongside its
workspace/request tags immediately before mutating the tree. A result that was
valid when its future completed may be stale when the application processes it.
Cache eviction can conservatively require a retry. Shutdown cancels/joins Metadata
before Connections; canceled native waits do not pretend to interrupt arbitrary
JDBC metadata. Detailed limits, protocol fields and shared fixtures are in
`protocol/jdbc-v1/README.md`.

## Database explorer and application composition

D07 supplies application-thread `DatabasePanel`, `DatabaseTree`, connection and
object dialogs, composing D03's driver-install modal. Alt+6 lazily loads Rust
definitions on an owned job; ordinary startup neither reads user database files
nor starts Java. Configuration/resource failures remain database feature errors
and leave native file editing operational. Native JDBC operations use F03's async
scheduler, with critical completion delivery and workspace tags. Configuration,
credentials and URL recents use blocking workers. Paint/input never performs I/O.

Save closes even an equal definition before changing private credentials. New
passwords use fresh references; failed public publication cleans up the owned new
entry without changing the old reference. Old/shared references are removed only
after successful publication and only when unused by remaining definitions.
Driver progress/cancel retains the original final; modal dismissal restores
parent fields and focus. Clipboard paste proofs check dialog, field, value,
selection and caret. Session rows refer to real shared editor documents and
preserve those documents when switching views; D08 owns their persisted manager,
targeting, execution/history and result UI.

Metadata publication checks both its node's tag and `Metadata::is_current`.
Refresh prunes obsolete descendants and cancels their loads; explicit refresh
conservatively invalidates the datasource cache. Display retention is bounded to
256 loaded nodes / 50,000 normalized objects / 32 MiB estimated object storage,
with explicit capacity/nesting errors, rather than a hard RSS guarantee. Closing
a branch releases its display subtree; native caching can still satisfy reopen.
Workspace disposal cancels panel jobs, stops metadata/connections/helper and joins
scheduler work; accepted config saves/deletions finish before leaving. Service
restart is explicit (`R`), invalidates old connection IDs and never replays SQL.

## SQL consoles and result ownership

D08's inert `console::Manager` owns stable `source:counter` session identities,
context preferences and bounded history (512 per console, 4,096 entries / 16 MiB
SQL text globally). Documents own current buffers. Capturing persistence clones
Arc metadata and rope snapshots; the blocking capture worker materializes buffers
from the same application sequence and preserves unknown fields. Restoring a
session never opens a connection, starts Java, restores a result or replays SQL.

`statement::{locate,script,statements}` borrow source text and use UTF-8 byte ranges.
Selections retain their actual offsets, including repeated text. Current at EOF
never falls back to the entire script. Quoted semicolons and comments are scanned;
PostgreSQL dollar/E strings and default MySQL escapes/comments are supported.
This is a statement locator, not a complete vendor procedural grammar. Explicit
selection/script remains available for nondefault SQL modes and procedural blocks.

Execution captures the host snapshot/selection, targets SQL on a blocking worker
and uses the native tracked executor. `execution::Options` has optional nullable
catalog/schema: absent preserves context; explicit null restores login defaults.
The helper applies context and SQL within one serialized connection job. Console
bindings resolve through datasource defaults; another console cannot accidentally
inherit a previous query's schema. Transactions remain shared by datasource.

Query results and cancel retries publish only to the original console/request.
Editing or changing focus does not discard a valid execution result. Results use
bounded immutable chunks, sampled widths, separate result/messages tabs, explicit
NULL/binary/truncation/incomplete/unknown/cancel status and independent focus.
Alt navigation preserves editor selection; ordinary arrows stay with the focused
owner. Search and editor popups precede SQL shortcuts. Hover targeting uses a
replaceable worker and validates document revision/selection before highlighting.

SQL export uses an owner save snapshot and native guarded atomic file I/O. A
known exported path uses its last disk identity; external changes and existing
unowned targets fail visibly without replacement. Failure retains the path and
editor buffer. Unsubmitted save dialogs are dismissed on workspace closure while
the SQL buffer is persisted. Accepted file saves finish before leaving. Executor
shutdown precedes metadata/connections/helper shutdown and scheduler join.

## REST model ownership

R01's `kode_rest::model::Workspace` owns one raw Postman v2.1 JSON collection,
typed environment/UI metadata and a mutation revision. `ProjectState` snapshots
share immutable collection/environment Arcs and retain arbitrary extra fields.
Collection edits preserve unknown JSON data, object-valued variables, disabled
entries, scripts, saved examples, ordering and `x-kode-*` fields. The model never
executes scripts and has no runtime response, cookie or execution-handle fields.
Saved examples in the imported collection remain collection data.

`NodePath` uses item-array indices, including positions occupied by unknown
nonobject entries. Invalid encoded paths return an error rather than dropping
components and targeting another item. Tree traversal skips nonobjects for
display, while export/copy retains them. Mutations rebase selected paths and move
destinations after array removal/insertion; moved request identities are preserved,
copied request identities are fresh UUID v4 values. Tree operations retain up to
100,000 nodes / 64 levels; this is a model bound, not a hard RSS guarantee.

Environment values retain enabled/type/description/secret/extra metadata and
structured JSON values. Enabled variables have ordered last-value-wins lookup.
The text codec matches source trimming/comment/escape behavior for `key=value`
lines; it escapes = and backslash. Text edits merge existing metadata by key and
occurrence, preserving an unchanged structured value's JSON type. Structured APIs
represent values outside that line syntax. Secret is display metadata and does
not imply encrypted storage; diagnostic Debug output omits environment values.
R03 owns file import/export/OpenAPI, R04 owns runtime HTTP/cookies, and R08 owns
application persistence and runner integration.

## REST request resolution

R02's `RequestResolver` reads a captured collection/path, project root and optional
typed environment. It supplies effective variable/header/auth rows for the UI and
pure `plan()` output. Scope precedence is collection→ancestor→request→environment;
disabled definitions do not shadow an earlier enabled value. Substitution returns
ordered unresolved/cycle sets and preserves literal dollar/backslash text.
Resolution is bounded to 64 nesting levels, 10,000 substitutions and 4 MiB per value.

Headers follow source case-insensitive last-enabled-value override semantics;
auth replaces and appends its own header. Effective row access retains duplicates,
disabled flags, source and editability. Nearest noauth/basic/bearer/apikey is
implemented; unsupported authentication and body modes fail before execution.
Path parameters are encoded as path data, query/form entries use form encoding,
fragments remain separate, and SSE intent survives URL normalization. Timeout,
redirect and TLS preferences are carried per request.

`BodyPlan` covers empty/raw/urlencoded/file/multipart/GraphQL without filesystem
I/O. `ResolvedRequest::materialize(limit,cancel)` runs explicitly on a blocking
worker, validates/reads regular files with cancellation and size/change checks,
and returns `PreparedRequest` with captured immutable bytes. The caller supplies
its buffering budget; errors never return a partial outgoing body. Multipart
names/filenames are quoted, explicit boundaries are honored, missing boundaries
are supplied in the resolved header, and payload delimiter collisions fail.
Missing/unreadable paths and other errors omit resolved secret values.

Prepared request previews use the same captured bytes, canonical Host and actual
Content-Length, with a caller-selected display cap. They are logical HTTP views,
not packet captures, and never mutate outgoing bytes. Request/header/auth/preview
Debug implementations omit values. HTTP/cookies/redirect execution is R04; SSE/WS
is R05. These pure model services never start networking themselves.

## REST import/export

R03's `import::Import` is a captured Postman collection plus source kind and
structured pointer/code/message notices. Parsing/conversion is pure; bounded
regular-file reads and guarded atomic exports run on blocking workers. Root
Postman import replaces the collection. Nested imports retain collection scope
and arbitrary fields. OpenAPI imports append tag folders and preserve the source
document/notices once under `x-kode-openapi`; operation items have fresh UUIDs
and source pointers. `Workspace::append_items` validates a candidate atomically
without changing existing indices, identities or selection.

Local JSON pointers are resolved with cycle/depth checks. No external reference,
example, include or credential acquisition can perform I/O during conversion.
Unsupported conversion has explicit notices; affected request items carry
`x-kode-import-incomplete`, which `RequestResolver::plan` rejects. R06/R07 must
show the notices and require correction/explicit acknowledgement before clearing
that marker. Supported security maps to unresolved credential variables; absent
OpenAPI security maps to noauth, avoiding destination authentication inheritance.

Import/export bounds are 16 MiB, 500,000 JSON values / 96 JSON levels (including
metadata and the nested destination), plus existing model tree limits. YAML has
additional event/anchor/alias budgets and preserves exact decimal number spelling
through source spans. Parser errors omit source snippets/values. Exports accept
only collection data, retain saved examples and require expected disk identity
to replace an existing file. Runtime results/cookies remain separate owners.

## REST HTTP execution

R04's inert per-workspace `http::Executor` consumes a captured `PreparedRequest`,
`ExecutionKey { tag: JobTag, item_id }` and independent `CancellationToken`.
The caller schedules/polls its native async future and cancels it on workspace
disposal. Editing/switching requests cannot retarget its immutable result.
Validation errors return before network I/O. Network/cancel/timeout outcomes
return typed responses with partial headers/body/counts retained. Non-2xx is
response data. No failure is retried; redirects obey their explicit setting.

Responses contain original/normalized initial request captures and normalization
notices, duplicate response headers with raw value bytes, runtime Set-Cookie data,
status/final URL/timing, capped raw body and total received counts. Logical previews
use captured bytes and normalized initial headers/auth/cookies. Redirect hops and
TLS frames are not packet captures. Expect/Upgrade/framing/authority are transport
managed, with omission/replacement notices. Formatting never changes raw bytes.

Cookie jars are runtime-only and expose inspection/clear/rejection-count APIs.
Domain/host-only/path/secure/expiry matching uses cookie_store with a static PSL
check and cookie-prefix rules. Explicit Cookie overrides jar values. Retention is
1,024 cookies / 1 MiB including effective domain/path metadata, with 16 KiB per
incoming cookie header and oldest-update eviction. HTTP limits are 256 headers /
256 KiB header data and 64 MiB outgoing body/maximum response preview (10 MiB
default). Request settings control redirect/timeout/TLS independently. These are
bounded captures, not hard RSS limits. R05 extends this boundary for SSE/WS;
R07 owns UI execution routing and owner lifecycle integration.

## REST streaming execution

R05's `stream::Executor` shares the HTTP owner's runtime cookie jar and accepts
the same execution key/request/token. `event_channel()` supplies at most 64 queued
events; a closed owner prevents connection, and cancellation interrupts reads or
backpressure. The returned tagged `Response` is authoritative completion. Normal
HTTP publishes on completion; SSE/WS publish incrementally. UI publication follows
workspace/item/execution identity rather than active focus or editor revision.

Events expose normalized initial request/notices and opened response metadata,
retained raw chunks (≤16 KiB), parsed SSE fields, replaceable progress counters,
display-limit notices and close details. Consumers append raw bytes, replace
counts, cap formatted/event-history displays and associate old responses with
their original request. Runtime events/results/cookies never enter project export.
Raw counts continue beyond preview truncation. SSE typed history has an independent
preview-sized byte budget / 4,096 records, with an explicit display-limit notice.

SSE accepts only bounded valid UTF-8 prefixes after split-BOM handling; raw bytes
remain separate. Limits: 64 KiB line, 256 KiB unfinished/parsed event, 1 MiB input
chunk. WS bounds are 64 KiB/frame, 256 KiB/message and 512 KiB write buffering.
Tungstenite assembles valid fragments before message publication; chunk events
retain text/binary identity and message-end flags. WS counts complete accepted
message payloads rather than wire bytes or rejected partial messages. Close code/
reason and failures retain previous data. Normal 101 WS completion is successful.

Connection/handshake timeout is distinct from established stream lifetime; live
streams continue until close/EOF/cancel. There is no reconnect or failure replay.
WS independently applies auth/cookies/redirects/TLS, strips cross-origin sensitive
headers and refuses downgrade. Certificate bypass is per-request; cryptographic
signature verification remains active. Default WS opening sends no body, matching
source. `InitialMessage::Text|Binary` is an explicit bounded extension. R07 must
make handshake/body normalization and source receive-only behavior visible.
