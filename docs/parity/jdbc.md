# External JDBC qualification matrix

Status: in-progress (D09). Source baseline:
`7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68`, clean.
Linux x86_64; Rust/Cargo stable 1.99.0; Java 21.0.9; Maven 3.9.16; H2 2.2.224.
All configurations, credentials, databases, repositories and sessions are isolated
synthetic fixtures. No developer datasource or database was accessed.

| Requirement | Authoritative evidence | State |
|---|---|---|
| Original `connectsExecutesAndIntrospectsH2` | Actual-helper native execution + metadata tests and `database_app` install/test/connect/tree workflow | passed |
| Original `returnsSqlErrorsAsResults` | Native and application H2 SQLSTATE/error/transaction tests | passed |
| Original `supportsUpdateCountsAndRollback` | Native H2 insert/update/rollback and application transaction controls | passed |
| Frames, hello, fragmentation, IDs, malformed/oversized/poisoned data | Rust `protocol`/`bridge`, Java `FrameCodecTest`/`RpcServerTest`, shared fixtures | passed |
| Maven graph, custom repository, local multi-JAR, aliases, classloaders | Java driver suite + native actual-helper driver test | passed with isolated local repositories/H2; external vendor downloads untested |
| Mixed results/counts, NULL/empty/binary/decimal/time, truncation, warnings/chained errors | Java query proxies/H2 + native execution and UI result fixtures | passed; proxy evidence is distinct from engine evidence |
| Independent cancel, blocked output, partial/unknown completion | Native execution fixtures, Java query tests and actual OS-pipe backpressure harness | passed, including D09 rerun |
| Lazy metadata, context/reconnect/DDL/refresh barriers | Native metadata tests and real H2 UI expansion while typing | passed |
| Current/selection/script, EOF safety, focus/search priority | Locator, editor-host, real application and SQL PTY tests | passed |
| File export, dirty SQL/session restart, no Java on restore | SQL save/conflict worker regression + four SQL PTYs | passed |
| Actual helper crash during an executing multi-statement script | `jdbc_qualification::actual_helper_crash_during_sql_preserves_outcome_and_native_editor_git_index_then_explicit_reconnect_does_not_replay` | passed |
| Native editor, save, Git and SQLite index after helper death | Same actual crash test edits/saves Kotlin, observes indexed symbol and real Git status, without starting Java | passed |
| Explicit helper restart/reconnect never replays SQL | Same test inserts before a billion-row aggregate, kills the recorded helper through pidfd, sees unknown outcome, restarts/reconnects and verifies exactly one durable row | passed |
| Busy workspace switch, stale result disposal, process reaping | `jdbc_qualification::workspace_switch_during_sql_reaps_helper_preserves_buffer_and_drops_old_results_without_replaying` | passed |
| Missing Java / old Java / launcher diagnostic redaction | Existing native and no-Java UI tests + `jdbc_qualification::wrong_java_version_is_actionable_redacted_and_native_edit_save_index_remain_available` | passed |
| Credentials absent from public config/session and diagnostics | Native credential/export tests, service mutation/shared-reference tests and launcher redaction regression | passed; deliberate user SQL buffer persistence remains enabled |
| REST operation remains independent during JDBC failures | Actual `jdbc_qualification` unsupported-Java and helper-crash cases open the real request/body/response hosts and send the current buffer to loopback before continuing native save/index work; no JDBC restart | passed |
| Non-H2 vendor engines | Complete [59-driver inventory](jdbc-drivers.json) preserves each driver and records runtime qualification separately | 58 explicitly untested; no PostgreSQL/MySQL server fixture configured |
| Distribution/platform behavior | Z03/Z04 | pending |

`scripts/test-jdbc-bridge.sh` builds the helper, runs its Maven tests and all native
and database UI/service/qualification tests including opt-in actual-helper cases.
The earlier D08 final run passed 62 Java / 75 Rust tests. The D09 rerun passed all
62 Java and 48 backend cases, then exhausted disk space while linking UI tests.
`cargo clean -p kode-tui` removed 36 GiB of generated artifacts. The subsequent
complete database UI/service/qualification command passed all 30 cases, including
the three D09 cases and binary-hex correction. Database/TUI all-target clippy
passes with `-D warnings`. The final REST-isolation rerun passes all three
qualification cases with zero ignored cases; D09's feature matrix is complete.

Recovery is explicit: after a crash, check the database outcome, press Alt+6 then
`R` to restart the retained failed helper epoch, then `c` to connect. SQL text is
retained and never replayed. Error feedback now names this sequence. A failed
epoch is intentionally not restarted simply because background work is pending.

D09 found that binary cells displayed only their size despite the JDBC contract's
hex requirement. The UI now decodes only a bounded base64 prefix for visible
hex/size display; `database_results` verifies `0x00FF (2 bytes)` and NULL separately.

This matrix is not a release-completion claim. The remaining REST isolation gate
must run against implemented REST owners rather than a fake service. All catalog
entries remain selectable; untested vendor engines are not represented as passed.
