# Data compatibility and acceptance plan

## User-data boundaries

| Existing data/resource | Rust behavior | Owner |
|---|---|---|
| `.kode.json`, `ProjectSession.version = 6` | Read/import into `.kode-rs.json`; retain original; current/new schema versioned separately | W03 |
| Open editors and recents | Preserve paths/viewer kind, timestamps, language/grammar, cursor/anchor/scroll, dirty text and undo/redo | E01/W03 |
| Legacy UTF-16 positions | Convert against the correct snapshot's text; never reinterpret as UTF-8 indexes | E01/W03 |
| Source roots/exclusions | Import, preserve relative path semantics and conventional default roots | W01/W03 |
| `.kode/db/kode.mv.db` internal H2 | Leave untouched; rebuild native SQLite cache in `.kode-rs/index.sqlite` | L05 |
| User H2 or other JDBC database | Keep JDBC access through external JVM; never “migrate” user database contents to SQLite | D01–D09 |
| User `database/datasources.json` | Preserve identifiers, URLs, class/coordinates/artifact/provider, properties and credential references | D01 |
| `credentials.properties` | Explicit compatible import including Java escaping; secrets stored separately with restricted access | D01 |
| JDBC installed JARs | Reuse configured classpaths or copy through installation service; preserve multi-JAR and aliases | D03 |
| JDBC URL template catalog/recents | Every entry retained, family-scoped exact MRU capped at 12, secrets excluded | D01 |
| SQL saved sessions | Preserve datasource association, title, catalog/schema, autocommit and buffer; runtime connections reconnect explicitly | D08/W03 |
| Postman v2.1 JSON | Preserve unknown fields, scripts, examples, order where material, disabled fields and `x-kode-headers` | R01/R03 |
| REST environments/UI | Escaped values, flags/metadata, active environment, selected node, pretty-print and divider heights | R01/R06–R08 |
| Runtime HTTP responses/cookies | In memory, not accidentally serialized into the persisted/exported collection | R04/R08 |
| `keyword-patterns.txt`, `token-colors.txt` | Preserve editable catalog semantics and keyword aliases; validate regex compatibility | L02 |
| `styles/app.css` | Map supported current properties/style IDs; ship compiled defaults and external override loading | F05 |
| `codeintel/definitions.json` | Preserve external-over-default precedence and heuristic fallback | L10 |
| `lsp/servers.json`, installed manifests | Compatible discovery/import, platform aliases and command arrays | P01 |
| Native ttyd resources | Equivalent packaged/installed executable wrapper and CLI behavior | Z01/Z03 |

Import must be idempotent and explicit about source/target files. Do not automatically
rewrite a Kotlin session into a new incompatible format. Do not read real credentials
or private sessions into fixtures. Use synthetic values and dedicated temporary homes.

## Important differences to resolve through evidence

- The Kotlin buffer normalizes CRLF and appears to append an extra empty line in
  some load paths. Exact byte-preserving save is the intended Rust behavior; add
  fixtures before deciding that an accidental extra newline is required parity.
- The cell renderer explicitly escapes wide/non-BMP/combining scalars and displays
  a tab as `|-->`. Preserve that presentation by default. Unicode storage correctness
  must improve without silently changing established display/navigation behavior.
- Legacy stale-session handling can discard entries. Preserve mtime protection
  while retaining a recovery copy of dirty imported text.
- Some read-only/virtual edit paths in Kotlin are guarded at input dispatch only.
  Rust must enforce mutation capability centrally and route virtual save to its host.
- Source README/specs mention diagnostics, rename, TextMate, learned rankers and
  SDK indexing more broadly than active implementations. F01/L12/P03 record which
  behaviors exist. `indexSdkSources()` is currently disabled. Heuristic completion
  ranking is implemented and must be ported.
- The byte view exposes selected mutations, but a complete direct hex-digit input/
  save workflow is not established by its current dispatch. Preserve actual
  behavior and label any expansion as an addition.
- Browser/terminal key combinations may be indistinguishable without enhanced
  keyboard support. Retain supported aliases and mouse alternatives, document
  capability limitations, and test the target terminals.

## Regression ownership

The detailed [TEST-MAP.tsv](TEST-MAP.tsv) assigns every source test file. Main suites:

