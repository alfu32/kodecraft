# Z03 requested release artifact matrix

The user expanded the distribution requirement on 2026-10-16. Finish U01–U12
before resuming Z03. The Kotlin reference is
`../kode/.github/workflows/release.yml` at
`7d3e5953ec2de75f2b5e9c44fb3e65549d39ec68`: Linux portable tar/ZIP and AppImage,
Windows portable ZIP/MSI/MSIX, and macOS portable tar archives. The Rust release
must implement the explicit architecture matrix below, using native Rust builds.
The reference's JVM/jpackage build is not the Rust packaging implementation.

| OS | CPU | Native target | Required application artifacts | Current evidence |
|---|---|---|---|---|
| Linux | x86_64 | `x86_64-unknown-linux-gnu` | Portable `.tar.gz` and `.zip`, AppImage, Snap `amd64` | Tar/AppImage/Snap foundation qualified locally; ZIP and post-Uxx rebuild pending |
| Linux | arm64 | `aarch64-unknown-linux-gnu` | Portable `.tar.gz` and `.zip`, architecture-specific AppImage, Snap `arm64` | Pending native build/runtime qualification |
| Windows | x86_64 | `x86_64-pc-windows-msvc` | Portable `.zip`, `.msi`, `.msix` | Pending native build/install/runtime qualification |
| Windows | arm64 | `aarch64-pc-windows-msvc` | Portable `.zip`, `.msi`, `.msix` | Pending native build/install/runtime qualification |
| macOS | x86_64 | `x86_64-apple-darwin` | Portable `.tar.gz`/`.zip`, `.dmg` | Pending native build/install/runtime qualification |
| macOS | arm64 | `aarch64-apple-darwin` | Portable `.tar.gz`/`.zip`, `.dmg` | Pending native build/install/runtime qualification |

DMG is included in the implementation plan following the user's suggestion.
AppImages must use matching architecture runtime/tool inputs. A universal macOS
artifact may replace separate DMGs only if both CPU slices are built and tested;
it does not remove either architecture from qualification.

Each target needs its actual native executable, resources, external JDBC helper,
standalone browser-terminal executable and required native libraries/notices.
Keep the JVM out of every main application archive/installer. Build a separate
matching optional Java runtime asset with checksum/manifest for each qualified
target; the architecture-independent helper must continue to find external Java.
Preserve native launchers, working-directory/argument semantics and optional
component discovery after installation/relocation.

All artifacts need checksums, inventories, native/compiler/source provenance,
licenses, and actual install/run evidence on their OS/CPU. Cover clean configuration,
paths with spaces, unrelated cwd, no-Java core, external-Java JDBC, browser mode,
upgrade/uninstall behavior for installers, and child cleanup. A successful cross
compile or an artifact filename alone does not prove runtime support. Record
signing/notarization and trust requirements with the actual installer results.
Z04 retains the final assembled release audit and publication.

The existing Linux draft adapter validates a fixed four-primary-artifact set.
Replace that assumption with the complete architecture-aware manifest and exact
asset validation. Keep one coordinated draft upload with the separately built JVM
assets, immutable action/input pins, and rejection of dirty/mismatched source or
existing releases. Do not append assets after publication.

## Authorized Windows local testing

Use SSH `alfu64@192.168.234.128`. Keep the project under the user-provided
`/Users/alfu64/Development/` directory, with an owned `kode-rs` checkout/build
directory beneath it. Inspect the existing folder/checkouts before writing and
preserve unrelated files. Discover the actual OS, CPU, shell and available build/
installer tools when this stage begins. Record which CPU/platform the machine
actually qualifies; CI or another matching host must qualify the other CPU.

No connection, remote modification, installer run, remote CI job or new platform
success is claimed by this planning document. The current Uxx implementation
continues first, as requested.
