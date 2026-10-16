# U09 language-server installation repair

Status: in-progress. Isolated upstream installs and LSP lifecycle pass on Linux
x86_64 for all six reported launchers. Windows x86_64 passes the four Node servers
and 26 applicable regression tests. Windows JDT repair, clangd execution, remaining
native platforms and ARM64 clangd installation remain open; U09 is not complete.

| Surface | Implemented and verified | Remaining qualification | Status |
|---|---|---|---|
| TypeScript | Server 6.0.2 with TypeScript 6.0.3, pinned archives, isolated dependency tree, Node >=22.22.2; Linux/Windows x86_64 initialize/didOpen/shutdown/reap | Other native OS/CPU combinations | in-progress |
| VSCode HTML/CSS/JSON | 4.10.0, separate language entries/launchers, complete 32-package closure, Node >=18; all three Linux/Windows x86_64 lifecycles pass | Other native OS/CPU combinations | in-progress |
| JDT LS | Exact 1.62.0 milestone tarball, checksum, direct Java launcher, platform configuration, project-specific configuration/data; Linux lifecycle passes with external Java 25 | Other native OS/CPU combinations, Windows ARM64 configuration | in-progress |
| clangd | 23.1.0 Linux x86_64 ZIP install and lifecycle; Windows AMD64 PE and universal macOS x86_64/arm64 Mach-O downloads/layout/digests inspected | Native Windows/macOS execution; actual Linux/Windows ARM64 install adapter and runtime | in-progress |
| Common installer | Declarative dependency archives, bounded Debian ar/GZIP/Zstandard import, selected file trees, prerequisite probes, cancellation, staged publication/rollback and configured command precedence; regression suite passes | Platform-specific closure/launcher coverage above | in-progress |
| Settings diagnostics | Owned action/server/request error snapshots; F4 Details, wrapping, scroll, complete copy, focus restoration and cleanup verified in application and actual CLI PTYs | Include in final assembled release replay | done |

The catalog uses verified official package metadata and archives. TypeScript's
upstream recommends compiler 6; blindly selecting the registry's latest compiler
7 would use an incompatible API. Packages unpack directly into isolated roots:
no global npm install, package manager or lifecycle script runs. Dependency archive
hashes, licenses, engines and nested package placements are recorded in
[`resources/lsp/npm-lock.json`](../../resources/lsp/npm-lock.json).

The pinned JDT milestone reports server version `1.62.0-SNAPSHOT`. Its actual
`org.eclipse.core.filesystem_1.12.0.v20260927-0940.jar` manifest requires
`JavaSE-25`, despite upstream README/wrapper text still mentioning Java 21. The
Java 21 startup failure was reproduced and diagnosed from the actual bundle;
the catalog now rejects that prerequisite before installation. Qualification used
a pre-existing external Temurin 25.0.1+8, without changing system Java. `JAVA_HOME`
or PATH selects the external JDK. Java/Node probes use managed children on the
existing worker/runtime, with cancellation and a five-second timeout.

The existing bounded HTTP/extraction, lock, cancellation, old-install retention
and atomic publication remain. Package closures share the 256 MiB download,
1 GiB extraction and 100,000-entry limits. Explicit configured executable commands
retain precedence. The JDT launcher avoids Python/batch wrappers and copies a
bounded configuration template into each project's cache before launching; it
keeps mutable project data out of installed templates. Archive identity versions
the configuration cache during upgrades while retaining project data. Configuration
and data locations use encoded file URLs to preserve special characters through
Java/Equinox argument handling. The URL change passes the real Linux JDT replay;
native Windows verification is pending.

## Evidence

- [`U09-upstream.json`](../parity/U09-upstream.json): final fresh replay of six
  actual public downloads through production `HttpFetcher`, checksum/executable
  validation, LSP initialize, didOpen, shutdown and reaping on Linux x86_64, using
  production `LspService` defaults (82.26 seconds for all six). JDT initializes in
  about 3.4 seconds. A subsequent installed JDT replay validates the URL change.
- [`U09-assets.json`](../parity/U09-assets.json): verified archive sources/hashes,
  JDT bundle prerequisite, clangd architecture headers and Linux library needs.
  Download/header inspection does not qualify another native platform.
