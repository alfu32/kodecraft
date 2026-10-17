# Linux distribution build and qualification

Z03 is reopened for the requested [Linux/macOS/Windows x86_64/arm64 artifact matrix](../docs/migration/Z03-ARTIFACT-MATRIX.md), after the Uxx series. This guide documents
the qualified Linux x86_64 foundation; it does not claim the expanded matrix is done.

Z03 build/distribution qualification is complete for **x86_64 Linux GNU**, with
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
identical output. A fresh Cargo/native build in separate directories reproduced
the archive byte-for-byte on the same host/toolchain; cross-toolchain equality is
not claimed. `build-release.py --target-dir /owned/path/cargo` and
`build-linux-native.py --build-root /owned/path/native` isolate these builds from
an active developer build. Release x86_64 code uses the portable x86-64 CPU target.

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

## AppImage and Snap

Build actual installers from the verified main archive, without a bundled JVM:

```sh
python3 scripts/package-appimage.py --archive target/distribution-z03/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz
python3 scripts/package-snap.py --archive target/distribution-z03/kode-0.1.0-x86_64-unknown-linux-gnu.tar.gz
```

AppImage needs `desktop-file-validate` and its downloaded checksummed
appimagetool/runtime inputs, pinned in `appimage-inputs.json`. Runtime legal texts
for its static dependencies are included. Snap needs Snapcraft; `--packer snap`
uses `snap pack` without a running host snapd service. Both packers produced
identical bytes locally. Both formats have checksum and payload manifest sidecars.
Repeat assembly with the same inputs and epoch to reproduce the installer bytes.

To launch an AppImage, make it executable and pass ordinary Kode arguments:

```sh
chmod +x kode-0.1.0-x86_64.AppImage
./kode-0.1.0-x86_64.AppImage /path/to/project
APPIMAGE_EXTRACT_AND_RUN=1 ./kode-0.1.0-x86_64.AppImage /path/to/project
```

The first launch uses FUSE and requires access to `/dev/fuse` and the host FUSE
mount helper. The second uses the runtime's extraction mode when FUSE is
unavailable. Both use the packaged helper and ttyd next to the actual executable;
they retain the glibc 2.39 minimum. No root installation is required.

The development Snap is named **kode-rs**, uses **strict** confinement and core24,
and has grade `devel`. It can coexist with the original `kode` Snap. Installing
this local, checksummed artifact requires a working host snapd:

```sh
sudo snap install --dangerous ./kode-rs_0.1.0_amd64.snap
kode-rs /path/to/project
```

`--dangerous` permits a local unsigned artifact; confinement remains strict.
The home/network interfaces support editing and browser mode; removable-media
access requires the normal explicit Snap interface connection. Configuration,
database, drivers and LSP data live under `~/snap/kode-rs/common`. A configured
JVM must be executable within confinement, for example an external runtime under
that directory. A host JDK outside the Snap's visible filesystem is not assumed
accessible. The separate Get Java installation remains outside the immutable Snap.

Qualification commands:

```sh
python3 scripts/test-appimage-terminal.py --appimage target/installers/kode-0.1.0-x86_64.AppImage
python3 scripts/test-serve-browser.py --package /path/to/extracted/package --appimage target/installers/kode-0.1.0-x86_64.AppImage --ssh
python3 scripts/prepare-snap-guest.py
python3 scripts/test-snap-guest.py --snap target/installers/kode-rs_0.1.0_amd64.snap --runtime target/java-runtime-artifacts/kode-jdbc-runtime-x86_64-unknown-linux-gnu.zip
python3 scripts/test-rust-without-java.py
```

Snap acceptance needs KVM access, QEMU, xorriso, OpenSSH and Python Playwright/
Chrome. It downloads a dated, checksummed Ubuntu image and pinned core24 base,
then installs only inside an owned temporary VM. Canonical's signed assertions
are verified inside the guest. No host Snap installation or existing `kode`
installation is changed. The VM, SSH keys, ports and overlay are cleaned up.

## Manual release workflow

The single [Release workflow](../.github/workflows/distribution.yaml) runs only
through GitHub Actions **Run workflow**. Select the branch in GitHub, or enter a
branch/tag as `source_ref`; the CLI equivalent is `gh workflow run distribution.yaml
--ref work -f source_ref=0.2.6`. The chosen source must have the same version in
`Cargo.toml` and `jdbc-bridge/pom.xml`. An existing matching `<version>` tag must
identify that source commit. If the branch has no matching tag, the workflow
creates one only after the full matrix passes.

The workflow runs locked Rust tests, Clippy, Maven tests, six native builds, two
portable archives per target, Linux AppImage/Snap on both architectures,
Windows MSI/MSIX, macOS DMG, a separately downloadable JDBC adapter JAR and
optional Java runtimes for all six targets. It verifies every asset and checksum,
then publishes a GitHub Release only if all jobs succeed. About and the package
manifests use the Cargo version, and release builds embed the source revision.
Windows installers and DMGs are currently unsigned; the release notes state this.

`build-provenance.py --output /path/to/assets/build-source.json` records exact
source/dirty state, compilers, native package versions/hashes and static libuv.
It expects the normal build directories unless overridden. For a network-free
review, collect the main archive and its sidecars, AppImage/Snap and their sidecars,
the separate Java runtime ZIP/manifest/checksum and `build-source.json` under an
input directory, then run:

```sh
python3 scripts/release-assets.py --directory /path/to/incoming --output /new/empty/review --tag 0.1.0 --source-revision FULL_GIT_COMMIT
```

The review rejects dirty/stale source, a mismatched version, altered assets,
missing runtime, duplicate names and nonempty output. It emits an exact review
manifest, checksums and release notes without accessing GitHub.

The older Linux-only draft review scripts remain for reproducing the recorded
Z03 foundation. The new workflow's release gate is `scripts/verify-release.py`.
Current status and acceptance evidence are tracked in `DASHBOARD.md` and Z03.
