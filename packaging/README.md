# Linux distribution build and qualification

Z03 remains in progress. The verified target is **x86_64 Linux GNU**, with
**glibc 2.39 or newer** (Ubuntu 24.04 baseline). The application uses system glibc,
libgcc, libm and zlib. The optional ttyd executable has its own bundled json-c,
OpenSSL and zlib libraries, with paths relative to the executable. Its pinned
libwebsockets/libuv event loop is built in; no development prefix or global
`LD_LIBRARY_PATH` is needed. The package contains no JVM or original Kotlin app.

Java 21+ is needed only for JDBC. Configure local Java with `KODE_JAVA`, or use
F5/Get Java after an explicit database action reports it unavailable. The JVM
artifact is generated and downloaded separately. HTTPS actions need the operating
system's CA trust store; missing TLS setup does not prevent core editor startup.
The helper and resource files are discovered beside the native executable.

## Build

Use stable Rust/Cargo, Python 3.11+, a Java 21+ JDK, Maven, CMake, a C compiler,
patchelf, binutils and native development headers for libuv, json-c, OpenSSL and
zlib. On the qualified Ubuntu baseline the development package names are
`libuv1-dev libjson-c-dev libssl-dev zlib1g-dev`. A libcap development package is
optional. `scripts/build-linux-native.py` builds its own checksummed libwebsockets
4.3.3; do not install or bundle the old shared JNI launcher.

```sh
python3 scripts/build-release.py
bash scripts/build-jdbc-helper.sh
python3 scripts/distribution_notices.py
python3 scripts/build-linux-native.py
python3 scripts/native_bundle.py --ttyd-bundle target/ttyd-release-bundle --output target/native-release-bundle
python3 scripts/package-linux.py --native-bundle target/native-release-bundle --output target/distribution-z03
```

For development packages extracted into a local `usr` tree, pass that tree as
`--dependency-prefix` to `build-linux-native.py`. This avoids changing the operating
system. Source hashes and versions are pinned in the native build scripts. The
builder corrects the pinned libwebsockets CMake config's unconditional shared-target
listing for its static-only build and prevents imported include paths from losing
priority to a different system libwebsockets configuration.

The Rust build remaps developer paths for Rust and C/C++ diagnostics. All 30 Java
runtime dependency versions are fixed in `jdbc-bridge/pom.xml`; the artifact/POM
hashes are locked in `packaging/java-dependencies.json`. Build-time Maven plugins
and the helper output timestamp are pinned too. The helper was rebuilt with
identical output. Independent Rust build reproducibility remains unqualified.

The notice collector verifies locked Cargo archive hashes and includes full legal
texts for every normal/build Linux dependency and every original shaded runtime
JAR. It retains upstream copyright/developer declarations, inherited POMs and Java
source archive URLs. Files omitted upstream are checked in under
`packaging/licenses/` with pinned provenance/hashes. Native and ttyd browser
component notices are also shipped; `notices/manifest.json`, `native-manifest.json`
and `notices/browser/manifest.json` describe their scope.

The assembler rejects changed notice inputs, missing files, duplicate payload
paths, mismatched native manifests and developer source paths in native binaries.
It emits a `.tar.gz`, exact file/mode/size/SHA-256 manifest and checksum sidecar.
With identical inputs and `SOURCE_DATE_EPOCH`, archive assemblies match. All
extraction occurs during qualification in owned temporary directories.

## Run and qualify

Extract the archive and run `kode` or `kode.sh` from the extracted directory.
`kode.ps1` is also exercised with PowerShell on Linux. Windows launcher sources are
retained, but Windows binary/launcher execution is unqualified and no Windows
artifact is produced. No configuration or credentials are shipped in the archive.

Repeat assembly with `--output target/distribution-z03-repro`, then run:

```sh
python3 scripts/test-linux-distribution.py --archive target/distribution-z03/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz --repro-archive target/distribution-z03-repro/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz
python3 scripts/test-serve-browser.py --package /path/to/extracted/package --ssh
```

These tests need Python Playwright, Chrome, Docker, local Java and (for SSH)
OpenSSH. They use synthetic credentials and isolated workspaces. The container
fixture runs as the calling uid in a read-only pinned Ubuntu image, with owned
writable workspace/tmp mounts and loopback host networking. It edits/saves through
Chrome without Java or a CA store, then checks child cleanup. Docker bridge/NAT
routing was unavailable in the local qualification environment; that network mode
is not claimed by this fixture. It does not change host network configuration.

The packaged browser fixture uses the adjacent ttyd and helper, with no developer
library or helper override. It checks SSH, all eight panel shortcuts, editing,
clipboard, streaming REST and real H2 SQL with disconnect persistence/reaping.

## Remaining Z03 gates

Linux AppImage/Snap installers, complete CI/release orchestration and actual
publication remain required. The optional-runtime workflow has not run remotely;
its published-release upload timing must be coordinated with the final release
lifecycle. macOS, Windows and other architectures remain unqualified. The full
migration and final integration matrix are still tracked in `DASHBOARD.md` and Z04.
