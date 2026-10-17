# U09 language-server installation repair

Status: in-progress. Isolated upstream installs and LSP lifecycle pass on Linux
x86_64 and Windows x86_64 for all six reported launchers. Windows also passes
33 applicable regression tests and native Clippy. The Linux ARM64 clangd catalog
closure installs and runs under QEMU; native ARM64 and other platforms remain
open, so U09 is not complete.

| Surface | Implemented and verified | Remaining qualification | Status |
|---|---|---|---|
| TypeScript | Server 6.0.2 with TypeScript 6.0.3, pinned archives, isolated dependency tree, Node >=22.22.2; Linux/Windows x86_64 initialize/didOpen/shutdown/reap | Other native OS/CPU combinations | in-progress |
| VSCode HTML/CSS/JSON | 4.10.0, separate language entries/launchers, complete 32-package closure, Node >=18; all three Linux/Windows x86_64 lifecycles pass | Other native OS/CPU combinations | in-progress |
| JDT LS | Exact 1.62.0 milestone tarball, checksum, direct Java launcher, platform configuration, project-specific configuration/data; fresh Linux/Windows x86_64 installs and lifecycle pass with external Java 25 | Other native OS/CPU combinations, Windows ARM64 configuration | in-progress |
| clangd | 23.1.0 Linux/Windows x86_64 ZIP installs and lifecycle; pinned Linux ARM64 catalog closure installs, initializes, parses bundled headers, resolves definitions and shuts down under QEMU; universal macOS Mach-O assets inspected | Native Linux ARM64/macOS execution; Windows ARM64 package path and lifecycle | in-progress |
| Common installer | Declarative dependency archives, bounded Debian ar/GZIP/Zstandard import, selected file trees, declared regular-file copies, native library probes, cancellation, staged publication/rollback and configured command precedence; regression suite passes | Platform-specific closure/launcher coverage above | in-progress |
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
Java/Equinox argument handling. The URL change passes real Linux and native
Windows installed JDT replays and a separate fresh Windows installation.

## Evidence

- [`U09-upstream.json`](../parity/U09-upstream.json): final fresh replay of six
  actual public downloads through production `HttpFetcher`, checksum/executable
  validation, LSP initialize, didOpen, shutdown and reaping on Linux x86_64, using
  production `LspService` defaults (232.49 seconds for all six after the Debian/TAR
  refactor). This fresh run includes the JDT configuration/data URL change.
