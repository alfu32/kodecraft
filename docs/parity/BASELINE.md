# Kotlin reference baseline

Reference: `../kode`, commit `7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68`.
The tracked source was clean before the audit. Kotlin 2.2.20, Java 21.0.9,
Gradle 8.14. Baseline command: `./gradlew test --no-daemon --console=plain`.

On 2026-10-14 the complete suite ran **153 tests: 152 passed, 1 failed**.
`TypescriptIdentifierPathScanTest.scansIdentifierPathsInTypescriptCorpus` fails
because `samples/.corpus/typescript` is absent. The test walks an optional corpus
rather than a portable checked-in fixture. L10/L12 must use a synthetic TypeScript
corpus; the missing-corpus outcome is not a parser failure or a Rust acceptance
expectation. Detailed per-test outcomes are in [kotlin-results.json](kotlin-results.json).

## Inventories and replay inputs

- [Source inventory](source-inventory.json): 127 production files and 44 test/support
  files, exact SHA-256 hashes, line counts and migration ownership. Regeneration
  rejects an unknown baseline or any unowned source/test file.
- [Named test inventory](test-inventory.json): 153 executable test methods and owners.
- [UI decision inventory](event-branches.json): conservative superset of 2,675
  decision sites in source components using `UIEvent`, including helper decisions.
  Each retains its executable condition, function, source location, owner and
  applicable editor acceptance cases. `BEH-<component>-<line>` identifies the
  source replay obligation for component behavior beyond the shared editor.
  These are migration obligations, not passing Rust tests. Owning tasks must
  reduce the applicable case sets to exact event/state assertions as they port
  their component. The original source at the recorded hash is the replay oracle.
- [Editor cases](editor-cases.json): all 110 UX rows, required outputs and owners.
- [Resource inventory](resource-inventory.json): exact shipped catalogs, styles and
  native assets with sizes and hashes. Catalog adoption retains all entries;
  asset presence alone does not establish platform support.
- [Fixture manifest](../../fixtures/manifest.json): origin, license, hash and expected
  behavior for synthetic documents, legacy state, collections, schemas, languages,
  SQL, bytes/images and copied MIT catalogs. No live developer session or secret
  file was read. Synthetic fixtures are dedicated to this migration under CC0-1.0.
- [Named editor sequences](../../fixtures/editor/event-sequences.json): newline
  aliases, reverse selection/paste/history, modal focus, popup dismissal and wrapped
  pointer capture. Owners supply state/cell assertions at 80×24, 120×40, 20×6 and 1×1.

## Executable command and application inventory

| Reference command / branch | Behavior to preserve | Owner / acceptance |
|---|---|---|
| No arguments; `help`, `-h`, `--help` | Print help without starting terminal/runtime | F02/W05 |
| `version`, `-v`, `--version` | Print build version | F02/Z03 |
| One file target | File's parent is workspace root; explicitly targeted file wins over saved active file | W05 / UX-029 |
| One folder target | Restore workspace and state; source roots limited to configured folders | W01/W03/W05 / UX-026–028 |
| Too many or missing arguments | Visible argument error and help | W05 |
| `cat <file>` | Syntax-colored noninteractive file output | W05/L02/V01 |
| `install` | Generate local platform launchers | Z03 |
| `update` | Update from release distribution; preserve operational error reporting | Z03 |
| `serve [options] [folder] [-- ttyd args]` | Port 11045, loopback bind, optional public/credential, forwarded arguments | Z01/Z03 |
| `serve --help` | Non-destructive help before resource discovery | Z01 |
| Global quit/workspace switch | Flush session and dispose owned jobs/hosts | F03/W03/W05 / UX-108 |
| Shell escape | Suspend terminal/input, run shell, restore and redraw | F04/Z01 / UX-107 |
| Panels/viewers/dialogs | File, project, Git, database, REST, settings, help, about, workspace picker, rescan; explicit focus and dispatch | W02/W04/W05/G03/D07/R06/Z02/F05 |
| SQL host dispatch | Selection/current statement/all; transactions, result focus, independent cancellation | D04–D08 / UX-101/104/106 |
| REST host dispatch | Request/body/header/environment, response, streaming, collection runner | R01–R08 / UX-102–104/106 |

## Intentional corrections and demonstrated behavior

`TextBuffer.loadText` and `applyFullText` normalize CRLF and append an additional
empty line after splitting text already ending with LF. Rust preserves exact
newline/BOM metadata and never introduces a newline on an unchanged load/save
(E01/E04, UX-032). The baseline bug is not an expected Rust result.

`CodeEditorView.VisualLine` displays each tab as exactly `|-->`, independent of
its column. `tabsRenderAtFourColumnStopsWithoutShiftingTokens` actually asserts
those fixed markers. Buffer tab helpers separately use column-aligned spaces;
the real editor rendering contract follows `VisualLine`. Control scalars render
as `\\xNN`; wide/supplementary/combining scalars use hexadecimal escapes padded to
at least four digits. Raw document content remains unchanged. UTF-16 surrogate
interior positions clamp to the preceding valid scalar boundary in Rust.

Read failures must remain visible, virtual saves must route to their host, stale
dirty sessions retain recovery content, and all read-only mutations must be
guarded at the document boundary. These strengthen source input-only guards and
permissive load behavior (UX-023/025/028/031/103).

Existing rename APIs return default `false`; diagnostics/document-symbol
interfaces with empty defaults do not establish complete workflows. SDK indexing
and learned/ONNX ranking are future interfaces, while the active heuristic
ranker, Kotlin/C semantic resolution, Tree-sitter lexical highlighting, native
JGit workflows, JDBC and REST behavior are implemented and must be migrated.
Preserve byte-view operations that are actually dispatched; a full arbitrary
hex-digit editing/saving workflow is not established by the source.

## Regeneration

`python3 scripts/baseline.py inventory` validates source/test allocation and hashes.
`python3 scripts/baseline.py fixtures` regenerates only dedicated migration
fixtures. `python3 scripts/baseline.py results` captures the last Kotlin test XML.
The results capture requires a freshly completed baseline run; timestamps remain
in each suite so old evidence cannot masquerade as a new execution.
