# JDBC stdio protocol v1

Normative lifecycle/operation semantics: [JDBC.md](../../docs/migration/JDBC.md).
Each frame is four unsigned big-endian length bytes followed by exactly that many
UTF-8 JSON bytes. Payload bounds are 1..8,388,608 bytes. Zero, oversized, truncated,
invalid UTF-8/JSON, non-object/batch and malformed envelope frames fail the stream;
untrusted payloads are never quoted in diagnostics. Nesting is bounded to 64 levels,
and JSON numeric tokens to 1,000 characters. Unicode strings must contain scalar
values; unpaired escaped surrogate units are invalid in both implementations.

Envelopes use `jsonrpc: "2.0"`; IDs/methods are nonempty strings up to 128 UTF-8
bytes without control characters. Params are objects. Responses have a string ID
and exactly one result/error. Notifications omit IDs. Unknown optional fields
remain available to operation decoders. [Operation definitions](operations.schema.json)
cover every planned method, SQL events, typed cells and results. The `x-methods`
map selects the parameter definition; connection close/commit/rollback use
`connectionId`. [Query fixtures](query-fixtures.json) are shared by Rust and Java.
Schema character lengths are supplemented by the stricter runtime UTF-8 byte
limits above. No request body is a log record. Connection properties are ephemeral
and may contain resolved secrets; never persist wire request objects as settings.

The first request must be hello with protocolVersion 1. A successful hello reports
helper/Java versions, a fresh helper epoch, maxFrameBytes and actual capabilities.
Unknown methods return -32601; invalid method parameters return -32602; incompatible
protocol versions return -32001. Handshake failure never enables JDBC operations.

Reader/control/worker/writer roles are independent. Writer queues and byte budgets
are bounded; control responses have priority, while a query's final response must
follow its own events. Overflow is an explicit failure, never silent query-data
loss. Cancellation acknowledgement is not query completion. A helper failure
invalidates its epoch and pending IDs and must never cause SQL replay.

The Rust supervisor reserves eight control slots and 1 MiB independently of the
64 ordinary pending calls, 32 queued frames and 32 MiB queued/inflight payloads.
The Java dispatcher uses four ordinary threads with a 32-item work queue and two
control threads with a 16-item queue; accepted calls retain bounded byte permits
until their completion. Its writer has separate eight-frame/1 MiB control and
32-frame/32 MiB ordinary queues (plus a currently written frame, whose bytes stay
charged). Query events and their own final response use the same FIFO.

The Rust reader retains up to 32 notifications/32 MiB independently of rendering.
Overflow disconnects the helper visibly, keeps the already received preview,
and marks submitted mutating calls unknownOutcome. It never silently drops query
data. Future query consumers must drain notifications independently of painting;
driver progress may be coalesced by its owning feature. Cancellation while still
queued atomically prevents later submission; cancellation/timeout after writing
may have begun cannot prove rollback. Dropping a call does not cancel JDBC work:
send query.cancel/driver.cancel with the operation ID explicitly.

`Runtime::new` performs no I/O; Java probing and helper lookup happen only in
`get` for an explicit JDBC operation. KODE_JDBC_HELPER overrides the packaged
`jdbc/kode-jdbc-bridge.jar` resource path. Failed running epochs require explicit
restart. Workspace shutdown joins startup/probe tasks and live process actors.
Every start uses a fresh helper epoch and request IDs are never reused within it.
Raw stderr is drained to 64 KiB and suppressed in public diagnostics, including
launcher output: third-party drivers are not trusted to redact their own logs.

Build/test with `bash scripts/test-jdbc-bridge.sh` (Java 21, Maven at build time).
The resulting shaded `jdbc-bridge/target/kode-jdbc-bridge.jar` needs only Java at
runtime. Jackson 2.21.4 is pinned and relocated to the helper namespace so later
vendor classloaders can use their own Jackson versions. The unshaded build input
is separate, so repeated packaging never shades an already shaded JAR.
