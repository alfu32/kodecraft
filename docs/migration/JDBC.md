# External JVM JDBC bridge

## Boundary

The user explicitly chose an **external JVM**. The IDE, its editor, Git, REST,
workspace management, and internal code index are Rust. Java is a small separate
helper for JDBC driver loading, Maven artifact resolution, connections, metadata,
and SQL execution. No JVM is embedded; no Kotlin runtime is required by the new
helper. Existing user H2/Derby/etc. datasources remain supported through JDBC.

Java baseline: 21. Runtime lookup order: explicit configured Java executable /
`KODE_JAVA` → `JAVA_HOME/bin/java` → `java` on PATH. Validate the executable version
and record it in the handshake. Default distribution uses an externally installed
JVM. A JRE bundle is a later packaging option, not a prerequisite for ordinary use.
Absence of Java makes database connections unavailable with an actionable message;
all other IDE features continue to work.

Start one helper lazily for the active workspace's first JDBC operation. Reuse it
for active datasource connections; isolate driver artifacts in classloaders keyed
by provider ID and resolved classpath digest. Switching workspaces or exiting
closes sessions and reaps the helper. Helpers do not listen on a network port.

## Transport v1

Rust spawns `java -jar <resource-path>/kode-jdbc-bridge.jar` with piped stdin/stdout
and separately drained stderr. Arguments never contain passwords or complete URLs
with credentials. Protocol stdout is reserved: Java captures its original output
stream for frames and redirects `System.out` to stderr before loading any driver.
Unexpected/corrupt output is a protocol error, not ignored bytes.

Frames: **four-byte unsigned big-endian payload length, then UTF-8 JSON**. Maximum
payload: **8 MiB**. Use Tokio's length-delimited codec and Java `DataInputStream` /
`DataOutputStream`; flush complete frames. Reads must tolerate arbitrary pipe
fragmentation and adjacent frames. Reject zero-length, oversized, invalid UTF-8,
invalid JSON, and truncated frames with a controlled disconnect.

Requests use JSON-RPC 2.0-style envelopes, string IDs, and method-specific params.
Each request gets exactly one final result/error response. Progress and query data
are notifications. No JSON-RPC batch requests in v1. `hello` must be first; it
negotiates `protocolVersion: 1` and reports helper build and Java versions.

```json
{"jsonrpc":"2.0","id":"rpc-1","method":"hello","params":{"protocolVersion":1,"clientVersion":"0.1.0"}}
{"jsonrpc":"2.0","id":"rpc-1","result":{"protocolVersion":1,"helperVersion":"0.1.0","javaVersion":"21","maxFrameBytes":8388608}}
{"jsonrpc":"2.0","id":"rpc-7","method":"query.execute","params":{"connectionId":"connection-a","executionId":"execution-a","sql":"select 1","maxRows":500,"fetchSize":200}}
{"jsonrpc":"2.0","method":"query.event","params":{"executionId":"execution-a","sequence":0,"event":{"type":"started"}}}
{"jsonrpc":"2.0","method":"query.event","params":{"executionId":"execution-a","sequence":1,"event":{"type":"resultSetStart","resultSetId":"result-a","columns":[{"label":"1","jdbcType":4,"typeName":"INTEGER","nullable":false}]}}}
{"jsonrpc":"2.0","method":"query.event","params":{"executionId":"execution-a","sequence":2,"event":{"type":"rows","resultSetId":"result-a","rows":[[{"kind":"text","text":"1"}]]}}}
{"jsonrpc":"2.0","method":"query.event","params":{"executionId":"execution-a","sequence":3,"event":{"type":"resultSetEnd","resultSetId":"result-a","limited":false}}}
{"jsonrpc":"2.0","id":"rpc-7","result":{"executionId":"execution-a","outcome":"completed","elapsedMs":4,"transactionUsable":true,"previewIncomplete":false}}
```

D02 formalizes these examples as schemas and round-trip fixtures in
`protocol/jdbc-v1/`. Unknown optional fields are ignored/preserved where appropriate;
unknown methods and incompatible major protocol versions produce explicit errors.

## Operations

| Method | Required semantics | Owner |
|---|---|---|
| `hello`, `ping`, `shutdown` | Compatibility, health, graceful termination | D02 |
| `driver.resolve` | Resolve Maven coordinates/version/runtime graph; vendor/local artifact manifests; progress notifications | D03 |
| `driver.cancel` | Cancel a particular resolution/download ID, clean temporary files | D03 |
| `driver.inspect` | Classpath digest, discovered Driver classes, load errors; no connection needed | D03 |
| `connection.test` | Temporary connection; return product/driver/version/latency or SQL error, then close | D04 |
| `connection.open` | Driver spec/classpath, JDBC URL, properties, ephemeral credentials; return connection ID and capabilities | D04 |
| `connection.close` | Cancel/finish owned work, close connection, invalidate cache | D04 |
| `connection.setContext` | Catalog/schema, read-only, autocommit with driver support checks | D04 |
| `connection.commit`, `connection.rollback` | Serialize on the owning connection; preserve errors and transaction state | D04 |
| `metadata.list` | Lazy metadata request using existing normalized object/request types | D06 |
| `query.execute` | Execute SQL; stream all result sets/update counts/warnings in order | D05 |
| `query.cancel` | Independent control request for one execution ID; acknowledgement is not completion | D05 |

