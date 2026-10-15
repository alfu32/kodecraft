# Rust session format, version 1

Status: implemented by W03. The file is `<workspace>/.kode-rs.json`, UTF-8 JSON.
It is distinct from Kotlin version 6 in `.kode.json`. Rust reads its own file first;
when absent, it imports Kotlin v6 in memory. Saving creates only the Rust file.
Missing both files selects existing conventional `src`, `lib`, `include` roots.
A saved empty root list is explicit and never replaced by defaults.

Root property names use camelCase. All fields except an editor's `origin` have
defaults; unknown properties survive round trips through flattened JSON maps.
Database/REST blocks are opaque ordered JSON until their feature owns validation.
Arbitrary precision JSON numbers, disabled flags, scripts, examples, environments,
escaped values, datasource identities, driver/classpath/provider metadata, SQL
buffers, autocommit, catalog/schema and credential references remain intact.
Runtime connections or HTTP responses are not added by this persistence layer.

| Property | Type / default |
| --- | --- |
| `version` | Integer `1`; other versions rejected |
| `recentFiles` | Array, empty; path, timestamps, optional editor, dirty flag, viewerType `CODE`/`HEX`/`IMAGE`; unknown viewer strings retained |
| `openEditors` | Array, empty; saved document and view snapshots |
| `sourceRoots`, `scanExclusions`, `ignorePatterns` | Arrays, empty |
| `database` | Object with empty `dataSources`/`sessions` by default |
| `restApi` | Version 1, empty default collection/environments and source UI defaults |
| `activeOrigin` | Nullable document origin, default null |
| `importedFrom` | Nullable import provenance, default null |

An editor `origin` is `{"File":"src/file.kt"}` or
`{"Virtual":{"owner":"sql","key":"db:1","label":"sql@local:1"}}`.
Internal file paths are relative to the workspace; explicit external files stay
absolute. Resolution normalizes `.`/`..` without interpreting UTF-8 bytes as path
separators. Filesystem tree actions use a stricter canonical project boundary.
Document IDs are allocated afresh at restore; views in one workspace share identity
through the application's document registry.

Editor metadata: `format` (`utf8_bom`, `line_ending`), `scrollTop` (visual row),
`mime`, `language`, `grammarLanguage`, `grammarAvailable`, `readOnly`,
`lastModifiedMillis`, nullable `diskIdentity`, and unknown properties. Disk identity
uses canonical path, length, modified nanoseconds and SHA-256. Defaults are false,
zero, null and the default UTF-8/LF format as appropriate.
`legacyFormat` defaults false and is true only until a converted Kotlin buffer
has been captured with a real native format/baseline; raw legacy fields remain
available independently. This prevents future CRLF restores losing clean history
because they were repeatedly treated as legacy normalized text.

`history` uses the core HistoryStateV1 field names: `version:1`, `revision`,
`saved_revision`, `current`, `saved_text`, `saved_baseline_known`, `undo`, `redo`,
`omitted_undo`, `omitted_redo`. Each buffer has `text` and directional `selection`
with UTF-8 byte offsets `anchor`/`head`; scalar boundaries are validated. Missing
`saved_baseline_known` defaults true for earlier version-1 snapshots. Unknown saved
baselines remain dirty until actual disk bytes or a completed save establish one;
no fabricated baseline text is required. Core import enforces history limits.
Capture retains at most 30 snapshots per stack under a shared 64 MiB history budget
while retaining current dirty text. Capture/encoding run on a worker using owned
document clones and immutable view metadata.

Kotlin v6 uses camelCase DTOs with absent defaults. `lines` join with LF, including
trailing empty entries. Every current/undo/redo cursor and anchor converts its
zero-based line/UTF-16 column against its own snapshot, clamping negative, oversized
and surrogate-interior coordinates. Current source format/BOM and disk baseline
come from actual disk bytes when available. `legacyBuffer` retains original buffer
fields; unknown fields colliding with Rust property names are preserved under
`legacyEditorFields`/`legacySessionFields`, avoiding duplicate JSON keys.
For a uniform CRLF file, converted legacy current/undo/redo line breaks and their
byte selections are adapted to that disk style once. A clean legacy current
buffer uses actual disk bytes while retaining converted history; inferred legacy
text is not treated as an exact hash baseline. Subsequent native capture clears
`legacyFormat`, preserving CRLF history on later restores.

Normal persistence waits three seconds after a change. Only one write runs at a
time. Completion establishes its sequence and disk identity; changes made while
it ran remain dirty. Failed writes/submission failures retain pending data. Writes
use exclusive creation or a guarded atomic replacement; the previous valid Rust
file is atomically copied to `.kode-rs.json.previous`. Temporary files use restricted
permissions, are flushed, and retain target permissions on replacement.
Orderly shutdown drains critical completions, stops edits, then calls
`SessionPersistence::flush_shutdown` to flush the latest state in a blocking worker.

At restore, current mtime more than 1000 ms newer, changed stored identity, or a
changed known Rust saved baseline causes a fresh disk view. Clean legacy snapshots
may reload differing disk content; inferred dirty legacy baselines alone are not
hash evidence. Dirty stale history is first written by `restore_with_recovery` to
a distinct restricted `.kode-rs/editor-recovery-*.json`. Recovery failure prevents
publication. Cursor/anchor are converted to line/UTF-16 and clamped onto new disk
text. Missing/unreadable/malformed text files retain persisted content and report
the disk error. Clean restored empty snapshots cannot silently erase changed disk
content. A recovery file never changes the source editor file.

Corrupt current Rust files may load the previous valid snapshot with a warning;
the corrupt file stays in place and writes remain blocked. Unsupported future
versions are never rolled back automatically. A 256 MiB session read/write bound
produces visible failure without truncation or deleting source data. Legacy files
and live credentials are never overwritten or copied into test artifacts.

Native filesystem guards reduce conflicting writes but are not a cross-process
compare-and-swap against an uncooperative writer; platform save limitations from
E04 apply. Shell wiring and recovery/error presentation are W05. See
[synthetic Rust fixture](../../fixtures/session/rust-v1.json) and
[legacy fixture](../../fixtures/sessions/legacy-v6.json).
