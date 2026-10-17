# JDBC stdio protocol v1

## Lazy metadata

`metadata.list` takes `connectionId`, a unique `metadataJobId` and a normalized request (`ROOT`, `CATALOG`,
`SCHEMA`, `CATEGORY`, `OBJECT`). Category requests require `category`; object
requests require `objectName` and `objectType`. Optional catalog/schema/name
identifiers are at most 4,096 UTF-8 bytes and cannot contain NUL. The connection
owner performs every JDBC getter, metadata iteration and ResultSet close.
Schema/table name patterns are escaped as literal identifiers. Catalog-scoped
schema/table calls retain the source's unscoped fallback when the first result
is empty; no vendor package SQL is invented.

Results preserve normalized objects, qualified names, attributes, expandable/leaf
flags and capabilities, and include the owner's `contextRevision`. Unsupported
accessors add a static visible `message`, preserving other supported sections;
ordinary SQL failures remain SQL errors. Standard JDBC has no package discovery,
so that category reports the limitation. Table children include columns, primary
keys, indexes and imported foreign keys; routine children are parameters only.
Responses are bounded to 10,000 objects / 3 MiB serialized objects, with at most
50,000 scanned rows and 8,192 UTF-8 bytes per string. Exceeding bounds returns an
explicit SQLSTATE 54000 failure. These limits do not bound driver allocations.
[Shared metadata fixtures](metadata-fixtures.json) exercise Unicode names,
foreign-key attributes and unsupported messages in Java and Rust.

Native caching is lazy and memory-only: 256 entries / 16 MiB encoded payloads,
16 concurrent native waits, a 60-second request deadline. UI-retained result Arcs
outlive cache eviction and are outside that cache accounting. Cache identity
includes datasource, helper epoch, connection ID, context revision, native
connection revision and the complete normalized request. Every SQL submission
and final invalidates conservatively, including procedure calls with unknown
schema effects. Refresh invalidates individual nodes or whole datasources and
rejects replies started before that refresh. Reconnect/helper loss/context change
cannot refill a newer tree from an older response. UI composition additionally
checks `Metadata.is_current` immediately before publishing an async tree update.
Cancellation/drop sends a priority job cancellation and does not cache the result.
Package source requests may carry `objectId` in addition to the display name. Providers
use it when a name has multiple independently addressable versions (Db2 LUW binds
schema, package name, version and `UNIQUE_ID` into this identity); clients treat it as
opaque. Package source uses a distinct `metadataJobId` and the reserved `metadata.cancel` control operation; the helper
schedules `Statement.cancel` off the connection owner and waits for that exact
cancel call before closing/releasing its statement. Cancellation may arrive before
the source request is dispatched; bounded tombstones preserve that ordering.
The cancellation acknowledgement includes `statementAttached`, which reports
whether the job had attached a JDBC statement when cancellation was accepted; it
does not claim that the driver has finished cancelling that statement.
Ordinary `metadata.list` carries a unique `metadataJobId`; cancellation uses the
same priority `metadata.cancel` operation as package source. The helper cancels
the active JDBC statement off the connection owner and waits for that call before
closing its result set. Cancellation-before-dispatch is retained in bounded
tombstones. Metadata is never replayed.

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
map selects the parameter definition; commit/rollback use `connectionId`.
Close also accepts `dataSourceId` or `attemptId`. [Query fixtures](query-fixtures.json)
are shared by Rust and Java.
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

Ordinary response delivery runs independently from JDBC owners. Its virtual tasks
remain bounded by the 64 accepted ordinary request permits, which are released
only after the reply is queued. A full output queue cannot pin a connection worker
while it completes a future. Close uses the independent reserved control route.

Connections have one platform owner per datasource (32 live connections, 16 queued
ordinary jobs per owner, plus a reserved close command). Four temporary-test
workers have 16 queued slots; two abort workers have 32 queued slots. Tests close
their temporary connection before replying and leave live sessions unchanged.
Only explicit cancellation/abort crosses JDBC ownership; statements, result sets,
metadata and transaction operations stay on the owner. Driver leases remain held
until closure is confirmed or the helper dies. An unconfirmed close requires
explicit helper restart, never a fabricated disconnect or rollback.

Native open/test calls supply a unique `attemptId`. Priority close records a
tombstone even if ordinary open dispatch has not happened yet; a later open/test
with that attempt cannot create a hidden connection. Tombstones are bounded to
4,096 per helper epoch; exhaustion requests explicit restart. Close by attempt
cannot target a newer connection. Omission remains supported for older callers,
but they cannot cancel before a connection ID exists with this guarantee.

