# Task index

67 implementation tasks. Status is recorded in each task definition and synchronized with [the dashboard](../../DASHBOARD.md).

Pick a task only when all its listed prerequisites have implementation evidence. The card owns its outputs; shared contracts are in [CONTRACTS.md](CONTRACTS.md). Source and test ownership are in [SOURCE-MAP.tsv](SOURCE-MAP.tsv) and [TEST-MAP.tsv](TEST-MAP.tsv).

First task: **F01**. Then F02 establishes contracts, and F03 establishes runtime boundaries. After those, use dependency readiness; do not infer readiness from a milestone number.

| ID | Task card | Prerequisites | Status |
|---|---|---|---|
| F01 | [Freeze behavior and fixtures](tasks/01-foundation.md) | — | [done](../parity/F01.md) |
| F02 | [Cargo workspace and shared types](tasks/01-foundation.md) | F01 | [done](../parity/F02.md) |
| F03 | [Jobs, processes, paths, logging and shutdown](tasks/01-foundation.md) | F02 | [done](../parity/F03.md) |
| F04 | [Terminal lifecycle and normalized input](tasks/01-foundation.md) | F02, F03 | [done](../parity/F04.md) |
| F05 | [Common widgets, style mapping, layout and focus](tasks/01-foundation.md) | F04 | [done](../parity/F05.md) |
| E01 | [Rope document and coordinate conversions](tasks/02-editor.md) | F02 | [done](../parity/E01.md) |
| E02 | [Movement and selection commands](tasks/02-editor.md) | E01 | [done](../parity/E02.md) |
| E03 | [Editing, auto-indent, undo and redo](tasks/02-editor.md) | E02 | [done](../parity/E03.md) |
| E04 | [File I/O, save state and clipboard](tasks/02-editor.md) | E03, F03 | [done](../parity/E04.md) |
| E05 | [Visual layout and editor cell rendering](tasks/02-editor.md) | E02, F05 | [done](../parity/E05.md) |
| E06 | [Editor input, hit testing, scrolling and focus](tasks/02-editor.md) | E03, E05, F04 | [done](../parity/E06.md) |
| E07 | [Regex find/replace and search bar](tasks/02-editor.md) | E03, E05, E06, F05 | [done](../parity/E07.md) |
| E08 | [Folding and Braille minimap](tasks/02-editor.md) | E05, E06 | [done](../parity/E08.md) |
| E09 | [Completion popup and insertion](tasks/02-editor.md) | E03, E05, E06 | [done](../parity/E09.md) |
| E10 | [Definitions, usages and hover preview](tasks/02-editor.md) | E05, E06, F03 | [done](../parity/E10.md) |
| E11 | [Highlight overlays and edit remapping](tasks/02-editor.md) | E05, E07, L02, L09 | [done](../parity/E11.md) |
| E12 | [Reusable editor hosts and virtual/read-only documents](tasks/02-editor.md) | E04, E07, E08, F05 | [done](../parity/E12.md) |
| E13 | [Complete editor interaction qualification](tasks/02-editor.md) | E09, E10, E11, E12, W04, W05, P03, D08, R07, G03 | todo |
| W01 | [Source roots, exclusions and project scanning](tasks/03-workspace.md) | F03 | [done](../parity/W01.md) |
| W02 | [File tree, file actions, refresh and project panel](tasks/03-workspace.md) | W01, F05, E04 | [done](../parity/W02.md) |
| W03 | [Legacy session import and Rust persistence](tasks/03-workspace.md) | E03, F03 | [done](../parity/W03.md) |
| W04 | [Project search modal and embedded editor](tasks/03-workspace.md) | W01, E07, E12, F05, V01 | [done](../parity/W04.md) |
| W05 | [CLI targets and workspace application shell](tasks/03-workspace.md) | F04, F05, W02, W03, E12, V01 | [done](../parity/W05.md) |
| V01 | [MIME/language detection and routing](tasks/04-viewers.md) | F02, F03 | [done](../parity/V01.md) |
| V02 | [Byte buffer and hex viewer](tasks/04-viewers.md) | F05, V01, E04 | [done](../parity/V02.md) |
| V03 | [Image decoding and colored cell modes](tasks/04-viewers.md) | F03, F05, V01 | [done](../parity/V03.md) |
| G01 | [Repository discovery and read operations](tasks/05-git.md) | F03 | [done](../parity/G01.md) |
| G02 | [Stage, unstage, commit, checkout and tags](tasks/05-git.md) | G01, E12, F05 | [done](../parity/G02.md) |
| G03 | [Diff views and restoring worktree chunks](tasks/05-git.md) | G02, E05, E12, V01 | [done](../parity/G03.md) |
| L01 | [Semantic facts, identifiers and immutable snapshots](tasks/06-intelligence.md) | F02, E01 | [done](../parity/L01.md) |
| L02 | [Keyword highlighting and incremental parser registry](tasks/06-intelligence.md) | E01, F03 | [done](../parity/L02.md) |
| L03 | [Kotlin semantic extraction](tasks/06-intelligence.md) | L01, L02 | [done](../parity/L03.md) |
| L04 | [C semantic extraction](tasks/06-intelligence.md) | L01, L02 | [done](../parity/L04.md) |
| L05 | [SQLite semantic store and atomic publication](tasks/06-intelligence.md) | L01, F03 | [done](../parity/L05.md) |
| L06 | [Names, scopes, imports and symbol resolution](tasks/06-intelligence.md) | L03, L04 | [done](../parity/L06.md) |
| L07 | [Expression types and receiver member lookup](tasks/06-intelligence.md) | L06 | [done](../parity/L07.md) |
| L08 | [Dependency invalidation and incremental generations](tasks/06-intelligence.md) | L07 | [done](../parity/L08.md) |
| L09 | [Background index coordinator and editor facade](tasks/06-intelligence.md) | L05, L08, L10, L11, W01, F03 | [done](../parity/L09.md) |
| L10 | [Other language adapters and heuristic fallback](tasks/06-intelligence.md) | L01, L02, L06 | [done](../parity/L10.md) |
| L11 | [Completion providers and deterministic ranking](tasks/06-intelligence.md) | L07, L02 | [done](../parity/L11.md) |
| L12 | [Language and persistence parity qualification](tasks/06-intelligence.md) | L09, L10 | [done](../parity/L12.md) |
| P01 | [Server catalog, discovery and installation](tasks/07-lsp.md) | F03, F05 | [done](../parity/P01.md) |
| P02 | [LSP client transport and lifecycle](tasks/07-lsp.md) | P01, E01, F03 | [done](../parity/P02.md) |
| P03 | [Editor enrichment and provider fallback](tasks/07-lsp.md) | P02, L09, E09, E10 | [done](../parity/P03.md) |
| R01 | [Lossless collection and environment model](tasks/08-rest.md) | F02 | [done](../parity/R01.md) |
| R02 | [Variables, auth, headers, URLs and request bodies](tasks/08-rest.md) | R01, F03 | [done](../parity/R02.md) |
| R03 | [Postman and OpenAPI import/export](tasks/08-rest.md) | R01 | [done](../parity/R03.md) |
| R04 | [HTTP execution, cookies and cancellation](tasks/08-rest.md) | R02, F03 | [done](../parity/R04.md) |
| R05 | [SSE and WebSocket execution](tasks/08-rest.md) | R04 | [done](../parity/R05.md) |
| R06 | [REST tree, environments and management dialogs](tasks/08-rest.md) | R01, R03, R05, F05, E12, W03 | [done](../parity/R06.md) |
| R07 | [Request/response editors and complete REST UX](tasks/08-rest.md) | R02, R04, R05, R06, E09, E12, L02 | [done](../parity/R07.md) |
| R08 | [Collection runner, persistence and REST qualification](tasks/08-rest.md) | R07, W03, W05 | [done](../parity/R08.md) |
| R09 | [Extended authentication protocols](tasks/08-rest.md) | R08 | [done](../parity/R09.md) |
| R10 | [OAuth acquisition and browser authorization](tasks/08-rest.md) | R09 | in-progress |
| D01 | [Database models, settings, credentials and URL catalogs](tasks/09-database.md) | F02, F03 | [done](../parity/D01.md) |
| D02 | [Protocol, Java bootstrap and Rust process supervisor](tasks/09-database.md) | D01, F03 | [done](../parity/D02.md) |
| D03 | [Driver install, Maven resolution and classloaders](tasks/09-database.md) | D02, F05 | [done](../parity/D03.md) |
| D04 | [Connections, testing and transaction control](tasks/09-database.md) | D03 | [done](../parity/D04.md) |
| D05 | [SQL execution, results and independent cancellation](tasks/09-database.md) | D04 | [done](../parity/D05.md) |
| D06 | [Lazy metadata and cache invalidation](tasks/09-database.md) | D04 | [done](../parity/D06.md) |
| D07 | [Database explorer, connection and driver dialogs](tasks/09-database.md) | D01, D03, D04, D06, F05, W03 | [done](../parity/D07.md) |
| D08 | [SQL editor, statement targeting and result panel](tasks/09-database.md) | D05, D07, E12, E09, L02 | [done](../parity/D08.md) |
| D09 | [End-to-end external JDBC qualification](tasks/09-database.md) | D08, W03, W05 | [done](../parity/D09.md) |
| Z01 | [Shell suspension and browser terminal serve mode](tasks/10-delivery.md) | W05, F04, F03 | [done](../parity/Z01.md) |
| Z02 | [Settings, help, about and operational status](tasks/10-delivery.md) | W05, L09, P01, D07, F05 | [done](../parity/Z02.md) |
| Z03 | [Reproducible build and distribution](tasks/10-delivery.md) | Z01, Z02, D02 | todo |
| Z04 | [Full functional parity and release qualification](tasks/10-delivery.md) | E13, L12, W05, V02, V03, G03, D09, R08, R10, Z03 | todo |

## Execution checkpoints

- Establish a usable editor shell first: F01–F05, E01–E08, E12, W01–W03, W05, V01.
- Build native intelligence independently once core contracts exist: L01–L12; attach E09–E11 and P01–P03.
- Complete workspace search, viewers and Git: W04, V02–V03, G01–G03.
- Complete both integration modules: D01–D09 and R01–R08.
- Qualify the shared editor after all real hosts are attached: E13.
- Finish shell/browser mode, settings, packages and full parity: Z01–Z04.

Independent tasks can be assigned separately after their contracts are ready. This is a dependency plan, not an instruction to spawn agents automatically.

## Completion record

When implementation begins, update `todo` to `in-progress`, `done`, or `blocked`, and link the evidence report. Record blockers explicitly. Reconcile task prerequisites whenever a shared contract changes. See [HANDOFF.md](HANDOFF.md) for the report template.
