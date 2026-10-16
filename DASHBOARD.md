# Implementation dashboard

Full Kode → Rust migration, including the [finalized UAT batch](docs/migration/UAT.md). Counts are derived from the status in every task definition.

| To do | In progress | Done | Blocked | Total |
|------:|------------:|-----:|--------:|------:|
|     7 |           1 |   71 |       0 |    79 |

## Ongoing tasks

- **U04** — [Multiple cursors and selections](docs/migration/tasks/11-uat.md#u04--multiple-cursors-and-selections)

## Task status

| Task | Definition                                                                                      | Status      | Evidence                     |
|------|-------------------------------------------------------------------------------------------------|-------------|------------------------------|
| F01  | [Freeze behavior and fixtures](docs/migration/tasks/01-foundation.md)                           | done        | [Report](docs/parity/F01.md) |
| F02  | [Cargo workspace and shared types](docs/migration/tasks/01-foundation.md)                       | done        | [Report](docs/parity/F02.md) |
| F03  | [Jobs, processes, paths, logging and shutdown](docs/migration/tasks/01-foundation.md)           | done        | [Report](docs/parity/F03.md) |
| F04  | [Terminal lifecycle and normalized input](docs/migration/tasks/01-foundation.md)                | done        | [Report](docs/parity/F04.md) |
| F05  | [Common widgets, style mapping, layout and focus](docs/migration/tasks/01-foundation.md)        | done        | [Report](docs/parity/F05.md) |
| E01  | [Rope document and coordinate conversions](docs/migration/tasks/02-editor.md)                   | done        | [Report](docs/parity/E01.md) |
| E02  | [Movement and selection commands](docs/migration/tasks/02-editor.md)                            | done        | [Report](docs/parity/E02.md) |
| E03  | [Editing, auto-indent, undo and redo](docs/migration/tasks/02-editor.md)                        | done        | [Report](docs/parity/E03.md) |
| E04  | [File I/O, save state and clipboard](docs/migration/tasks/02-editor.md)                         | done        | [Report](docs/parity/E04.md) |
| E05  | [Visual layout and editor cell rendering](docs/migration/tasks/02-editor.md)                    | done        | [Report](docs/parity/E05.md) |
| E06  | [Editor input, hit testing, scrolling and focus](docs/migration/tasks/02-editor.md)             | done        | [Report](docs/parity/E06.md) |
| E07  | [Regex find/replace and search bar](docs/migration/tasks/02-editor.md)                          | done        | [Report](docs/parity/E07.md) |
| E08  | [Folding and Braille minimap](docs/migration/tasks/02-editor.md)                                | done        | [Report](docs/parity/E08.md) |
| E09  | [Completion popup and insertion](docs/migration/tasks/02-editor.md)                             | done        | [Report](docs/parity/E09.md) |
| E10  | [Definitions, usages and hover preview](docs/migration/tasks/02-editor.md)                      | done        | [Report](docs/parity/E10.md) |
| E11  | [Highlight overlays and edit remapping](docs/migration/tasks/02-editor.md)                      | done        | [Report](docs/parity/E11.md) |
| E12  | [Reusable editor hosts and virtual/read-only documents](docs/migration/tasks/02-editor.md)      | done        | [Report](docs/parity/E12.md) |
| E13  | [Complete editor interaction qualification](docs/migration/tasks/02-editor.md)                  | todo        | —                            |
| W01  | [Source roots, exclusions and project scanning](docs/migration/tasks/03-workspace.md)           | done        | [Report](docs/parity/W01.md) |
| W02  | [File tree, file actions, refresh and project panel](docs/migration/tasks/03-workspace.md)      | done        | [Report](docs/parity/W02.md) |
| W03  | [Legacy session import and Rust persistence](docs/migration/tasks/03-workspace.md)              | done        | [Report](docs/parity/W03.md) |
| W04  | [Project search modal and embedded editor](docs/migration/tasks/03-workspace.md)                | done        | [Report](docs/parity/W04.md) |
| W05  | [CLI targets and workspace application shell](docs/migration/tasks/03-workspace.md)             | done        | [Report](docs/parity/W05.md) |
| V01  | [MIME/language detection and routing](docs/migration/tasks/04-viewers.md)                       | done        | [Report](docs/parity/V01.md) |
| V02  | [Byte buffer and hex viewer](docs/migration/tasks/04-viewers.md)                                | done        | [Report](docs/parity/V02.md) |
| V03  | [Image decoding and colored cell modes](docs/migration/tasks/04-viewers.md)                     | done        | [Report](docs/parity/V03.md) |
| G01  | [Repository discovery and read operations](docs/migration/tasks/05-git.md)                      | done        | [Report](docs/parity/G01.md) |
| G02  | [Stage, unstage, commit, checkout and tags](docs/migration/tasks/05-git.md)                     | done        | [Report](docs/parity/G02.md) |
| G03  | [Diff views and restoring worktree chunks](docs/migration/tasks/05-git.md)                      | done        | [Report](docs/parity/G03.md) |
| L01  | [Semantic facts, identifiers and immutable snapshots](docs/migration/tasks/06-intelligence.md)  | done        | [Report](docs/parity/L01.md) |
| L02  | [Keyword highlighting and incremental parser registry](docs/migration/tasks/06-intelligence.md) | done        | [Report](docs/parity/L02.md) |
| L03  | [Kotlin semantic extraction](docs/migration/tasks/06-intelligence.md)                           | done        | [Report](docs/parity/L03.md) |
| L04  | [C semantic extraction](docs/migration/tasks/06-intelligence.md)                                | done        | [Report](docs/parity/L04.md) |
| L05  | [SQLite semantic store and atomic publication](docs/migration/tasks/06-intelligence.md)         | done        | [Report](docs/parity/L05.md) |
| L06  | [Names, scopes, imports and symbol resolution](docs/migration/tasks/06-intelligence.md)         | done        | [Report](docs/parity/L06.md) |
| L07  | [Expression types and receiver member lookup](docs/migration/tasks/06-intelligence.md)          | done        | [Report](docs/parity/L07.md) |
| L08  | [Dependency invalidation and incremental generations](docs/migration/tasks/06-intelligence.md)  | done        | [Report](docs/parity/L08.md) |
| L09  | [Background index coordinator and editor facade](docs/migration/tasks/06-intelligence.md)       | done        | [Report](docs/parity/L09.md) |
| L10  | [Other language adapters and heuristic fallback](docs/migration/tasks/06-intelligence.md)       | done        | [Report](docs/parity/L10.md) |
| L11  | [Completion providers and deterministic ranking](docs/migration/tasks/06-intelligence.md)       | done        | [Report](docs/parity/L11.md) |
| L12  | [Language and persistence parity qualification](docs/migration/tasks/06-intelligence.md)        | done        | [Report](docs/parity/L12.md) |
| P01  | [Server catalog, discovery and installation](docs/migration/tasks/07-lsp.md)                    | done        | [Report](docs/parity/P01.md) |
| P02  | [LSP client transport and lifecycle](docs/migration/tasks/07-lsp.md)                            | done        | [Report](docs/parity/P02.md) |
| P03  | [Editor enrichment and provider fallback](docs/migration/tasks/07-lsp.md)                       | done        | [Report](docs/parity/P03.md) |
| R01  | [Lossless collection and environment model](docs/migration/tasks/08-rest.md)                    | done        | [Report](docs/parity/R01.md) |
| R02  | [Variables, auth, headers, URLs and request bodies](docs/migration/tasks/08-rest.md)            | done        | [Report](docs/parity/R02.md) |
| R03  | [Postman and OpenAPI import/export](docs/migration/tasks/08-rest.md)                            | done        | [Report](docs/parity/R03.md) |
| R04  | [HTTP execution, cookies and cancellation](docs/migration/tasks/08-rest.md)                     | done        | [Report](docs/parity/R04.md) |
| R05  | [SSE and WebSocket execution](docs/migration/tasks/08-rest.md)                                  | done        | [Report](docs/parity/R05.md) |
| R06  | [REST tree, environments and management dialogs](docs/migration/tasks/08-rest.md)               | done        | [Report](docs/parity/R06.md) |
| R07  | [Request/response editors and complete REST UX](docs/migration/tasks/08-rest.md)                | done        | [Report](docs/parity/R07.md) |
| R08  | [Collection runner, persistence and REST qualification](docs/migration/tasks/08-rest.md)        | done        | [Report](docs/parity/R08.md) |
| R09  | [Extended authentication protocols](docs/migration/tasks/08-rest.md)                            | done        | [Report](docs/parity/R09.md) |
| R10  | [OAuth acquisition and browser authorization](docs/migration/tasks/08-rest.md)                  | done        | [Report](docs/parity/R10.md) |
| D01  | [Database models, settings, credentials and URL catalogs](docs/migration/tasks/09-database.md)  | done        | [Report](docs/parity/D01.md) |
| D02  | [Protocol, Java bootstrap and Rust process supervisor](docs/migration/tasks/09-database.md)     | done        | [Report](docs/parity/D02.md) |
| D03  | [Driver install, Maven resolution and classloaders](docs/migration/tasks/09-database.md)        | done        | [Report](docs/parity/D03.md) |
| D04  | [Connections, testing and transaction control](docs/migration/tasks/09-database.md)             | done        | [Report](docs/parity/D04.md) |
| D05  | [SQL execution, results and independent cancellation](docs/migration/tasks/09-database.md)      | done        | [Report](docs/parity/D05.md) |
| D06  | [Lazy metadata and cache invalidation](docs/migration/tasks/09-database.md)                     | done        | [Report](docs/parity/D06.md) |
| D07  | [Database explorer, connection and driver dialogs](docs/migration/tasks/09-database.md)         | done        | [Report](docs/parity/D07.md) |
| D08  | [SQL editor, statement targeting and result panel](docs/migration/tasks/09-database.md)         | done        | [Report](docs/parity/D08.md) |
| D09  | [End-to-end external JDBC qualification](docs/migration/tasks/09-database.md)                   | done        | [Report](docs/parity/D09.md) |
| Z01  | [Shell suspension and browser terminal serve mode](docs/migration/tasks/10-delivery.md)         | done        | [Report](docs/parity/Z01.md) |
| Z02  | [Settings, help, about and operational status](docs/migration/tasks/10-delivery.md)             | done        | [Report](docs/parity/Z02.md) |
| Z03  | [Reproducible build and distribution](docs/migration/tasks/10-delivery.md)                      | todo        | [Report](docs/parity/Z03.md) |
| Z04  | [Full functional parity and release qualification](docs/migration/tasks/10-delivery.md)         | todo        | —                            |
| U01  | [Finalized file and project search shortcuts](docs/migration/tasks/11-uat.md)                   | done        | [Report](docs/parity/U01.md) |
| U02  | [Readable database errors and diagnostics](docs/migration/tasks/11-uat.md)                      | done        | [Report](docs/parity/U02.md) |
| U03  | [Optional downloadable Java runtime artifact](docs/migration/tasks/11-uat.md)                   | done        | [Report](docs/parity/U03.md) |
| U04  | [Multiple cursors and selections](docs/migration/tasks/11-uat.md)                               | in-progress | —                            |
| U05  | [Hex layouts and text or binary find](docs/migration/tasks/11-uat.md)                           | done        | [Report](docs/parity/U05.md) |
| U06  | [Word selection with double-click and Alt+double-click](docs/migration/tasks/11-uat.md)         | todo        | —                            |
| U07  | [File-tree picker buttons for path fields](docs/migration/tasks/11-uat.md)                      | todo        | —                            |
| U08  | [Project-relative paths in the open-file list](docs/migration/tasks/11-uat.md)                  | done        | [Report](docs/parity/U08.md) |
| U09  | [Repair language server installations](docs/migration/tasks/11-uat.md)                          | todo        | —                            |
| U10  | [Panel shortcuts follow visual tab order](docs/migration/tasks/11-uat.md)                       | done        | [Report](docs/parity/U10.md) |
| U11  | [Connection-type JDBC URL template dropdown](docs/migration/tasks/11-uat.md)                    | done        | [Report](docs/parity/U11.md) |
| U12  | [Database package listing and DDL](docs/migration/tasks/11-uat.md)                              | todo        | —                            |

Regenerate with `python3 scripts/tasks.py`; verify with `python3 scripts/tasks.py --check`.

Change a task with `python3 scripts/tasks.py F01 in-progress`. Prerequisites must be done before starting or completing dependent work.
