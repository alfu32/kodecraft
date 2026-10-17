# Integration and delivery tasks

## Z01 — Shell suspension and browser terminal serve mode

**Status:** done

**Depends:** W05, F04, F03. **Owns:** `kode/src/serve.rs`,
`kode-platform/src/shell.rs`, serve packaging metadata.
**Read:** Main's shell escape and serve argument construction, TtydAdapter.kt,
TtydAdapterTest, native resource layout and source README remote-mode instructions.

1. Implement Ctrl+T suspension using F04's input pause/mode restoration, launch
   user's shell with inherited terminal and workspace cwd, then resume/repaint.
2. Implement `kode serve [--port 11045] [folder] [-- <ttyd args>]`. Launch an
   installed or bundled ttyd executable around the Rust binary, preserving loopback
   default, explicit public binding, credentials, writable terminal and argv boundaries.
3. Package/build ttyd executables for actual supported release targets; replacing
   old JNA shared-library entrypoints is an implementation change, not removal of
   browser mode. Missing ttyd/platform support is reported explicitly.

**Accept:** shell exit/error/resize recovery, `serve` argument fixtures, spaces in
   paths, loopback browser terminal edit/save/mouse/clipboard/SSE/SQL behavior,
   helper child cleanup and SSH tunnel workflow. Document browser-reserved shortcuts
   and the existing Chrome app-window workaround. No new web IDE frontend.

## Z02 — Settings, help, about and operational status

**Status:** done

**Depends:** W05, L09, P01, D07, F05. **Owns:**
`kode-tui/src/panels/{settings,help,about}/`, `app/metrics.rs`.
**Read:** SettingsView, AboutView, HelpView, Main's PerfSnapshot and source styles.

1. Port settings controls for LSP catalog/install/start/stop and indexing state.
   Translate old internal H2 start/restart/clear controls to meaningful native
   index worker/rebuild controls; keep user JDBC connections separate.
2. Generate help from shared command bindings, covering editor/search/files/Git/
   SQL/REST/image/hex and scoped modifier conflicts. Preserve scrollable panels.
3. Implement status FPS/process memory/CPU, scan versus resolve/publish progress,
   build version, MIT license, Rust/Java/native-library credits and source links.

**Accept:** help matches actual actions, tiny-panel scrolling, status updates
   without full-frame busy loop, metrics work without Java, clear index affects
   only rebuildable Rust cache. Missing optional components have actionable status.

## Z03 — Reproducible build and distribution

**Status:** in-progress

**Depends:** Z01, Z02, D02, U01, U02, U03, U04, U05, U06, U07, U08, U10, U11, U13. **Owns:** `scripts/`, `.github/workflows/`, packaging
metadata, notices, install/run documentation.
**Continuation:** [qualified foundation](../Z03-INTEGRATION.md) and
[requested full artifact matrix / Windows test host](../Z03-ARTIFACT-MATRIX.md).
**Sequence:** the user accepted the MVP and restarted release work independently of the suspended U09 and U12 follow-ups.
**Progress:** archive, actual AppImage and strict Snap, relocated ttyd/helper,
full notices/provenance, clean Rust/native rebuilds, no-Java compilation and
terminal/browser/SSH acceptance are qualified on Linux x86_64 GNU. The manual
six-target release workflow and exact asset gate are being qualified remotely.
The user accepted the MVP; suspended LSP/database follow-ups are outside this
release gate. [Foundation evidence](../../parity/Z03.md).
**Read:** source build.gradle.kts distribution tasks, snapcraft.yaml, scripts,
LICENSE.md, native library licenses and source launcher behavior.

1. Build locked Rust release and separate Java helper JAR; pin Java/Maven library
   dependencies and include licenses/notices. Ordinary Rust development can compile
   without Java; JDBC tests/package step builds or uses the helper artifact.
2. Package executable + resources + helper JAR + optional ttyd executable. Retain
   shell/Windows/PowerShell launcher ergonomics and executable-relative resources.
   Runtime JDBC uses external Java by default. No full original Kotlin app shipped.
3. Build and qualify portable Linux/macOS/Windows distributions for both x86_64
   and arm64, plus architecture-specific Linux AppImages/Snaps, Windows MSI/MSIX
   and macOS DMGs. Use the source workflow as the artifact-family reference, and
   implement the [full matrix](../Z03-ARTIFACT-MATRIX.md) with checksums, manifests,
   native notices and coordinated CI/release assets. Use the authorized Windows
   SSH host/project directory for local testing; qualify each OS/CPU with actual
   build/install/runtime evidence.

**Accept:** install elsewhere, run from unrelated cwd, paths with spaces, clean
   configuration, no-Java core launch and external-Java JDBC launch. No hardcoded
   developer paths, build caches, credentials or Kotlin runtime in package.

## Z04 — Full functional parity and release qualification

**Status:** suspend

**Depends:** E13, L12, W05, V02, V03, G03, D09, R08, R10, Z03, U01, U02, U03, U04, U05, U06, U07, U08, U09, U10, U11, U12, U13.
**Owns:** `docs/parity/release.md`, full integration test orchestration.
**Read:** all ledgers, EDITOR-UX.md, COMPATIBILITY.md and original feature screenshots.

1. Run affected unit/integration suites and assembled workspace checks, Java bridge
   checks, catalog/fixture coverage, and deterministic TUI interaction replays.
2. Complete local terminal/SSH/tmux/browser/platform matrix; exercise all features
   in one workspace, then switch/restart with dirty files, active jobs and sessions.
3. Record responsiveness/memory/latency on named fixtures and hardware, dependency
   versions and release artifact results. Resolve every implemented-feature gap.

**Accept:** every task and editor UX row has evidence; legacy import and Postman
   round-trip complete; Git and JDBC writes have behavioral checks; SQL/REST and
   index jobs cannot freeze typing; terminal and children clean up on exit. Future
   proposals remain clearly labeled. Do not declare full migration complete with
   a placeholder screen, unsupported existing feature, or undocumented exception.
