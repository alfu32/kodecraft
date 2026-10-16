# U03 optional Java runtime integration

U03 implements the optional Java runtime for the qualified **Linux x86_64 GNU**
target. See [acceptance evidence](../parity/U03.md), the
[terminal report](../parity/U03-pty.json) and
[package separation report](../parity/U03-package.json).
Full distribution/publication work continues under Z03; no remote release upload
or untested platform success is claimed.

## Artifact and installer boundary

- `scripts/build-java-runtime.py` uses the user's `jlink-build/bundle.sh` module
  baseline plus common driver modules. It writes a standalone runtime ZIP, exact
  file manifest and SHA-256 sidecar. User jlink scripts/bundles are unchanged.
- `.github/workflows/java-runtime.yaml` has immutable action pins, a read-only
  Linux build/artifact job and a release-only publication job. It has not been
  executed remotely. The public latest-release endpoint returned HTTP 404 during
  qualification; failure is handled in the production UI.
- `Options::from_environment(storage_root(database_home))` selects the native
  target and `KODE_RUNTIME_RELEASE_URL` or the repository's latest-release URL.
- `Installer::run(cancel, progress)` executes on a blocking worker, using the
  existing Tokio handle for HTTP/process futures. Never invoke it from paint,
  input or an async runtime worker. Progress uses a coalesced watch channel.
- Metadata is resolved once per attempt; ZIP/checksum assets come from that
  capture. Redirects, sizes, paths, links, archive/file hashes and Java version
  are validated before atomic generation publication. Failure preserves the
  previous pointer; cancellation joins the owned probe child and removes staging.
- Configured/local Java takes precedence over managed discovery. The runtime
  reads its installed pointer only after a typed local-Java failure; construction
  and ordinary core startup remain inert.

## Production UI and ownership

- `pump_database` derives the offer only from the typed
  `BridgeError::requires_java_runtime()` flag. F5/Get Java is visible in the tree
  and connection form when applicable, and is listed in shared shortcut help.
- Opening the offer performs no download. Download captures a workspace/request,
  cancellation token and progress receiver. It rechecks Java before fetching any
  metadata; Java becoming available avoids the download.
- `Services::install_java_runtime` runs the installer on a blocking worker. The
  existing critical database scheduler job cancels and joins it during workspace
  disposal. Dialog completion requires its original request and generation.
- Cancel waits for completion; Retry starts a fresh request. Hide/Esc returns to
  the form/editor while work continues; F5 returns to progress. F4 opens U02's
  wrapped, copyable details. Draft fields and their focus survive installation.
- Success clears only the missing-Java hint and tells the user to retry
  Test/Connect explicitly. It never replays SQL or a database mutation.

## Distribution handoff to Z03

`scripts/package-linux.py` assembles the built native executable, resource files,
external JDBC helper, README and application license. Its manifest records every
payload file and direct ELF dependency. It never copies a runtime or a developer
configuration/driver directory. Two assemblies of the same inputs match. This is
archive assembly reproducibility, not a claim of independent Rust build
reproducibility.

The archive was extracted into a directory with spaces and launched from an
unrelated cwd: no-Java core/offer and actual adjacent-helper H2 SQL both passed
without fetching runtime metadata. Z03 still owns complete notices, native library
redistribution, ttyd integration, launchers/installers, CI release orchestration and
actual publication. Keep the separate JVM asset out of the main package.
