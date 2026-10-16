# JDBC database tasks

Read [JDBC.md](../JDBC.md) before implementing any card. The user chose an external
JVM. Preserve all existing JDBC driver advantages; native per-engine Rust drivers
are not a required replacement. Paths under `jdbc-bridge/` are Java; all IDE UI and
client models remain Rust.

## D01 — Database models, settings, credentials and URL catalogs

**Status:** done

**Depends:** F02, F03. **Owns:** `kode-database/src/{model,config,credentials,catalog}.rs`,
`resources/database/`.
**Read:** DatabaseModels.kt, config/*.kt, JdbcUrlTemplates.kt, registry definitions
in JdbcDrivers.kt; DataSourceRepositoryTest/JdbcUrlTemplatesTest/JdbcDriverRegistryTest.

1. Port datasource/driver/artifact/session/metadata/result models and every registry
   entry/alias. Preserve driver classes, coordinates, custom repositories, vendor
   page/download URLs, local JAR lists and provider-family mappings.
2. Import legacy user configuration with credential references and Java properties
   escaping. Store Rust config separately; keep secrets out of project JSON and logs.
3. Port standard JDBC URL templates, family aliases, validation, exact MRU order/
   dedup/cap=12, safe-parameter handling and credential-bearing URL rejection.

**Accept:** all source config/catalog tests translated, catalog entry count/names
   match baseline, aliases share provider artifacts/recents, import doesn't change
   URLs/classes, credentials never appear in exported datasource/project state.

## D02 — Protocol, Java bootstrap and Rust process supervisor

**Status:** done

**Depends:** D01, F03. **Owns:** `protocol/jdbc-v1/`, `kode-database/src/bridge/`,
`jdbc-bridge/pom.xml`, Java `protocol/` and `Main`.
**Read:** JDBC.md; original DatabaseService boundary and process/resource helpers.

1. Formalize wire schemas and fixtures; implement fragmented length-prefix frames,
   hello/version/capabilities, request IDs, errors, notifications and size bounds in
   both languages. Use a pinned Java JSON library instead of hand-parsing JSON.
2. Build Java 21 helper with protocol stdout and redirected driver System.out;
   independent reader/control/worker/writer roles. Runtime helper needs no Maven CLI.
3. Implement Java discovery, startup timeout, lazy reuse, stderr draining, request
   cancellation routing, crash epoch, graceful shutdown/kill/reap and missing-Java UI
   state. Use a fake helper for Rust failure tests.

**Accept:** all protocol cases in JDBC.md; no JVM inside Rust, no Java requirement
   at editor startup, poisoned output/oversized frames fail cleanly. Pipe reader
   can accept cancellation while a fake database operation blocks.

## D03 — Driver install, Maven resolution and classloaders

**Status:** done

**Depends:** D02, F05. **Owns:** Java `drivers/`, Rust bridge driver client and install
manifests; `kode-tui/src/panels/database/driver_install.rs`.
**Read:** JdbcDrivers.kt resolver/downloader/loader; JdbcDriverRegistryTest.

1. Use Apache Maven Resolver with pinned Java dependencies to resolve runtime
   graph/classifiers/custom repos; preserve selected version and alias/provider
   install family. Atomic install manifests include artifact paths/digests.
2. Implement vendor download/local multi-JAR sources, cancellation/progress, reuse,
   failed-partial cleanup and manual vendor page-only behavior. Keep catalog source
   overrides and user plugin paths working.
3. Implement classloader isolation, explicit Driver class/ServiceLoader discovery,
   Driver.connect dispatch and loader lifetime. Explain restart requirements for
   native-driver changes; never delete an in-use classpath.

**Accept:** H2 load/query, a driver with transitive dependencies, shared alias
   artifact, custom repository, local multiple JARs, cancellation and failed install.
   Every registry entry remains selectable; no invented vendor artifact URL.

## D04 — Connections, testing and transaction control

**Status:** done

**Depends:** D03. **Owns:** Java `connections/`,
`kode-database/src/connections.rs`.
**Read:** DatabaseConnectionManager.kt, DatabaseService.kt, connection model types.

1. Implement test/open/close with ephemeral credential injection, driver properties,
   catalog/schema/read-only/autocommit and normalized connection status/errors.
2. Maintain serialized connection workers and datasource-level shared transaction
   state. Implement commit/rollback/autocommit changes with explicit driver failures.
3. Handle changed datasource definitions, disconnect while busy, helper failure
   and stale IDs. Reconnect is explicit; never replay a possibly executed statement.

**Accept:** H2 connection test/product/version/latency, authentication/URL failures,
   update+rollback, autocommit, shared-console transaction semantics and process
   crash with pending operation. No false successful rollback after helper death.

## D05 — SQL execution, results and independent cancellation

**Status:** done

**Depends:** D04. **Owns:** Java `queries/`, `kode-database/src/execution.rs`.
**Read:** SqlQuery.kt execution/conversion/warning code; JdbcDatabaseIntegrationTest.

1. Execute JDBC statements and iterate mixed results/update counts with getMoreResults.
   Convert nullable cells/type labels, binary/decimal/time values, warnings and chained
   SQL errors to bounded ordered protocol events.
2. Stream batches with row/cell/frame limits from JDBC.md; track query/result-set IDs,
   sequence, elapsed time, partial results and final outcome.
3. Register active statements by execution ID and process cancellation on independent
   control executor. Handle already-finished/unsupported/hanging cancellation without
   targeting the next query or blocking input/control frames.

**Accept:** select/update/multiple-result/error/NULL/binary/truncation fixtures;
   long-running query cancelled while reader is busy, saturated output and helper
   crash. Completion exactly once; cancelled does not imply a committed/rolled-back
   transaction. Source SQL result semantics retained.

## D06 — Lazy metadata and cache invalidation

**Status:** done

**Depends:** D04. **Owns:** Java `metadata/`, `kode-database/src/metadata.rs`.
**Read:** DatabaseMetadata.kt, DatabaseModels metadata types; MetadataCacheTest.

1. Port normalized catalogs/schemas/categories/tables/views/columns/procedures/
   functions/packages/types/indexes/primary/foreign keys and capability discovery.
2. Implement existing JDBC metadata calls and vendor-specific fallbacks present in
   source, preserving qualified names, attributes and leaf/expandable flags.
3. Cache by datasource/connection epoch/request; invalidate on reconnect, context
   change, explicit refresh and relevant executed schema changes.

**Accept:** original invalidation tests, H2 metadata tree and representative server
   fixtures; unsupported metadata becomes a message/capability, not an empty fake
   success. Late metadata from old connection cannot populate new tree.

## D07 — Database explorer, connection and driver dialogs

**Status:** done

**Depends:** D01, D03, D04, D06, F05, W03.
**Owns:** `kode-tui/src/panels/database/{tree,connection_dialog,object_menu}.rs`.
**Read:** DatabasePanelView.kt, DatabaseConnectionDialog.kt, DatabaseObjectMenu.kt,
JdbcDriverDownloadDialog.kt and Main database callbacks.

1. Port new/edit/delete datasource, test/save, connect/disconnect, lazy expansion,
   refresh, object actions and SQL-console launch. Preserve `n`, Enter/`c`, `s`,
   Alt+T, Alt+D and Ctrl+S in their proper focus contexts.
2. Port bounded connection fields and separate buttons, password masking,
   driver/provider selection, properties and URL Recents/Standard suggestions.
3. Attach install/progress/cancel and error dialogs; retain focus/caret and entered
   values through validation or connection errors. Persist datasource state.

**Accept:** complete keyboard/mouse dialog traversal, no field clipping/neighbor
   hit confusion, safe recents, unavailable Java/driver messages and metadata
   expansion while typing elsewhere. Driver install UI uses real bridge events.

## D08 — SQL editor, statement targeting and result panel

**Status:** done

**Depends:** D05, D07, E12, E09, L02.
**Owns:** `kode-database/src/{statement,console}.rs`,
`kode-tui/src/panels/database/{console,results,session_dialog,statement_menu}.rs`.
**Read:** SqlStatementLocator in SqlQuery.kt, SqlConsoleManager.kt, SqlResultPanel,
SqlSessionSaveDialog, SqlStatementMenu and Main SQL toolbar/dispatch helpers.

1. Port current-statement/selection/all targeting with strings/comments and offsets,
   console identities/title counters/history, saved sessions and context bindings.
2. Reuse editor for SQL, transient execution highlight, syntax, selection and copy.
   Wire Ctrl+Enter/current, Ctrl+Shift+Enter/all, Escape/cancel and Ctrl+Alt+A/C/R
   for autocommit/commit/rollback with correct popup/search focus precedence.
3. Port lower result panel with result-set tabs, rows/type labels/NULL/error/warnings,
   Alt+Arrows/Alt+Page scrolling, Alt+Tab switching, sizing and independent focus.

**Accept:** SqlStatementLocatorTest, session save/restore and UX-101/104/106. SQL
   strings containing semicolons don't split wrongly; multiple results/update
   counts accessible; cancel does not mutate SQL text or steal editor focus.

## D09 — End-to-end external JDBC qualification

**Status:** in-progress

**Depends:** D08, W03, W05. **Owns:** `jdbc-bridge/src/test/`,
`kode-database/tests/bridge_integration/`, `docs/parity/jdbc.md`.

1. Run the Rust client against the actual Java helper for all original H2 JDBC
   integration cases and the wire failure/cancellation/backpressure matrix.
2. Replay create datasource→download/load→test→connect→expand→query→rollback→save
   session→restart. Add local PostgreSQL/MySQL drivers where test fixtures exist;
   record vendor entries as tested/untested without dropping them.
3. Run with no Java, wrong Java, helper crash and workspace switch. Inspect that
   editor/Git/REST/native index still work and child processes are reaped.

**Accept:** full JDBC feature matrix, Rust/Java fixture agreement, all catalog and
   URL tests, no secret persistence/logging and no automatic SQL replay. External
   Java requirement and failure recovery documented for distribution.
