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
remain available to operation decoders. SQL/error/driver schemas will extend the
same envelope without changing framing. No request body is a log record.

The first request must be hello with protocolVersion 1. A successful hello reports
helper/Java versions, a fresh helper epoch, maxFrameBytes and actual capabilities.
Unknown methods return -32601; invalid method parameters return -32602; incompatible
protocol versions return -32001. Handshake failure never enables JDBC operations.

Reader/control/worker/writer roles are independent. Writer queues and byte budgets
are bounded; control responses have priority, while a query's final response must
follow its own events. Overflow is an explicit failure, never silent query-data
loss. Cancellation acknowledgement is not query completion. A helper failure
invalidates its epoch and pending IDs and must never cause SQL replay.