- [`U09-pty.json`](../parity/U09-pty.json): four real CLI cases at 100x34 and
  60x24. Actual HTTP 503 with a URL exceeding 6,000 characters opens full details;
  End reaches the tail and Ctrl+C copies the entire diagnostic into a file that
  is saved. An old Node prerequisite fails before HTTP or install-root creation.
  Failed installs retain no partial server or lock, and terminal modes restore.
- `cargo +stable test -p kode-lsp -p kode-tui`: 342 pass, six opt-in tests ignored
  (four external JDBC cases, the six-server upstream replay and the Debian archive
  qualification). New cases cover Debian formats/corruption/window bounds,
  selected links/traversal, independent dependency selection and manifest size.
- [`U09-deb.json`](../parity/U09-deb.json): production `HttpFetcher` and `InstallJob`
  download, verify and extract the official ARM64 clangd Debian archive on Linux
  x86_64. The actual ARM64 ELF, canonical executable and license are retained;
  unselected aliases/control payloads are absent. This qualifies the archive
  adapter and layout, not ARM64 execution. LLVM/Clang library and resource archive
  hashes/layouts and ELF dependency names were also verified for the next step.
- [`U09-windows.json`](../parity/U09-windows.json): 26 applicable native Windows
  regression tests pass. Fresh upstream TypeScript/HTML/CSS/JSON installs pass
  production lifecycle defaults with native process-exit checks. JDT fails on its
  cache path; the file URL repair remains to be replayed on Windows. clangd has not
  run there. An earlier HTML timeout passed on both installed and fresh replays
  after the host restart, with the original deadline unchanged; its cause is not
  established.
- `cargo +stable clippy -p kode-lsp -p kode-tui --all-targets -- -D warnings` and
  formatting pass on stable Rust 1.99.1. Package/prerequisite/override/JDT and Debian
  archive tests pass. Locked dependency credits include the two new MIT decoder/
  hash crates. The upstream replay is run
  separately, explicitly.

Reproduce upstream qualification with fresh owned `KODE_TEST_LSP_ROOT`,
`KODE_TEST_NODE_BIN` and optional `KODE_TEST_JAVA_BIN` directories:

```sh
cargo +stable test -p kode-lsp --test upstream_install -- --include-ignored --nocapture
python3 scripts/test-lsp-install-terminal.py --binary /path/to/kode
```

`KODE_TEST_LSP_REPLAY=<server-id>` explicitly replays an existing owned install
without downloading it. It writes separate `replay-<server-id>.json` evidence and
never counts as a fresh install. Fresh runs retain completed case records if a
later server fails; only a completely successful selected run is marked passed.

## Remaining work

1. Implement and qualify practical ARM64 clangd inputs. Official LLVM 23 ARM64
   monolithic packages exceed the current install budget and need formats the
   installer does not yet support; adding a URL alone would not fix installation.
   The smaller official apt.llvm.org package now passes the bounded Debian adapter
   and selected-file qualification. Complete its isolated library/resource closure,
   loader/prerequisite checks and ARM64 lifecycle before adding it to the shipped
   catalog. The qualification fixture records the exact dated upstream build,
   rather than labeling it as the stable 23.1.0 GitHub artifact.
2. Exercise actual installs and lifecycle on Windows/macOS and remaining CPU
   combinations. The authorized Windows SSH host is reachable and reports Windows
   x86_64. The owned qualification project is
   `C:/Users/alfu64/Development/kode-rs-u09-20261016-01`; existing user projects are
   preserved. Isolated stable GNU Rust 1.99.1 is installed for U09 only; it does
   not qualify the Z03 MSVC artifact targets. Finish Windows JDT/clangd and Clippy
   first. SSH became unavailable again after the four Node-server passes and JDT
   failure; retain the existing roots/logs and resume when reachable.
3. Finish U12 and all Uxx acceptance before resuming
   [Z03's full artifact matrix](Z03-ARTIFACT-MATRIX.md). Windows project location
   and permission remain as recorded there.

The terminal replay also exposed an initial-file-load focus race when Settings is
selected before loading finishes. This remains explicitly tracked under E13.
The U09 replay waits for the initial file to render and does not claim to fix that
race. No remote CI result, ARM64 runtime, macOS runtime or release artifact success
is implied by the Linux and partial Windows evidence.
