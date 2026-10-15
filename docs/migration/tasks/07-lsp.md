# Optional LSP integration tasks

## P01 — Server catalog, discovery and installation

**Status:** done

**Depends:** F03, F05. **Owns:** `kode-lsp/src/{catalog,install,discovery}.rs`,
`kode-tui/src/panels/settings/lsp.rs`, `resources/lsp/servers.json`.
**Read:** `LspCatalog.kt`, `LspService.kt` command resolution, `SettingsView.kt`,
`lsp/servers.json`; SPEC.LSP.md only as historical context.

1. Port catalog/schema, preferred commands, language mapping, platform/architecture
   matching, configured paths, installed manifests and status.
2. Download/install/update/remove using reqwest/zip and hashes where supplied;
   support manual server entries and local executables. Handle partial installs
   atomically, executable bits, unpack subdirectories and path traversal bounds.
3. Port settings controls and error/progress display; separate installed from
   running and provide actual start/stop/restart callbacks to P02.

**Accept:** fixture catalogs for Linux/macOS/Windows and architecture aliases,
   missing/manual/checksum/error/cancel states, preserved install root and command
   argument arrays. Existing user-installed servers can be discovered without download.

## P02 — LSP client transport and lifecycle

**Status:** done

**Depends:** P01, E01, F03. **Owns:** `kode-lsp/src/{transport,client,documents}.rs`.
**Read:** LspClient.kt, LspService.kt; lsp-types docs and server fixture protocol.

1. Use lsp-types and Serde for message DTOs; implement byte-counted Content-Length
   framing over child pipes, IDs/pending responses, notifications, stderr drain and
   server-initiated request replies. Do not reuse JDBC length-prefix framing.
2. Initialize with root/workspace/capabilities; negotiate text position encoding and
   full/incremental synchronization. Send initialized/open/change/save/close in order.
3. Implement definitions/references/completion, timeouts/cancel, graceful shutdown/
   exit/kill/reap and process failure. Only enable additional requests if supported.

**Accept:** fake server fragmentation, UTF-8 Content-Length, out-of-order replies,
   error results, startup failure, restart, server requests and hanging child. Correct
   UTF-16 coordinates and revision gating; never block typing awaiting a reply.

## P03 — Editor enrichment and provider fallback

**Status:** in-progress

**Depends:** P02, L09, E09, E10. **Owns:** `kode-lsp/src/provider.rs`,
`kode-tui/src/app/intelligence.rs`.
**Read:** LspEditorIntelligence.kt, CompositeEditorIntelligenceService in
EditorIntelligence.kt, CodeEditorView LSP hooks.

1. Preserve actual provider precedence: current non-empty primary results win,
   otherwise native fallback. Do not assume the old spec's proposed merge behavior
   is what CompositeEditorIntelligenceService implements.
2. Attach document lifecycle and current definitions/references/completion to editor
   host services; make stale results and failures fall back cleanly.
3. Expose supported diagnostics/symbol/hover enrichment only where implemented and
   capability-checked. Rename/refactoring remains a separately labeled extension
   until implemented; a default false/empty interface is not feature parity evidence.

**Accept:** missing/offline/crashed LSP still gives native editor intelligence;
   correct current-version targets, fallback on empty/error, and no focus/selection
   theft. Fake server plus optional real rust-analyzer/clangd smoke with recorded version.
