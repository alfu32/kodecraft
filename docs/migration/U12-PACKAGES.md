# U12 database packages

Status: suspend (user-requested). This inventory covers all 59 shipped driver entries. Capability
flags describe the selected helper provider, not a claim that every listed vendor
has been run. Existing driver classes, aliases, downloads and URL handling remain.

Oracle package discovery, overload-aware members and complete specification/body DDL
are implemented in the helper; native helper qualification passes against an isolated
Oracle Free container. MariaDB package listing, declaration-based member discovery
and actual specification/body retrieval also pass against MariaDB 11.8.9. These are
native helper, Rust metadata-client and package-source TUI action reports; they do
not represent full U12 qualification. The TUI replays verify actual source in a
read-only editor. Tibero now has an explicit provider based on its documented
catalog views; native Tibero and Db2 qualification and broader TUI interaction
qualification remain unfinished. U12 is not accepted. See the
[handoff](IMPLEMENTATION-HANDOFF.md).

| Provider family | Package operations and remaining work | Source |
|---|---|---|
| Oracle | ALL_OBJECTS, ALL_PROCEDURES/ALL_ARGUMENTS, DBMS_METADATA.GET_DDL; bind identifiers, preserve overload identity; native helper and package-source TUI action pass, broader TUI qualification pending | [Oracle dictionary](https://docs.oracle.com/en/database/oracle/oracle-database/26/refrn/ALL_PROCEDURES.html), [DDL API](https://docs.oracle.com/en/database/oracle/oracle-database/26/arpls/DBMS_METADATA.html) |
| Tibero | Implemented package listing/members and reads actual package specification/body source from `ALL_SOURCE`; provider fixtures pass, native qualification pending ([report](../parity/U12-tibero.json)). Uses Tibero catalog views rather than assuming full Oracle API compatibility | [Tibero 7 static views](https://technet.tmax.co.kr/upload/download/online/tibero/pver-20240502-000002/reference/ch_static_view.html), [Tibero 7 tbPSM `GET_DDL` reference](https://tmaxtibero.blog/wp-content/uploads/2025/01/Tibero_7_tbPSM-Reference-Guide_v7.2.2.pdf) |
| MariaDB | SHOW PACKAGE STATUS plus declaration-based members and actual SHOW CREATE PACKAGE/BODY; native helper and package-source TUI action pass, broader TUI qualification pending | [Package status](https://mariadb.com/docs/server/reference/sql-statements/administrative-sql-statements/show/show-package-status), [DDL](https://mariadb.com/docs/server/reference/sql-statements/administrative-sql-statements/show/show-create-package), [metadata limitation](https://jira.mariadb.org/browse/MDEV-33385) |
| Db2 LUW | Bound application packages are distinct from PL/SQL routine packages. Query SYSCAT.PACKAGES and SYSCAT.STATEMENTS; preserve schema/name/version/UNIQUE_ID for source retrieval and expose SQL statements without inventing package DDL. The provider accepts empty `PKGVERSION` with null or empty statement `VERSION`; current helper fixtures pass, native vendor qualification pending ([current report](../parity/U12-db2.json)) | [Packages](https://www.ibm.com/docs/en/db2/12.1.x?topic=views-syscatpackages), [statement source](https://www.ibm.com/docs/en/db2/12.1.x?topic=views-syscatstatements) |

## Catalog routing inventory

Only an implemented, recognized database-product provider advertises packages.
Other entries continue through standard JDBC metadata and do not gain speculative
package nodes. A missing vendor provider is not evidence that its engine lacks all
forms of modules, libraries or packages. Custom JDBC connections use the actual
reported database product, so a recognized provider is not tied to one catalog ID.

| Catalog ID | Package provider state |
|---|---|
| postgresql | Standard JDBC metadata; no vendor package provider advertised |
| mysql | Standard JDBC metadata; no vendor package provider advertised |
| mariadb | Native helper, Rust metadata client and package-source TUI action qualified; broader TUI interactions pending |
| oracle | Native helper, Rust metadata client and package-source TUI action qualified; broader TUI interactions pending |
| sqlserver | Standard JDBC metadata; no vendor package provider advertised |
| db2 | Db2 LUW provider implemented, native qualification pending; other Db2 platforms require explicit providers |
| sqlite | Standard JDBC metadata; no vendor package provider advertised |
| h2 | Standard JDBC metadata; no vendor package provider advertised |
| hsqldb | Standard JDBC metadata; no vendor package provider advertised |
| derby-embedded | Standard JDBC metadata; no vendor package provider advertised |
| derby-client | Standard JDBC metadata; no vendor package provider advertised |
| firebird | Standard JDBC metadata; no vendor package provider advertised |
| informix | Standard JDBC metadata; no vendor package provider advertised |
| iris | Standard JDBC metadata; no vendor package provider advertised |
| tibero | Tibero helper provider implemented; native vendor qualification pending |
| teradata | Standard JDBC metadata; no vendor package provider advertised |
| vertica | Standard JDBC metadata; no vendor package provider advertised |
| exasol | Standard JDBC metadata; no vendor package provider advertised |
| sap-hana | Standard JDBC metadata; no vendor package provider advertised |
| snowflake | Standard JDBC metadata; no vendor package provider advertised |
| redshift | Standard JDBC metadata; no vendor package provider advertised |
| bigquery | Standard JDBC metadata; no vendor package provider advertised |
| trino | Standard JDBC metadata; no vendor package provider advertised |
| presto | Standard JDBC metadata; no vendor package provider advertised |
| hive | Standard JDBC metadata; no vendor package provider advertised |
| databricks | Standard JDBC metadata; no vendor package provider advertised |
| clickhouse | Standard JDBC metadata; no vendor package provider advertised |
| duckdb | Standard JDBC metadata; no vendor package provider advertised |
| singlestore | Standard JDBC metadata; no vendor package provider advertised |
| spanner | Standard JDBC metadata; no vendor package provider advertised |
| ignite2 | Standard JDBC metadata; no vendor package provider advertised |
| mongodb-atlas | Standard JDBC metadata; no vendor package provider advertised |
| neo4j | Standard JDBC metadata; no vendor package provider advertised |
| athena | Standard JDBC metadata; no vendor package provider advertised |
| documentdb | Standard JDBC metadata; no vendor package provider advertised |
| elasticsearch | Standard JDBC metadata; no vendor package provider advertised |
| cockroachdb | Standard JDBC metadata; no vendor package provider advertised |
| greenplum | Standard JDBC metadata; no vendor package provider advertised |
| yugabytedb | Standard JDBC metadata; no vendor package provider advertised |
| timescaledb | Standard JDBC metadata; no vendor package provider advertised |
| citus | Standard JDBC metadata; no vendor package provider advertised |
| aurora-postgresql | Standard JDBC metadata; no vendor package provider advertised |
| tidb | Standard JDBC metadata; no vendor package provider advertised |
| oceanbase-mysql | Standard JDBC metadata; no vendor package provider advertised |
| aurora-mysql | Standard JDBC metadata; no vendor package provider advertised |
| azure-sql | Standard JDBC metadata; no vendor package provider advertised |
| sqlserver-localdb | Standard JDBC metadata; no vendor package provider advertised |
| spark-thrift | Standard JDBC metadata; no vendor package provider advertised |
| sybase-ase | Standard JDBC metadata; no vendor package provider advertised |
| denodo | Standard JDBC metadata; no vendor package provider advertised |
| mimer | Standard JDBC metadata; no vendor package provider advertised |
| openedge | Standard JDBC metadata; no vendor package provider advertised |
| phoenix | Standard JDBC metadata; no vendor package provider advertised |
| tarantool | Standard JDBC metadata; no vendor package provider advertised |
| couchbase | Standard JDBC metadata; no vendor package provider advertised |
| cassandra | Standard JDBC metadata; no vendor package provider advertised |
| redis | Standard JDBC metadata; no vendor package provider advertised |
| dynamodb | Standard JDBC metadata; no vendor package provider advertised |
| custom-jdbc | Recognized product detection; otherwise no package provider advertised |

## Qualification and remaining work

- Java helper tests include provider selection across all shipped database families, absent categories on H2, bound
  schema/package names with quotes/wildcards/Unicode, overload identity, exact DDL
  characters, missing/denied source, reader limits and cancellation cleanup.
- `scripts/test-oracle-packages.py` runs only with an explicitly supplied owned test
  schema. It refuses an existing fixture package, creates its own package, verifies
  actual names/members/DDL through the external helper and removes that package.
  Its report labels helper scope and does not claim TUI qualification.
- `scripts/test-db2-packages.py` is ready for a read-only native Db2 LUW helper
  check when an owned endpoint and an existing bound package are available. Set
  `KODE_TEST_PACKAGE_DB2_OWNED=1` and the same prefix's `URL`, `USER`, `PASSWORD`,
  `SCHEMA`, `PACKAGE`, `VERSION`, `SOURCE_MARKER`, and `JCC_VERSION` variables;
  `VERSION` may be empty. If the schema/name/version identifies several bound
  packages, also set `UNIQUE_ID` to the expected 16-digit hexadecimal value. Run
  `python3 scripts/test-db2-packages.py --report docs/parity/U12-db2-native.json`.
  It checks the selected opaque package identity, provider capabilities and actual
  SQL statement text, then closes the connection and helper. It writes a passing
  report only after a live run; the existing [Db2 fixture report](../parity/U12-db2.json)
  remains unqualified until then.
- `scripts/test-tibero-packages.py` is ready for a read-only native Tibero helper
  check against an existing package with a specification, body and known member.
  Set `KODE_TEST_PACKAGE_TIBERO_OWNED=1` and the same prefix's `URL`, `USER`,
  `PASSWORD`, `JAR`, `SCHEMA`, `PACKAGE`, `MEMBER`, `MEMBER_TYPE` (`FUNCTION` or
  `PROCEDURE`) and `SOURCE_MARKER` variables. `JAR` must be an existing local
  Tibero JDBC driver. Run
  `python3 scripts/test-tibero-packages.py --report docs/parity/U12-tibero-native.json`.
  The script checks provider capabilities,
  exact package/member ownership and both stored DDL documents, then closes the
  connection and helper. It writes a passing report only after a live run; the
  [current Tibero report](../parity/U12-tibero.json) remains unqualified.
- Native qualification exposed an Oracle idle disconnect error: a simultaneous
  abort raced the connection owner's normal close. The fix keeps independent abort
  for busy work and acknowledges completed operations only after the owner is idle.
  The focused connection-owner regression and real Oracle replay now pass.
- All 79 Java helper tests pass. The helper test for early `metadata.cancel`
  dispatch verifies the canceled job returns SQLSTATE 57014 while a neighboring
  metadata list on the same connection still succeeds. The [Oracle report](../parity/U12-oracle.json) and [MariaDB report](../parity/U12-mariadb.json)
  each include native helper and live Rust metadata-client evidence for package
  listing, members, actual source and post-refresh fixture removal. They also verify
  exact Unicode/wildcard names, schema ownership,
  two Oracle overloaded functions or two MariaDB functions, one procedure, complete
  specification/body DDL, fixture removal and helper reaping. They also record the
  live source-action replay; the broader TUI remains in progress. The Oracle run
  verifies clean idle disconnect.
- The current helper artifact, SHA-256
  `35328cbed66ea0aeb33ecabc3afd1f84e175b757a6d9dd96ffd09b14563c4cc6`,
  passes all 79 Java tests, including the corrected Db2 unversioned-package
  fixture. A focused explorer/menu replay also preserves Db2's opaque package
  identity and routes SQL source without offering fabricated DDL. The
  Oracle/MariaDB live helper, Rust metadata-client and TUI source-action replays
  now pass against this same artifact. The reports record the hash for each layer.
- On 2026-10-17, the native helper scripts were rerun against the healthy owned
  Oracle Free and MariaDB 11.8.9 containers. Both passed package listing, member and
  DDL checks, removed their owned fixture package, and reaped the helper. Fresh
  Rust metadata-client and actual TUI package-source replays also passed against
  both vendors. The reports record each layer's rerun separately. The TUI runs
  verify schema-aware expansion, source-menu dispatch, metadata-refresh rejection
  of a retrieved stale source, rejection after focus moves to Settings, successful
  reopen into the read-only editor, Escape cancellation of a 1.4 MB package source
  without replacing the existing editor/focus, and fixture cleanup; Oracle also
  exercises SQL and transaction behavior.
- The package tree retains Enter for lazy member expansion and now exposes
  `[source]` on source-capable package rows. `M` opens the selected object's menu;
  the native Oracle/MariaDB TUI replay uses this keyboard route instead of
  inserting the first menu directly, then clicks the rendered `[source]` cells
  through the full application dispatcher. A focused render/input replay also
  checks the button on an expandable package.
- The Rust database/TUI serial suite passes with zero failures and twelve
  ignored, including the two opt-in Oracle/MariaDB live-client replays. The six shared wire
protocol tests are included in that total. The native source-menu action and read-only editor pass for Oracle/MariaDB. The
controller regression verifies source cancellation, rejects stale context, and
ensures a late superseded source result cannot replace the newer request's loading
state. The live TUI replays also verify metadata-refresh and Settings-focus barriers,
plus Escape cancellation while loading large source. A fresh [Oracle/MariaDB
member-expansion replay](../parity/U12-tui-members.json) drives Enter through the
application, then checks three rendered package members, selected schema/package
ownership, two functions, one procedure and distinct IDs. The controller regression
verifies a readable source-error notice and F4 details. The H2
  TUI replay verifies unsupported-provider capability flags and absence of package
  categories in both the Rust metadata result and rendered explorer
  ([report](../parity/U12-h2.json)); the Java family matrix covers provider routing
  across the shipped catalog.
- `package_source_opens_as_replaced_ephemeral_readonly_sql_history` attempts
  paste and Ctrl+S against the package-source document; both are rejected, the
  contents remain exact, Ctrl+A/Ctrl+C copies the source, Ctrl+F/Ctrl+Enter
  finds source text, and the virtual label is not written to disk. The live
  Oracle and MariaDB TUI source-menu replays passed again on 2026-10-17.
- Package-source and ordinary `metadata.list` cancellation use unique job IDs and
  the reserved priority control request. The helper records cancellation that
  arrives before dispatch, schedules only that job's JDBC `Statement.cancel` off
  the owner, and waits for the cancel call before releasing its result set. Rust
  keeps the request pending through cancellation and never caches cancelled work;
  Java, Rust and H2 tests cover active cancellation. Java also verifies the early
  cancel-before-dispatch path and job isolation. Deterministic provider fixtures
  block ordinary metadata listing and DDL retrieval, then simulate a
  driver cancellation exception (SQLSTATE 72000). Both paths map it to SQLSTATE
  57014 and verify active statement cancel plus result/statement cleanup; package
  source returns no partial response. A DDL reader I/O failure during cancellation
  also maps to 57014 with reader cleanup. Fresh external-helper replays against
  Oracle Free and MariaDB 11.8.9 cancel while a JDBC statement is attached during
  retrieval of 1.4 MB package DDL, return SQLSTATE 57014, remove each owned fixture
  and reap the helper ([Oracle](../parity/U12-cancel-oracle.json),
  [MariaDB](../parity/U12-cancel-mariadb.json)).
- Qualify Tibero and Db2 against real vendor servers, and run Rust/TUI
  replays for lazy expansion, refresh/context barriers, source-error presentation,
  and read-only source editors. The shared source response
  fixture and opaque versioned object identity contract are in place. Record each
  vendor as qualified or unqualified; unavailable vendors never count as passing
  execution.

The B009 Oracle refresh/statement-terminator request is complete under U13 and
participates in E13/Z03/Z04 prerequisites. Its source UAT text is preserved.
