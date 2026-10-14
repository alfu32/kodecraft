# Crate and library decisions

Research date: **2026-10-14**. Links below are primary documentation. “Selected”
means selected for the plan after checking its documented role. It does **not**
mean compiled, benchmarked, or verified against Kode fixtures yet. Each owning task
must perform that last step and record exact versions, features, MSRV, licenses,
and platform requirements. Commit the resulting `Cargo.lock`.

## Selected Rust building blocks

| Function | Selection and evidence | Kode-specific work / limitation | Owner |
|---|---|---|---|
| TUI rendering/layout | [ratatui](https://ratatui.rs/installation/feature-flags/), documented 0.30.2; [custom widgets](https://ratatui.rs/recipes/widgets/custom/) | Render editor cells through custom widgets. Kode still owns layout maps, selection, focus, and hit testing. | F04/F05/E05 |
| Terminal input and modes | [crossterm](https://docs.rs/crossterm/latest/crossterm/event/index.html), documented 0.29.0 | Key/mouse/resize, bracketed paste, focus events; normalize platform variants. Use one input reader. Match Ratatui's Crossterm version. | F04 |
| Text storage | [ropey](https://docs.rs/ropey/latest/ropey/), documented 1.6.1 | UTF-8 rope with scalar-index operations; wrap byte/scalar conversions. Cursor state, history, search, and persisted snapshots are application logic. | E01–E04 |
| Unicode layout | [unicode-width](https://docs.rs/unicode-width/latest/unicode_width/), [unicode-segmentation](https://docs.rs/unicode-segmentation/latest/unicode_segmentation/) | Width and grapheme/word boundaries are different concepts. Preserve Kode's default escaped glyph display and explicit word rules. Do not change movement semantics merely by adopting grapheme helpers. | E01/E02/E05 |
| Text decoding | [encoding_rs](https://docs.rs/encoding_rs/latest/encoding_rs/) where non-UTF-8 decoding is needed | Explicit UTF-8/BOM policy first; byte-preserving save and invalid-decoding behavior must be specified. Never silently turn binary into lossy text. | E04/V01 |
| Regex | [regex](https://docs.rs/regex/latest/regex/) plus [fancy-regex](https://docs.rs/fancy-regex/latest/fancy_regex/) for required look-around/backreferences | `regex` alone does not cover Java regex. Audit keyword resources and user replacement syntax; bound expensive matching and show unsupported-pattern errors. Neither is a universal Java Pattern clone. | E07/L02 |
| Files and ignore rules | [ignore](https://docs.rs/ignore/latest/ignore/) | Recursive walking and Git ignore support; Kode adds source roots, explicit exclusions, hidden metadata policy, permission-error handling, and cancellation. Tree visibility, search scope, and index scope are separate policies. | W01/W02/W04 |
| Filesystem refresh | [notify](https://docs.rs/notify/latest/notify/) | Notifications plus coalescing; retain periodic refresh/polling fallback for unsupported/network filesystems. Preserve selection and expanded nodes. | W02 |
| Clipboard | [arboard](https://docs.rs/arboard/latest/arboard/) | Local clipboard adapter plus process-local fallback. SSH/browser copy may use explicit OSC 52 support; terminal paste arrives through input. Do not require a desktop clipboard on remote hosts. | E04 |
| Incremental parsing | [tree-sitter](https://docs.rs/tree-sitter/latest/tree_sitter/) and grammar crates | Reuse parser/tree, apply `InputEdit`, compile queries. Parser output is not symbol resolution. Audit all grammar ABIs and AST assumptions. | L02–L04 |
| Kotlin grammar candidate | [tree-sitter-kotlin-ng](https://docs.rs/tree-sitter-kotlin-ng/latest/tree_sitter_kotlin_ng/) | Candidate pending AST parity check against the Kotlin binding currently used. A crate with a similar name is not proof of matching nodes. Pin a grammar source revision if necessary. | L02/L03 |
| Native semantic persistence | [rusqlite](https://docs.rs/rusqlite/latest/rusqlite/) | Internal SQLite cache only; schema migrations, atomic publication, and semantic facts remain Kode work. Prefer bundled SQLite if supported by packaging tests. | L05 |
| LSP data structures | [lsp-types](https://docs.rs/lsp-types/latest/lsp_types/) | Supplies protocol types, not a complete editor client. Implement bounded stdio framing, request correlation, capability checks, and lifecycle using Tokio/Serde. | P02 |
| Git | [git2](https://docs.rs/git2/latest/git2/struct.Repository.html), documented 0.21.0 | Repository API covers required status, index, history, checkout, commits, tags, and diffs. Uses libgit2; package/build native dependencies explicitly. Repository handles stay in their worker. | G01/G02 |
| Diff alignment | [similar](https://docs.rs/similar/latest/similar/) | Produces text differences; Kode supplies synchronized panes, styling, hunk actions, and stale-file guards. git2 remains authoritative for staged/unstaged repository state. | G03 |
| JSON persistence | [serde_json](https://docs.rs/serde_json/latest/serde_json/) plus Serde derive | Use `Value` with object order preservation for Postman, unknown fields, and extension blocks. Enable number preservation where required by round-trip fixtures. “Lossless” means semantic fields/values, not original whitespace. | W03/R01/D01 |
| YAML import | [serde-saphyr](https://docs.rs/serde-saphyr/latest/serde_saphyr/) | Selected candidate for maintained Serde-compatible YAML parsing; prove OpenAPI 2/3 fixture handling and configured resource limits. Mapping to Postman remains custom. | R03 |
| HTTP | [reqwest](https://docs.rs/reqwest/latest/reqwest/) | Async client, cookies, redirects, multipart, TLS, streaming. Explicitly select supported features for the pinned version; request settings, header inheritance, previews, and collection runs remain custom. | R04 |
| SSE | [eventsource-stream](https://docs.rs/eventsource-stream/latest/eventsource_stream/) | Parse an HTTP byte stream into SSE events. Preserve incremental display, cancellation, and event framing across chunk boundaries. | R05 |
| WebSocket | [tokio-tungstenite](https://docs.rs/tokio-tungstenite/latest/tokio_tungstenite/) | Async WebSocket stream; validate TLS/proxy/cookie behavior separately from reqwest. Avoid assuming reqwest settings configure it. | R05 |
| Raster decode/resize | [image](https://docs.rs/image/latest/image/) | Supports many raster formats; choose codec features and impose decode limits. Does not by itself replace SVG/PSD/KRA handling or Kode's Braille/quadrant renderer. | V03 |
| SVG | [resvg](https://docs.rs/resvg/latest/resvg/) | Rasterize SVG, then use the common cell renderer. Verify fonts and sizing against fixtures. | V03 |
| PSD candidate | [psd](https://docs.rs/psd/latest/psd/) | Check composite/layer, bit-depth, and color-mode coverage against Kode samples before claiming support. | V03 |
| ZIP resources | [zip](https://docs.rs/zip/latest/zip/) | LSP archives and Krita merged previews; define extraction/path and decompression bounds. KRA's composite PNG extraction is application logic. SDK archive helpers are currently inactive source code. | P01/V03 |
| XML formatting | [quick-xml](https://docs.rs/quick-xml/latest/quick_xml/) | Parse/format REST XML without rewriting invalid payloads. Do not build a partial Maven resolver with it. | R07 |
| CLI | [clap](https://docs.rs/clap/latest/clap/) | File/folder/help/serve arguments and validation; preserve existing startup rules. | W05/Z01 |
| Runtime/processes | [tokio process](https://docs.rs/tokio/latest/tokio/process/) and Tokio channels/timers | Explicit child termination/reaping, cancellation, bounded queues, and shutdown. Async runtime does not make blocking calls safe on the UI thread. | F03/D02/P02 |
| JDBC wire frames | [tokio-util length codec](https://docs.rs/tokio-util/latest/tokio_util/codec/length_delimited/index.html) | Big-endian u32 frame prefix over pipes; maximum frame size and streaming are fixed in JDBC.md. This is separate from LSP Content-Length framing. | D02 |
| Status metrics | [sysinfo](https://docs.rs/sysinfo/latest/sysinfo/) | Process memory/CPU sampling away from drawing; UI calculates FPS. Avoid full-system refresh per frame. | Z02 |

Supporting dependencies to pin in F02/F03 as needed: `thiserror`, `anyhow`,
`tracing`, `tracing-subscriber`, `tracing-appender`, `bytes`, `futures-util`, `url`,
`uuid`, `sha2`, `base64`, `tempfile`, and platform config-directory helpers.
These are utility choices, not replacements for a Kode feature. Verify their APIs
at adoption. Add dependencies only to the crate that uses them.

## External JDBC decision

| Approach investigated | Evidence | Decision |
|---|---|---|
| Embed JVM through `j4rs` | [j4rs](https://docs.rs/j4rs/latest/j4rs/) exposes Java invocation and Maven artifact helpers | Rejected for this migration: user explicitly prefers an external JVM. |
| Embed JVM through `jni` | [jni](https://docs.rs/jni/latest/jni/) supports launching/interacting with a JVM | Rejected for the same reason; JNI lifetimes and VM startup would also enter the Rust process. |
| Replace JDBC with SQLx | [sqlx](https://docs.rs/sqlx/latest/sqlx/) provides Rust database drivers | Does not preserve the existing arbitrary JDBC JAR catalog and Java driver model. Not the selected database architecture. |
| Native drivers per engine | [tokio-postgres](https://docs.rs/tokio-postgres/latest/tokio_postgres/), [mysql_async](https://docs.rs/mysql_async/latest/mysql_async/), [tiberius](https://docs.rs/tiberius/latest/tiberius/), [oracle](https://docs.rs/oracle/latest/oracle/), [duckdb](https://docs.rs/duckdb/latest/duckdb/) | Viable for particular engines; unnecessary replacement work when JDBC is retained. Not required for parity. |
| ODBC fallback | [odbc-api](https://docs.rs/odbc-api/latest/odbc_api/) | Different driver ecosystem and installation requirements; does not run arbitrary JDBC drivers. Not selected. |
| Java helper child process | [Tokio process](https://docs.rs/tokio/latest/tokio/process/), [JDBC Statement](https://docs.oracle.com/en/java/javase/21/docs/api/java.sql/java/sql/Statement.html), [JDBC metadata](https://docs.oracle.com/en/java/javase/21/docs/api/java.sql/java/sql/DatabaseMetaData.html) | **Selected.** Preserve JDBC behind a narrow process protocol and keep Rust responsible for the IDE. |
| Maven driver dependencies in helper | [Apache Maven Resolver](https://maven.apache.org/resolver/) | Selected Java library for Maven dependency resolution. Preserve local and vendor JAR paths too. Runtime users need Java, not a Maven installation. |

The helper uses Java's [URLClassLoader](https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/net/URLClassLoader.html)
and JDBC `Driver` discovery. JDBC cancellation is a request whose actual behavior
depends on the driver; the protocol must distinguish acknowledgement, completion,
unsupported cancellation, and unknown outcome. See [JDBC.md](JDBC.md).

The recommendation to isolate JDBC is an architectural judgment based on the
documented APIs and the user's stated preference, not a claim that a published
crate already implements Kode's complete database module.

## Alternatives deliberately not used for the default implementation

- [tui-textarea](https://docs.rs/tui-textarea/latest/tui_textarea/) is useful for
  text widgets, but adopting it does not supply Kode's folding, minimap, persistent
  undo, semantic overlays, popup capture, or exact wrapped hit testing. Use one
  reusable Kode editor built on Ropey and Ratatui for full and embedded editing.
- [syntect](https://docs.rs/syntect/latest/syntect/) is a syntax-highlighting option,
  not a semantic index. The current active implementation uses configurable keyword
  regex and Tree-sitter semantic facts. No TextMate replacement is required merely
  because old README prose mentions TM4E.
- [lsp-server](https://docs.rs/lsp-server/latest/lsp_server/) is server-oriented;
  do not mistake it for a complete editor-side LSP client. Use `lsp-types` and a
  small explicit client transport.
- [ratatui-image](https://docs.rs/ratatui-image/latest/ratatui_image/) offers terminal
  image protocols. Optional future enhancement; preserve portable colored cell
  modes first, including over SSH and browser terminals.
- [serde_yaml](https://docs.rs/serde_yaml/latest/serde_yaml/) is documented as
  unmaintained/deprecated. Do not introduce it as the default YAML parser.
- [openapiv3](https://docs.rs/openapiv3/latest/openapiv3/) supplies typed OpenAPI
  structures, not Postman conversion or execution. It may assist a strictly
  versioned importer later; the baseline needs both Swagger 2 and OpenAPI 3 plus
  unknown-field retention, so a JSON/YAML value tree remains the source model.
- [keyring](https://docs.rs/keyring/latest/keyring/) is an optional credential-store
  backend candidate. A working headless configuration and legacy credential import
  remain required; a desktop secret service cannot become a startup dependency.

## Required adoption experiments

1. F02/F04: build one aligned Ratatui/Crossterm dependency graph, draw a custom
   widget, receive resize/mouse/paste, restore the terminal after an error.
2. E01/E05: round-trip byte/UTF-16/cell coordinates for tab, control, combining,
   wide, and supplementary characters, including wraps and mouse clicks.
3. E07/L02: compile all shipped patterns; compare captures/replacements and
   markup/CSS look-around behavior. Publish any syntax differences explicitly.
4. L02: parse one fixture per enabled grammar and compare node/field names before
   porting extraction. Record binding version, grammar revision, ABI, and license.
5. G01/G02: use a temporary repo for staged versus worktree changes, an unborn HEAD,
   detached HEAD, branch checkout, commit, and tags. Check libgit2 build requirements.
6. D02–D05: perform a Rust–Java hello, H2 query, streamed result, cancellation, and
   forced helper exit. This proves the process architecture before database UI work.
7. R03–R05: import OpenAPI JSON/YAML, execute against a loopback HTTP server, stream
   SSE/WS in pieces, cancel, and verify cookies and duplicate response headers.
8. V03: decode every advertised sample format, including SVG/PSD/KRA/DDS; report
   actual coverage. A successful PNG decode is insufficient.

These experiments are implementation tasks. No dependency installation, Rust
prototype, Java helper, build, or application test was performed during planning.
