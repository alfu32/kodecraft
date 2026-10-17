# Implementation handoff — 2026-10-17

The full migration goal remains open. The dashboard has 80 tasks: 75 done,
three in progress (E13, U09 and U12), two to do (Z03 and Z04).
Do not commit or push without the user's instruction. Bash, formatting and tests
are already authorized; do not ask again. Use stable Rust, currently 1.99.1.
Preserve user-owned UAT text and unrelated working tree changes.

## U09: original LSP failures repaired; platform acceptance remains open

All six reported servers pass fresh production installation and LSP lifecycle on
Linux x86_64 and native Windows x86_64. Windows also passes 33 regression tests
(two ignored) and native Clippy. The combined Linux LSP/TUI suite passes 349
(seven opt-in ignored); the separate production-catalog ARM64 run passes under
QEMU. Native ARM64 remains unqualified.
See [integration status](U09-INTEGRATION.md) and [parity evidence](../parity/U09.md).

The Linux ARM64 clangd dependency closure is now a pinned production catalog
version. A fresh catalog-backed run passes a real ARM64 binary under private
QEMU: install, initialization, parsing bundled headers, definition, shutdown and
reaping. This does not qualify native ARM64 hardware. Remaining native platforms
keep U09 in progress. Do not repeat the completed x86_64 investigation.

The Windows SSH host is `alfu64@192.168.234.128` and is currently shut down at
the user's request. If Windows testing becomes necessary, pause and ask the user
to turn it on before connecting. The owned qualification checkout is
`C:/Users/alfu64/Development/kode-rs-u09-20261016-01`. Its isolated GNU toolchain
qualifies U09, not the Z03 MSVC distribution. Source overlays are hash guarded;
do not assume later local files have been sent to Windows. U12 is local only.

## U12: Oracle and MariaDB helpers pass; broader package work is unfinished

See [provider inventory](U12-PACKAGES.md), the [native Oracle helper report](../parity/U12-oracle.json) and the [native MariaDB report](../parity/U12-mariadb.json). All 79 Java tests pass. Native Oracle verifies listing, exact schema/name handling, overload identity, specification/body DDL, fixture removal, clean disconnect and helper reaping. Native MariaDB verifies listing, Unicode/wildcard names, declaration members, specification/body DDL, fixture removal and helper reaping.
The connection-owner fix avoids concurrent abort during normal idle close, while
retaining independent abort for busy work.

Rust database/TUI compilation passes with the new source client, package menu
action and ephemeral read-only shared editor. The source client checks connection
epoch, context revision and refresh generation before publication. The editor
has no execution/save target and retains at most one package-source document.
The Rust database metadata client now passes live Oracle and MariaDB replays through source retrieval and fixture removal after refresh; the native TUI source-menu action opens the actual specification/body in a read-only editor with save/execute disabled; broader TUI qualification remains open.

Handoff verification: `cargo +stable test -p kode-database -p kode-tui` passes
the full Rust database/TUI suite, including the two opt-in Oracle/MariaDB live-client
replays, with no failures. The six shared wire-protocol tests passed in this run. Formatting, `git diff --check` and task
ledger validation pass. Database-client and TUI source-action live-vendor replays pass for Oracle and MariaDB; broader TUI interaction qualification remains listed below.

Package-source and ordinary metadata-list cancellation use unique job IDs and the
reserved `metadata.cancel` operation. The helper handles cancellation-before-
dispatch, targets one statement and joins its cancel call before releasing its
result set. Rust waits for the cancellation path and never caches cancelled work.
A shared fixture and typed response round-trip test cover source and cancellation.
The Oracle and MariaDB helper, Rust metadata-client and TUI source-action replays
pass against helper SHA-256 `35328cbed66ea0aeb33ecabc3afd1f84e175b757a6d9dd96ffd09b14563c4cc6`.
This build includes the Db2 empty-version correction and passes all 79 Java tests
and the actual H2 TUI replay as well.

Remaining U12 work:

- Oracle/MariaDB now have live helper/client/TUI evidence for listing, members,
  actual DDL, refresh and focus barriers, Escape cancellation, and the read-only
  source editor. A local regression also verifies copy works while paste and save
  are rejected. Re-run these checks after material changes.
- Native Db2 LUW and Tibero qualification remains unavailable. Db2 LUW has a
  catalog provider and fixture coverage for empty package versions as well as
  nullable or empty statement versions. IBM documents
  `SYSCAT.STATEMENTS` with schema/name/version/unique-id keys and actual SQL text;
  preserve that identity and never invent CREATE PACKAGE DDL for bound SQL packages.
  A read-only native helper qualification script is prepared at
  `scripts/test-db2-packages.py`; it requires an explicitly marked owned endpoint,
  an existing package, expected source marker and pinned JCC version. It has not
  run because no Db2 endpoint or credentials are configured. It does not qualify
  the TUI.
- Record each vendor as qualified or unqualified. Unavailable vendor execution does
  not count as passing qualification.

## Remaining sequence

U13/B009 Oracle refresh, selected/script execution, cancellation, error, and transaction acceptance is complete. It remains included in E13/Z03/Z04 dependencies.
Finish U09 and U12 before resuming [Z03](Z03-ARTIFACT-MATRIX.md): six portable
OS/CPU combinations, AppImage, Snap, MSI/MSIX, DMG and separate optional JVM assets.

## Local evidence and reusable commands

The host reboot erased previous `/tmp` qualification caches. Current durable cache:
`/home/devlin/.cache/kode-rs-qualification/`. U09 logs/Windows overlays are in
`u09-20261017`; U12 logs, native report and fixture startup scripts are in
`u12-20261017`. The Oracle/MariaDB environment files contain private test secrets;
do not print them. Two owned containers remain available for U12 work:
`kode-rs-u12-20261017-oracle` and `kode-rs-u12-20261017-mariadb`. Verify ownership
before eventual cleanup; do not remove user containers.

Use one Cargo build at a time with this target:

```sh
env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS \
  CARGO_TARGET_DIR=/home/devlin/.cache/kode-rs-qualification/target \
  CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo +stable test -p kode-database -p kode-tui
python3 scripts/tasks.py --check
```

Java helper builds use Java 21 and the credential-independent build script:

```sh
env JAVA_HOME=/home/devlin/.sdkman/candidates/java/21.0.9-tem \
  PATH=/home/devlin/.sdkman/candidates/java/21.0.9-tem/bin:$PATH \
  KODE_JDBC_BUILD_CACHE=/home/devlin/.cache/kode-rs-qualification/maven \
  bash scripts/build-jdbc-helper.sh
```