| Tests | Migration owner / required behavior |
|---|---|
| TextBufferInput/Undo/Search | E01–E07: input aliases, edit history, captures/search/save |
| CodeEditorView, SearchReplaceBar, HighlightEdits | E05–E13: wrapping, folding, cells, popup capture, edit remapping |
| AnsiCanvasRenderer, TabView | F04/F05: decoded input/overflow/focus |
| ProjectFileScanner, ProjectFolderStatus, FileTree | W01/W02: ignore/root/exclusion rules and hidden metadata |
| ProjectSessionManager | W03: defaults and database/REST session persistence |
| DefaultMimeTypeDetector, KorimAsciiImageRenderer | V01/V03: exact routing/format and cell behavior |
| JGitService, GitPanelView | G01–G03: large blobs, errors, repository workflows |
| KotlinExpressionSemantics, KotlinSemanticVerticalSlice | L03/L06/L07/L11: type/member/scope/identity correctness |
| CSemantic | L04/L06/L07: tag/typedef/pointer/cross-file resolution |
| H2SemanticStore, SemanticInvalidation, DbServerManager | L05/L08: persistence, dependency invalidation and native cache lifecycle |
| SemanticHighlighting, CodeIntelService | L02/L09/E11: source spans, revisions, fallbacks, repaint |
| TreeSitterSemanticAdapter, Kotlin/TypeScript identifier suites | L02/L03/L10/L12: active parser/adapter behavior |
| JDBC registry/URL/config/metadata/SQL tests | D01–D09: full driver catalog, secrets, recents, metadata, SQL transaction semantics |
| REST model/resolver/HTTP/format/editor tests | R01–R08/E12: lossless data, HTTP/streams, focus and field editing |
| TtydAdapter | Z01/Z03: platform/resource/serve behavior |

Port meaningful fixture assertions, not Kotlin-specific renderer object layouts or
developer-corpus absolute paths. Add missing interaction/protocol tests at their
owning tasks. No test execution was needed for these documentation-only changes;
the above commands and suites are future implementation acceptance work.

## Acceptance fixture sets

1. **Editor:** empty/short/large/very long line, forward/reverse multiline selection,
   tabs, controls, emoji, combining/wide scalars, CRLF/BOM/no final newline, dirty
   history, regex error/captures, folds, completion and multiple navigation targets.
2. **Workspace:** nested Git ignores/negation, hidden metadata, exclusions, symlinks,
   unreadable directories, roots outside project as supported, renamed/open files,
   workspace switch during background scan and dirty session restore.
3. **Git:** temporary repo with unborn HEAD, commits/tags/branches, staged and
   unstaged changes, deletions/renames/conflicts, large/binary files and hunk restore.
4. **Languages:** minimal per-language corpus plus multi-file Kotlin/C projects
   exercising all source semantic tests; fresh parse/edit/reload/invalidation variants.
5. **JDBC:** H2 helper integration by default, protocol fake driver with delays and
   multiple results, optional local PostgreSQL/MySQL; all catalog entries checked
   structurally even when external vendor systems are unavailable.
6. **REST:** nested Postman document with unknown fields, JSON/YAML OpenAPI versions,
   all body/auth/variable modes, loopback HTTP/SSE/WS servers, duplicate headers,
   cookies, slow/large/truncated/error/cancel responses and sequential runs.
7. **Viewers/resources:** source MIME samples, each supported image decoder and
   colored-cell mode, byte-boundary/clipboard tests and external-resource overrides.

## Terminal and platform matrix

| Environment | Required checks |
|---|---|
| Linux local terminal | All features, raw-mode recovery, mouse/resize/paste, shell suspend/resume |
| SSH session | Clipboard fallback/paste, mouse/modifiers, external Java on remote host, no local-path assumptions |
| tmux | Key aliases, mouse capture, resize, selection and clipboard behavior |
| ttyd browser | File editing, splitters/popups/embedded editors, reserved shortcuts and credentials/default bind |
| Windows terminal | Crossterm event kinds, path/URL conversions, shell, Java discovery, clipboard and child lifecycle |
| macOS terminal | Clipboard, modifiers, process lifecycle, resources and architecture-specific packages |

Do not call a platform supported based only on a target build. The frozen source
contains ttyd shared libraries for Linux, macOS and Windows on x86_64 and aarch64
(verified with `git ls-tree` against the frozen baseline during Z01). Their presence
is evidence of bundled assets, not runtime qualification of the Rust executable
replacement on those platforms.
Make unsupported packaging combinations explicit while retaining the implementation
tasks needed for the intended release targets.

## Responsiveness and cleanup gate

Z04 records machine/toolchain and p50/p95 key-to-frame timings on a fixed fixture
set (1 MiB file, 100,000 short lines, a 100,000-character line, and concurrent index/
Git/REST/JDBC activity). Initial target: ordinary key-to-frame p95 below 50 ms on
the reference machine; measure baseline and document justified hardware-dependent
adjustments. This is a target, not a measured result of planning.

Large work must run outside the UI thread. Queues, previews, pending requests and
decoded images have explicit limits. Exit, workspace switch and helper crash leave
no owned child processes, terminal raw mode, stale locks or half-published state.
No test can pass by dropping user edits or clearing an error without surfacing it.

## Definition of complete migration

All implemented source features and all 110 editor UX rows have reproducible
evidence; source/test allocation has no unowned entries; resources/catalogs and
legacy data import are covered; release artifacts work away from the development
directory. Documented source proposals may remain future work. An implemented
feature may be deferred only as an explicit scope decision, never through an
unnoticed crate limitation or a placeholder implementation.
