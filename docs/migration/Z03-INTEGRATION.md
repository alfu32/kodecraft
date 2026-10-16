# Z03 distribution continuation

Z03 is **done** for Linux x86_64 GNU. The archive, AppImage, strict Snap, notices,
relocated browser terminal and local acceptance are qualified. The full migration
goal remains active; Z04 owns final release qualification after remaining UAT work.

## Implemented and qualified

- `python3 scripts/build-release.py` builds the locked stable native executable
  and remaps developer paths in Rust and C/C++ diagnostics. The assembler rejects
  native inputs still containing the builder's home path. Only Linux x86_64 GNU
  is qualified, with **glibc 2.39+ / Ubuntu 24.04** as the tested baseline.
- `bash scripts/build-jdbc-helper.sh` builds/tests the Java 21 helper. All **30
  runtime versions** are fixed in dependency management, with JAR/POM hashes in
  `packaging/java-dependencies.json`; the dependency-list plugin, other plugins
  and output timestamp are pinned. All **64 Java tests** pass. Two helper builds
  match: [helper evidence](../parity/Z03-helper-build.json).
- `python3 scripts/distribution_notices.py` reads exact locked Cargo archives,
  verifies their hashes, and collects legal texts for **405 Linux normal/build
  dependencies** and **30 original runtime JARs**, including their individual
  notices, inherited POM declarations and Java source archive URLs. Upstream texts
  omitted from archives are checked in with source revision/URL/hash provenance.
  A dual-licensed crate with no full texts retains its declarations/authors and
  selects the declared Apache-2.0 alternative; no copyright holder is invented.
- `python3 scripts/build-linux-native.py` builds checksummed libwebsockets 4.3.3
  with **built-in libuv** and the pinned standalone ttyd source. Supply
  `--dependency-prefix /path/to/extracted/usr` for development packages extracted
  without OS installation. Never modify the frozen sibling source repositories.
  The source CMake config's unconditional shared-target list is corrected for the
  static-only build; imported header priority is retained to avoid ABI mismatch
  with another installed libwebsockets configuration. No JNI launcher is used.
- `python3 scripts/native_bundle.py --ttyd-bundle target/ttyd-release-bundle --output target/native-release-bundle`
  copies the native runtime closure, patches relative RPATHs and includes static/
  shared component legal texts, standard license texts, and notices for **18
  pinned ttyd browser dependencies**, including the existing zmodem patch.
  The bundle has **5 ELF files**. No global `LD_LIBRARY_PATH` is needed or passed
  to child editors/JVMs. The older system event-plugin configuration is rejected.
- `python3 scripts/package-linux.py --native-bundle target/native-release-bundle --output target/distribution-z03`
  emits the actual main archive, payload hashes/modes/sizes and ELF dependency
  manifest. It includes shell, PowerShell and Windows launcher sources. Shell and
  Linux PowerShell argument/exit behavior pass; Windows execution is unqualified.
  The JVM, Kotlin app, developer configuration and driver/build caches are absent.
- The current archive has **984 files**, **32,813,172 compressed bytes**, SHA-256
  `e16bc9d3668e9de49285b5da05f68a47d1eebd039b7fc173bf38c880b1653f59`.
  Fresh Rust/native builds in separate directories reproduce this hash on the
  same host/toolchain. This does not claim cross-toolchain or cross-OS equality.
  See [rebuild evidence](../parity/Z03-rebuild.json) and
  [source/compiler/native provenance](../parity/Z03-build-source.json).
- [Actual AppImage and strict Snap artifacts](../parity/Z03-installers.json)
  rebuild identically; Snapcraft and service-free `snap pack` produce identical
  Snap bytes. Runtime/tool inputs and their static native dependency legal texts
  are pinned for AppImage. Neither installer contains a JVM.
- [Four actual AppImage PTY cases](../parity/Z03-appimage-pty.json) and
  [five AppImage browser/SSH cases](../parity/Z03-appimage-browser.json) pass.
- [Three actual strict Snap cases](../parity/Z03-snap-vm.json) run inside an owned
  Ubuntu KVM guest: no-Java editing, external-Java H2 SQL, and browser editing/save
  over SSH, with child/helper cleanup. Host Snap installations remain untouched.
