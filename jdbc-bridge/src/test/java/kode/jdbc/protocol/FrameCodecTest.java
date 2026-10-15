package kode.jdbc.protocol;

import static org.junit.jupiter.api.Assertions.*;
import com.fasterxml.jackson.databind.JsonNode;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;

class FrameCodecTest {
    @Test void sharedFixturesAndByteCountedConcatenatedFramesRoundtrip() throws Exception {
        JsonNode fixtures = FrameCodec.JSON.readTree(Files.readAllBytes(Path.of(System.getProperty("kode.protocol.fixtures"),"fixtures.json")));
        var output = new ByteArrayOutputStream();
        for (JsonNode value : fixtures.get("valid")) {
            byte[] bytes = FrameCodec.encode(value);
            assertEquals(value,FrameCodec.decode(bytes));
            FrameCodec.write(new DataOutputStream(output),bytes);
        }
        var input = new DataInputStream(new ByteArrayInputStream(output.toByteArray()));
        for (JsonNode value : fixtures.get("valid")) assertEquals(value,FrameCodec.read(input).value());
        assertNull(FrameCodec.read(input));
        for (JsonNode value : fixtures.get("invalid")) assertThrows(FrameCodec.ProtocolException.class,()->FrameCodec.validate(value));
    }
    @Test void malformedHeadersPayloadsUtf8SurrogatesAndStdoutPoisonFailWithoutBodyEcho() {
        for (byte[] bytes : new byte[][] {{0,0,0,0},{0,(byte)128,0,1},{0,0},{0,0,0,8,'{'},{0,0,0,2,(byte)255,(byte)254},"poisoned stdout".getBytes()}) {
            assertThrows(FrameCodec.ProtocolException.class,()->FrameCodec.read(new DataInputStream(new ByteArrayInputStream(bytes))));
        }
        for (String text : new String[] {"synthetic-private-marker", "{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"result\":\"\\uD800\"}", "{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"result\":null} false"}) {
            Exception failure=assertThrows(FrameCodec.ProtocolException.class,()->FrameCodec.decode(text.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
            assertFalse(failure.getMessage().contains("synthetic-private-marker"));
        }
    }
}
