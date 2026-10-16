# U09 language-server installation repair

Status: in-progress. Source audit complete; no new upstream artifact, installer
or language-server runtime qualification is claimed yet. U07 is complete.

The current local catalog and installer explain the reported dead ends:

| Surface | Source-backed finding | Required work | Status |
|---|---|---|---|
| JDT LS | `resources/lsp/servers.json` pins 1.33.0 with a timestamped tarball URL and a Windows ZIP entry, without checksums | Verify official release artifacts, hashes, platform launcher/configuration and Java prerequisites; replace broken URLs and initialize the installed server | todo |
| TypeScript | 5.1.3 / TypeScript 5.9.3 appear only in an `npm install -g` instruction | Provide an isolated executable install path with pinned dependencies and Node prerequisite checks, preserving local overrides | todo |
| VSCode HTML/CSS/JSON | Two catalog entries provide only 4.10.0 npm instructions; the combined html/css/json entry prefers the HTML executable | Install the required package in isolation and resolve the correct executable for each language, including JSON | todo |
| clangd | 21.1.0 has manual LLVM/package-manager instructions without a compatible download entry | Verify actual official platform assets, hashes and native library requirements; install and discover its executable | todo |
| Common installer | `InstallJob::run` needs `entry.download(platform)`; otherwise it returns “No download” plus the manual instruction JSON | Extend the existing bounded, cancellable, staged/rollback-safe workflow for the verified package forms | todo |
| Settings errors | Fixed-height cards render status/message on single clipped lines | Add wrapping/scrolling or owned details with actionable HTTP/package/prerequisite/launch failures | todo |

Existing foundations to retain: bounded HTTP fetch/extraction, archive checks and
staged command resolution, per-root installation lock, retained prior installation
and atomic publication, explicit user install/start actions, configured local
executable overrides, tracked process lifecycle and workspace ownership. Keep
downloads/installations out of input/render and normal editor startup.

Next steps:

1. Verify current official upstream metadata and usable release/package inputs;
   record versions, checksums/integrity, executable layouts and prerequisites.
   The versions above describe the current repository and are not an assertion
   about current upstream versions.
2. Implement the smallest explicit install adapters and command discovery needed
   for those artifacts. Preserve cancellation, isolation, rollback and overrides;
   never equate a manual instruction with a successful installation.
3. Repair Settings diagnostics and test failed HTTP/package/dependency cases
   without leaving a partial installation or blocking editing.
4. Install each reported server into owned temporary roots and exercise actual
   LSP initialize/shutdown, including all distinct HTML/CSS/JSON launchers. Record
   which platform/CPU each artifact and runtime result qualifies. Cross-compilation
   or catalog rows do not qualify another runtime platform.

The current Linux host has Node 20.19.3, npm 10.8.2 and Java 21.0.9; clangd is
not on the inspected PATH. These are observations for local qualification,
not the servers' verified minimum requirements.

After U09, finish U12 and the remaining Uxx acceptance before resuming
[Z03's full artifact matrix](Z03-ARTIFACT-MATRIX.md). The authorized Windows SSH
host and `/Users/alfu64/Development/` project location remain recorded there.