`contextRevision` changes when context mutation is attempted, including partial
failure; metadata uses it for invalidation. `stateRevision` orders owner snapshots
so independently delivered replies cannot restore older transaction/context state.
Omitted setContext fields remain unchanged; explicit null catalog/schema restores
login defaults. Driver setters are checked against actual getters; ignored
read-only/context changes are errors. Enabling autocommit occurs last because it
can commit work. Manual transaction failures preserve SQLSTATE/vendor/chained
errors and actual context; failed transactions require rollback before commit.
Known property values and named credential assignments are redacted from bounded
error messages. Driver stderr is drained but never exposed as raw diagnostics.

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

D03 adds independent notification subscriptions by method: driver progress cannot
consume query events. Progress coalesces by resolution ID; query events remain
ordered and lossless until an explicit bounded overflow failure. Consumers drain
already received events before observing a stopped epoch. Dropping an unfinished
install wait sends driver.cancel; an ordinary cancellation waits for its original
final reply, so an acknowledgement never claims cleanup has finished.

`driver.inspect` accepts either providerId/classpath/driverClass or a driver spec.
The spec form checks explicit local paths, validated immutable installs and legacy
provider/version directories without downloading or creating an install root.

SQL execution runs through that connection's owner. `Statement.execute`, result
sets and large update counts iterate through `getMoreResults`; warnings and later
errors preserve earlier received results. Decimal/boolean/temporal values retain
source display strings, SQL NULL has its own kind and binary previews use base64.
LOB/XML streams close and JDBC values free on the owner. Arrays use at most 32
element previews. Invalid driver UTF-16 becomes U+FFFD without splitting scalars.

SQL text is limited to 4 MiB UTF-8; maxRows defaults to 500 (1..100,000) and fetch
hint to 200 (0..10,000). Preview limits: 256 columns, 1 KiB UTF-8 labels/type names,
64 KiB per cell and 512 KiB raw data per row. Wide rows reduce each cell's bound,
with truncation explicit. Batches have at most 200 rows, split around 2 MiB encoded
data; a single larger bounded row fits the 8 MiB frame ceiling. One extra row is
fetched to distinguish an exact row cap from a limited result. Result sequences
are bounded to 1,024 result sets/update counts. Limits are not hard driver RSS caps.

Each execution ID is unique for its helper epoch. Early query.cancel records a
bounded tombstone so priority cancellation cannot be overtaken by a queued execute.
Four cancellation workers/64 queued calls operate independently of owner/IPC
threads; a separate deadline returns requested after 50 ms if cancel is still
pending. Known failures return unsupported; neither result ends the original
operation. While that execution owns the connection, cancellation retries cover
drivers resetting their flag on execute entry. The owner waits for its cancel
call before closing the statement or running the next query, preventing an old
connection-wide cancel from affecting a new execution. A hanging driver requires
explicit disconnect/helper restart. Cancellation never claims transaction rollback.

Final results include `eventCount`, the number of successfully queued preceding
notifications. The Rust collector drains through that count before publishing
completion; a known final remains authoritative even if the helper exits directly
afterward. `connectionState` carries the owner snapshot for transaction/context
tracking. Native execute waits allow 600 seconds, cancel acknowledgement waits two
seconds and final-event draining five seconds. Failure leaves the received preview
and marks uncertainty; SQL is never automatically submitted again.

The native executor permits 16 operations and drains one dedicated query consumer
independently from drawing. It validates IDs, sequence, result transitions, widths,
cells and byte bounds. Retention is capped at 32 MiB per preview and 64 MiB shared;
immutable result clones retain their byte permits until released. Exhaustion is
explicit (`retention_limited`, incomplete preview and cancellation request), not
silent coalescing. Progress may coalesce snapshots; row chunks remain owned.
Cancellation status/error stays visible while awaiting the original final; explicit
retry uses the same execution ID. SQL text and cell/error bodies are omitted from
native debug diagnostics.
An empty explicit classpath requests helper-visible classes; the source-compatible
bundled H2 version is 2.2.224. Spec lookup never substitutes that bundle for a
different explicitly selected version. Version-qualified lookup recognizes a
previous latest-version install when its family and resolved version agree.
Inspection reports actual selected classes, sanitized load error types and restart
requirements. Only driver.resolve downloads/publishes an install. Its sources use
the artifact repository type; explicit local loading remains an inspect override.

Build/test with `bash scripts/test-jdbc-bridge.sh` (Java 21, Maven at build time).
The resulting shaded `jdbc-bridge/target/kode-jdbc-bridge.jar` needs only Java at
runtime. Jackson 2.21.4 is pinned and relocated to the helper namespace so later
vendor classloaders can use their own Jackson versions. The unshaded build input
is separate, so repeated packaging never shades an already shaded JAR.
