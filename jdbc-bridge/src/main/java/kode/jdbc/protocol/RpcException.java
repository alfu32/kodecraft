package kode.jdbc.protocol;

import com.fasterxml.jackson.databind.JsonNode;

/** Only feature-provided, already-redacted errors may enter response messages. */
public final class RpcException extends Exception {
    public final int code;
    public final JsonNode data;
    public RpcException(int code, String message) { this(code, message, null); }
    public RpcException(int code, String message, JsonNode data) { super(message); this.code = code; this.data = data; }
}
