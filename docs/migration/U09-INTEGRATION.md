# U09 language-server installation repair

Status: in-progress. Isolated upstream installs and LSP lifecycle pass on Linux
x86_64 for all six reported launchers. Other native platforms and ARM64 clangd
installation remain open; U09 is not complete.

| Surface | Implemented and verified | Remaining qualification | Status |
|---|---|---|---|
| TypeScript | Server 6.0.2 with TypeScript 6.0.3, pinned archives, isolated dependency tree, Node >=22.22.2; Linux initialize/didOpen/shutdown/reap | Other native OS/CPU combinations | in-progress |
| VSCode HTML/CSS/JSON | 4.10.0, separate language entries/launchers, complete 32-package closure, Node >=18; all three Linux lifecycles pass | Other native OS/CPU combinations | in-progress |
| JDT LS | Exact 1.62.0 milestone tarball, checksum, direct Java launcher, platform configuration, project-specific configuration/data; Linux lifecycle passes with external Java 25 | Other native OS/CPU combinations, Windows ARM64 configuration | in-progress |
| clangd | 23.1.0 Linux x86_64 ZIP install and lifecycle; Windows AMD64 PE and universal macOS x86_64/arm64 Mach-O downloads/layout/digests inspected | Native Windows/macOS execution; actual Linux/Windows ARM64 install adapter and runtime | in-progress |
| Common installer | Declarative dependency archives, combined bounds, prerequisite probes, cancellation, staged publication/rollback and configured command precedence; regression suite passes | Platform-specific archive/launcher coverage above | in-progress |
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
keeps mutable project data out of installed templates.

## Evidence

- [`U09-upstream.json`](../parity/U09-upstream.json): final fresh replay of six
  actual public downloads through production `HttpFetcher`, checksum/executable
  validation, LSP initialize, didOpen, shutdown and reaping on Linux x86_64.
- [`U09-assets.json`](../parity/U09-assets.json): verified archive sources/hashes,
  JDT bundle prerequisite, clangd architecture headers and Linux library needs.
  Download/header inspection does not qualify another native platform.
- [`U09-pty.json`](../parity/U09-pty.json): four real CLI cases at 100x34 and
  60x24. Actual HTTP 503 with a URL exceeding 6,000 characters opens full details;
  End reaches the tail and Ctrl+C copies the entire diagnostic into a file that
  is saved. An old Node prerequisite fails before HTTP or install-root creation.
  Failed installs retain no partial server or lock, and terminal modes restore.
- `cargo +stable test -p kode-lsp -p kode-tui`: 336 pass, five opt-in tests ignored
  (four external JDBC cases and the explicit upstream qualification replay).
- `cargo +stable clippy -p kode-lsp -p kode-tui --all-targets -- -D warnings` and
  formatting pass. The six-server upstream replay is run separately, explicitly.

Reproduce upstream qualification with fresh owned `KODE_TEST_LSP_ROOT`,
`KODE_TEST_NODE_BIN` and optional `KODE_TEST_JAVA_BIN` directories:

```sh
cargo +stable test -p kode-lsp --test upstream_install -- --include-ignored --nocapture
python3 scripts/test-lsp-install-terminal.py --binary /path/to/kode
```

## Remaining work

1. Implement and qualify practical ARM64 clangd inputs. Official LLVM 23 ARM64
   monolithic packages exceed the current install budget and need formats the
   installer does not yet support; adding a URL alone would not fix installation.
   Smaller official apt.llvm.org ARM64 packages were located, but require an
   archive adapter and an explicit native dependency/loader plan before use.
2. Exercise actual installs and lifecycle on Windows/macOS and remaining CPU
   combinations. The authorized Windows SSH host is reachable and reports Windows
   x86_64. The owned qualification project is
   `C:/Users/alfu64/Development/kode-rs-u09-20261016-01`; existing user projects are
   preserved. Isolated stable GNU Rust 1.99.1 is installed for U09 only; it does
   not qualify the Z03 MSVC artifact targets. Native Windows evidence is pending.
3. Finish U12 and all Uxx acceptance before resuming
   [Z03's full artifact matrix](Z03-ARTIFACT-MATRIX.md). Windows project location
   and permission remain as recorded there.

The terminal replay also exposed an initial-file-load focus race when Settings is
selected before loading finishes. This remains explicitly tracked under E13.
The U09 replay waits for the initial file to render and does not claim to fix that
race. No remote CI result, ARM64 runtime, Windows/macOS runtime or release artifact
success is implied by the Linux evidence.