`connection.open` creates one connection per active datasource by default, matching
the source's datasource-level transaction behavior. Multiple SQL consoles bound
to that datasource share its transaction context. Make that visible in the UI.
Additional independent connections can be an explicit future operation.

Requests for a connection are serialized on its worker, including metadata and
transaction commands. The pipe reader and cancellation executor are separate from
that queue. No JDBC object is shared arbitrarily across workers. An active
statement is registered by execution ID before work starts; cancellation cannot
accidentally target the next statement on the same connection.

## Query data and flow control

Emit `started`, `resultSetStart`, `rows`, `resultSetEnd`, `updateCount`, and `warning`
events, each with execution ID and monotonic sequence. Final result reports
`completed`, `cancelled`, or `failed`, elapsed time, and transaction usability.
Execution failures include SQLSTATE, vendor code, message, and chained exceptions.
Rust keeps already received results when a later statement fails.

Default row cap: 500 per result set, matching the source; fetch hint: 200. Emit at
most 200 rows per frame and split earlier to obey the frame limit. Bound each cell
preview to 64 KiB, mark truncated cells explicitly, and expose received/limited
status. These are preview limits, not claims that the DB returned fewer rows.

Cell kinds: `null`, `text`, and `binary`. Numeric/decimal/temporal values are
lossless display strings with JDBC type metadata; binary previews use base64 on
the wire and hex in the UI. Preserve SQL NULL separately from an empty string.
Close/free large JDBC objects after converting their bounded preview. Do not send
entire unbounded BLOB/CLOB values in a single JSON frame.

One writer serializes frames from bounded queues. Responses to independent control
requests have priority over row batches. A query's own final response must follow
all its emitted query events; it cannot overtake queued rows. Preserve per-query
event sequence. Cancellation may discard unsent rows only if its final outcome
explicitly marks the preview incomplete. If output is slow, row production waits
with cancellation checks. The Rust bridge reader drains frames independently from
TUI rendering; it coalesces progress, not query data or final status. Cancelling or
shutting down must still work when the data queue is full. Cross-query ordering is
unspecified; each query's sequence and final response are authoritative.

## Cancellation and failures

`query.cancel` is processed immediately by a separate control executor and calls
the active statement's JDBC cancellation method. Its result is one of
`requested`, `alreadyFinished`, or `unsupported`; completion is delivered by the
original execution response. See the [JDBC Statement API](https://docs.oracle.com/en/java/javase/21/docs/api/java.sql/java/sql/Statement.html).

Do not block the pipe reader waiting for `Statement.cancel()` or `execute()`.
For drivers that hang, report that cancellation is still pending and offer an
explicit disconnect/helper restart action. Killing the helper affects all its
connections and can leave operation outcomes unknown. Never silently report such
an operation as rolled back or successful.

On EOF/crash/protocol error: fail pending calls, label active executions
`unknownOutcome` when appropriate, invalidate connection IDs and metadata caches,
retain SQL text/results, and expose reconnection. **Never automatically replay
SQL, commit, or rollback after a process failure.** A new helper has a new epoch;
ignore old responses and never reuse old connection IDs.

Normal exit: send shutdown, close connections/loaders, await a bounded grace period,
then terminate/reap if needed. EOF on parent stdin triggers helper shutdown. Rust
must drain stderr concurrently and retain only a bounded, redacted diagnostic tail.

## Driver and credential compatibility

Preserve every entry and alias in `JdbcDriverRegistry.defaultDefinitions()` and
`database-jdbc-url-templates.json`. Do not replace this with a short native-driver
list. Preserve family-scoped URL recents (12 MRU entries) and credential filtering.

Use Apache Maven Resolver inside the helper to handle runtime transitive artifacts,
parent POMs, dependency management, classifiers, and custom repositories. Pin the
resolved version/classpath in an install manifest; repeat launches reuse it.
Resolve downloads in temporary paths and publish a complete install atomically.
Vendor-download-page-only entries remain manual; do not fabricate a download URL.
Local classpaths support multiple JARs and platform path separators.

Retain isolated URLClassLoader + ServiceLoader/explicit class discovery behavior.
Use `Driver.connect` with the intended driver instance so DriverManager classloader
visibility does not accidentally choose another driver. Close classloaders only
after all their connections close; changing a native driver may require helper
restart rather than pretending native libraries can unload.

Rust resolves credential references and sends secrets only in the connection
request. The helper retains them only as needed for the live connection and does
not log/persist request bodies. Legacy `credentials.properties` import must honor
Java properties escaping. User datasource definitions and project session state
must never gain plaintext password fields.

## Independent implementation split

D01 owns Rust models/config. D02 owns wire DTOs, Java bootstrap, and Rust process
supervision. D03 owns Java artifact/classloader services and Rust installation
status. D04 owns Java connections. D05 owns Java execution/cancellation. D06 owns
metadata. D07–D08 own Rust UI. D09 proves the assembled Rust/Java workflow.

Rust and Java implementations both consume the same wire fixtures. A fixture
runner must prove malformed framing, fragmented UTF-8, missing Java, incompatible
versions, startup timeout, multiple in-flight request IDs, backpressure, query
cancellation, helper crash, disconnect, and shutdown. H2 is the deterministic
integration engine; local PostgreSQL/MySQL fixtures add real network-driver checks
when available. Catalog coverage is not a claim that every vendor DB was tested.
