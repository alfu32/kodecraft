package kode.jdbc.protocol;

import com.fasterxml.jackson.core.StreamReadConstraints;
import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.json.JsonMapper;
import com.fasterxml.jackson.core.JsonFactory;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.Iterator;
import java.util.Map;

/** One bounded UTF-8 JSON message per unsigned four-byte length prefix. */
public final class FrameCodec {
    public static final int MAX_FRAME_BYTES = 8 * 1024 * 1024;
    public static final int MAX_DEPTH = 64;
    public static final JsonMapper JSON = JsonMapper.builder(JsonFactory.builder()
        .streamReadConstraints(StreamReadConstraints.builder().maxNestingDepth(MAX_DEPTH)
            .maxStringLength(MAX_FRAME_BYTES).maxNumberLength(1000).build()).build())
        .enable(DeserializationFeature.FAIL_ON_TRAILING_TOKENS)
        .enable(DeserializationFeature.USE_BIG_DECIMAL_FOR_FLOATS).build();

    public record Message(ObjectNode value, int bytes) {}
    public static final class ProtocolException extends IOException {
        public ProtocolException(String message) { super(message); }
    }
    private FrameCodec() {}

    public static Message read(DataInputStream input) throws IOException {
        int first = input.read();
        if (first == -1) return null;
        long length;
        try {
            length = Integer.toUnsignedLong((first << 24) | (input.readUnsignedByte() << 16)
                | (input.readUnsignedByte() << 8) | input.readUnsignedByte());
        } catch (EOFException failure) { throw new ProtocolException("Truncated frame header"); }
        if (length == 0 || length > MAX_FRAME_BYTES) throw new ProtocolException("Payload size outside 1..8 MiB");
        byte[] bytes = new byte[(int) length];
        try { input.readFully(bytes); }
        catch (EOFException failure) { throw new ProtocolException("Truncated frame payload"); }
        return new Message(decode(bytes), bytes.length);
    }
    public static ObjectNode decode(byte[] bytes) throws IOException {
        if (bytes.length == 0 || bytes.length > MAX_FRAME_BYTES) throw new ProtocolException("Payload size outside 1..8 MiB");
        try {
            String text = StandardCharsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT).decode(ByteBuffer.wrap(bytes)).toString();
            JsonNode value = JSON.readTree(text);
            validate(value);
            return (ObjectNode) value;
        } catch (ProtocolException failure) { throw failure; }
        catch (Exception failure) { throw new ProtocolException("Malformed UTF-8 JSON payload"); }
    }
    public static byte[] encode(JsonNode value) throws IOException {
        validate(value);
        byte[] bytes = JSON.writeValueAsBytes(value);
        if (bytes.length > MAX_FRAME_BYTES) throw new ProtocolException("Payload exceeds 8 MiB");
        return bytes;
    }
    public static void write(DataOutputStream output, byte[] bytes) throws IOException {
        if (bytes.length == 0 || bytes.length > MAX_FRAME_BYTES) throw new ProtocolException("Payload size outside 1..8 MiB");
        output.writeInt(bytes.length); output.write(bytes); output.flush();
    }
    private static boolean boundedName(JsonNode node) {
        if (node == null || !node.isTextual()) return false;
        String value = node.textValue();
        return !value.isEmpty() && value.getBytes(StandardCharsets.UTF_8).length <= 128
            && value.chars().noneMatch(Character::isISOControl);
    }
    private static void string(String value) throws ProtocolException {
        for (int index = 0; index < value.length(); index++) {
            char unit = value.charAt(index);
            if (Character.isHighSurrogate(unit)) {
                if (++index >= value.length() || !Character.isLowSurrogate(value.charAt(index)))
                    throw new ProtocolException("Unpaired Unicode surrogate");
            } else if (Character.isLowSurrogate(unit)) throw new ProtocolException("Unpaired Unicode surrogate");
        }
    }
    private static void tree(JsonNode value, int remaining) throws ProtocolException {
        if (value == null || remaining == 0) throw new ProtocolException("JSON nesting exceeds 64 levels");
        if (value.isTextual()) string(value.textValue());
        if (value.isNumber() && value.asText().length() > 1000) throw new ProtocolException("JSON number exceeds 1000 characters");
        if (value.isObject()) {
            Iterator<Map.Entry<String,JsonNode>> fields = value.properties().iterator();
            while (fields.hasNext()) { var field = fields.next(); string(field.getKey()); tree(field.getValue(), remaining-1); }
        } else if (value.isArray()) for (JsonNode child : value) tree(child, remaining-1);
    }
    public static void validate(JsonNode value) throws ProtocolException {
        if (value == null || !value.isObject()) throw new ProtocolException("Expected one JSON-RPC object; batches unsupported");
        tree(value, MAX_DEPTH);
        if (!value.path("jsonrpc").isTextual() || !value.path("jsonrpc").textValue().equals("2.0")) throw new ProtocolException("Expected JSON-RPC version 2.0");
        if (value.has("id") && !boundedName(value.get("id"))) throw new ProtocolException("ID must be a bounded nonempty string");
        if (value.has("method")) {
            if (!boundedName(value.get("method"))) throw new ProtocolException("Method must be a bounded nonempty string");
            if (value.has("result") || value.has("error")) throw new ProtocolException("Method envelope cannot contain result/error");
            if (value.has("params") && !value.get("params").isObject()) throw new ProtocolException("Params must be an object");
        } else {
            if (!value.has("id") || value.has("result") == value.has("error")) throw new ProtocolException("Response needs ID and exactly one result/error");
            if (value.has("error")) {
                JsonNode error = value.get("error");
                if (!error.path("code").isIntegralNumber() || !error.path("code").canConvertToLong() || !error.path("message").isTextual())
                    throw new ProtocolException("Malformed JSON-RPC error");
            }
        }
    }
}