- [All Rust workspace targets compile without Java/Maven](../parity/Z03-no-java-build.json)
  in an owned clean pinned container, using read-only source/registry mounts.
- CI orchestrates stable/locked Rust checks, helper/JDBC tests, notice/native
  builds, installer reproducibility and actual AppImage/Snap/no-Java acceptance.
  The optional JVM is built separately and attached with the full asset set to a
  new draft, before publication. Published-release append/upload is removed.
  Thirteen [preflight/CLI integrity cases](../parity/Z03-release-preflight.json)
  pass using actual artifacts and a mocked GitHub CLI; no real upload is claimed.

## Acceptance and discovered startup fix

- [Four archive/OS cases](../parity/Z03-archive.json) verify all actual files,
  modes, legal manifests and absent developer source paths; launch from paths
  with spaces/unrelated cwd through shell/PowerShell; actual no-Java PTY core and
  real packaged-helper H2 SQL; and browser editing/save/disconnect in a clean,
  read-only pinned Ubuntu image running as the calling uid. It has **no Java and
  no CA bundle**. The gateway loads its packaged libraries and reaps the child.
- The clean-image test found eager LSP HTTP/TLS client construction aborting the
  core editor when the system had no CA store. `HttpFetcher::new` now captures
  only the scheduler handle; client construction occurs on explicit download on
  its blocking worker. Failures are not cached, enabling retry after TLS setup is
  repaired. TLS verification is retained. Seven LSP installer tests and 19 real
  application-runtime tests pass; affected all-target clippy and format pass.
- [Five packaged browser/SSH cases](../parity/Z03-browser.json) use the adjacent
  ttyd/helper and no developer library path. They exercise all eight panel keys,
  file editing/save, actual browser clipboard, reconnect, streamed REST cancellation,
  real H2 SQL and dirty buffer persistence/child/helper reaping. The package
  manifest hash matches the archive qualification. Resize selection now waits
  and selects the rendered xterm row atomically, avoiding a fixture race.
- [Three integrity failures](../parity/Z03-integrity.json) corrupt a full license,
  native library and helper input in owned copies. Each attempt fails for the
  expected integrity reason and emits no archive.
- The container uses loopback host networking. Docker bridge/NAT traffic could not
  reach its confirmed listening socket locally; no bridge-network success or host
  network configuration change is claimed.
- Optional Java remains separate under [U03](../parity/U03.md). No remote Actions
  run, release upload or publication is inferred. Installer support is limited to
  the actual locally qualified Linux x86_64 artifacts described above.

See [the distribution guide](../../packaging/README.md) for prerequisites and exact
commands. Browser tests require Python Playwright, Chrome and (for SSH) OpenSSH;
archive clean-image tests require Docker and local Java for the external-helper
case. All fixtures use owned directories and synthetic credentials.

## Original platform inventory

The frozen Kotlin adapter recognizes Linux/macOS/Windows x86_64 and aarch64.
`file` confirms matching ELF, Mach-O and PE architectures for all six embedded
shared-library resource paths. The Gradle download task names only macOS aarch64,
Windows x86_64 and Linux x86_64 assets; the Snap metadata declares amd64 and arm64.
These historical shared resources/declarations are **not evidence of separate
native Rust release artifacts**. Current qualification covers only Linux x86_64
GNU. Preserve the source's other platform branches as unqualified; do not claim
that they work or that the former library resources were standalone releases.

## Final release boundaries

Remote Actions execution and actual publication have not happened in this
workspace. Workflows pass local actionlint 1.7.7 validation (shellcheck unavailable
and disabled); Python compilation, credits/task checks and local artifact cases
pass. The local build provenance correctly says `sourceDirty: true`, so these
uncommitted-source assets are rejected for draft publication. Synthetic clean
metadata exists only inside temporary preflight test copies.

Z04 retains the final full parity/platform matrix and release execution after all
editor and UAT tasks pass. Windows launcher execution and macOS/Windows/other
architecture artifacts remain unqualified; no release artifacts are inferred.