- [`U09-assets.json`](../parity/U09-assets.json): verified archive sources/hashes,
  JDT bundle prerequisite, clangd architecture headers and Linux library needs.
  Download/header inspection does not qualify another native platform.
  The official clangd 23.1.0 release has no standalone Windows ARM64 asset. The
  LLVM ARM64 Windows tar.zst is 424,902,634 bytes and the WOA64 MSI is 764,403,867
  bytes; both exceed the installer's 256 MiB download bound, and MSI is not an
  accepted input format. This narrows the unresolved path to a bounded official
  package extraction strategy or another acceptable standalone source; it does
  not count as a runtime qualification. See the recorded [Windows ARM64 asset
  inventory](../parity/U09-assets.json#L504) and the [official clangd
  23.1.0 release](https://github.com/clangd/clangd/releases/tag/23.1.0).
- [`U09-pty.json`](../parity/U09-pty.json): four real CLI cases at 100x34 and
  60x24. Actual HTTP 503 with a URL exceeding 6,000 characters opens full details;
  End reaches the tail and Ctrl+C copies the entire diagnostic into a file that
  is saved. An old Node prerequisite fails before HTTP or install-root creation.
  Failed installs retain no partial server or lock, and terminal modes restore.
- `cargo +stable test -p kode-lsp -p kode-tui -- --test-threads=1`, freshly rerun
  on 2026-10-17: 349 pass, seven opt-in tests ignored
  (four external JDBC cases, the six-server upstream replay and the Debian archive
  qualification, plus ARM64 emulation). New cases cover Debian formats/corruption/window bounds,
  selected links/traversal, independent dependency selection, manifest size,
  safe regular-file copies and prepublication/prelaunch native library checks.
- [`U09-deb.json`](../parity/U09-deb.json): production `HttpFetcher` and `InstallJob`
  download, verify and extract the official ARM64 clangd Debian archive on Linux
  x86_64. The actual ARM64 ELF, canonical executable and license are retained;
  unselected aliases/control payloads are absent. This qualifies the archive
  adapter and layout, not ARM64 execution. LLVM/Clang library and resource archive
  hashes/layouts and ELF dependency names were also verified for the next step.
- [`U09-arm64.json`](../parity/U09-arm64.json): a fresh install selected from the
  production Linux ARM64 clangd catalog entry uses 19 pinned archives
  (72,555,470 download bytes),
  including matching LLVM/Clang libraries, headers, private glibc and its runtime
  dependencies. All 19 ELF mappings resolve inside the install. Under private
  QEMU, production lifecycle defaults pass initialize, didOpen, diagnostic-free
  parsing, a `size_t` definition into bundled headers, shutdown and reaping.
  Publication and launch both check the private library closure. Resource paths
  become absolute only for launch; the manifest stays relocatable. Install roots
  include spaces, Unicode and a colon. This is emulation evidence, not native
  hardware qualification. The catalog selection is pinned to the exact dated
  upstream build and does not label it as the stable 23.1.0 GitHub artifact.
- [`U09-windows.json`](../parity/U09-windows.json): 33 applicable native Windows
  regression tests pass. Fresh upstream TypeScript/HTML/CSS/JSON installs pass
  production lifecycle defaults with native process-exit checks. JDT fails on its
  cache path in the earlier run; the file URL repair now passes a native installed
  replay in 3.7 seconds. An initial repaired JDT startup timed out at 10 seconds;
  its subsequent replay keeps the same deadline, and its cause is not established.
  A separate fresh six-server run passes all installs, initialize, didOpen,
  shutdown and reaping in 62.07 seconds. JDT initializes in 3.4 seconds and clangd
  in 98 ms. An earlier HTML timeout passed on both installed and fresh replays
  after the host restart, with the original deadline unchanged; its cause is not
  established.
- `cargo +stable clippy -p kode-lsp -p kode-tui --all-targets -- -D warnings` and
  formatting pass on stable Rust 1.99.1. Package/prerequisite/override/JDT and Debian
  archive tests pass. Locked dependency credits include the two new MIT decoder/
  hash crates. The upstream replay is run
  separately, explicitly.
- Native Windows `cargo +stable clippy --locked -p kode-lsp --all-targets -- -D warnings`
  passes with the isolated stable GNU toolchain. An earlier SSH interruption
  stopped compilation; a reconnected run passes on unchanged source. Evidence and
  log hashes are retained in the Windows report.

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
The Linux ARM64 catalog closure has a separate explicit emulation run; supply a
fresh `KODE_TEST_ARM64_ROOT` and a private `KODE_TEST_QEMU_AARCH64` executable:

```sh
cargo +stable test -p kode-lsp --test arm64_upstream production_catalog_arm64_clangd_installs_resolves_libraries_and_runs_lsp_under_emulation -- --include-ignored --exact --test-threads=1
```

That run proves catalog selection and emulated lifecycle behavior. It does not
replace native ARM64 qualification.

## Remaining work

1. Complete native qualification of the Linux ARM64 clangd catalog entry. Official LLVM 23 ARM64
   monolithic packages exceed the current install budget and need formats the
   installer does not yet support; adding a URL alone would not fix installation.
   The smaller official apt.llvm.org package now passes the bounded Debian adapter
   and selected-file qualification. Its isolated library/resource closure, bundled
   loader, prerequisite checks and actual ARM64 LSP execution now pass under QEMU.
   The production catalog selects the exact dated upstream build and preserves
   23.1.0 assets for other platforms. Native ARM64 hardware qualification remains
   necessary before claiming native support.
2. Extend actual native installs and lifecycle qualification to macOS and remaining
   Windows/Linux CPU combinations. The authorized Windows SSH host is connected and reports Windows
   x86_64. Guarded source overlays apply only after validating all previous hashes,
   with backups retained. All six fresh upstream installs/lifecycles, the existing
   JDT replay and 33 regressions pass.
   The owned qualification project is
   `C:/Users/alfu64/Development/kode-rs-u09-20261016-01`; existing user projects are
   preserved. Isolated stable GNU Rust 1.99.1 is installed for U09 only; it does
   not qualify the Z03 MSVC artifact targets. Native LSP Clippy also passes.
   The fresh six-server run uses a separate `upstream-03` root,
   preserving earlier installs, partial-run logs and diagnostic evidence.
3. Finish U12 and all Uxx acceptance before resuming
   [Z03's full artifact matrix](Z03-ARTIFACT-MATRIX.md). Windows project location
   and permission remain as recorded there.

The terminal replay exposed an initial-file-load focus race when Settings was
selected before loading finished. E13 now fixes the focus transfer for delayed
text, image and hex opens and verifies all three with an application-runtime test.
The U09 replay still waits for the initial file to render and is not that fix's
evidence. No remote CI result, native ARM64/macOS runtime or release artifact
success is implied by the Linux and partial Windows evidence.
