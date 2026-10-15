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
    @org.junit.jupiter.api.Test void everyDocumentedMethodHasSharedWireFixture() throws Exception {
        var root=java.nio.file.Path.of(System.getProperty("kode.protocol.fixtures"));
        JsonNode schema=FrameCodec.JSON.readTree(root.resolve("operations.schema.json").toFile());
        JsonNode fixtures=FrameCodec.JSON.readTree(root.resolve("operation-fixtures.json").toFile());
        var covered=new java.util.TreeSet<String>();
        for(JsonNode value:fixtures.path("valid")) {assertEquals(value,FrameCodec.decode(FrameCodec.encode(value)));String method=value.path("method").asText();covered.add(method);assertTrue(schema.path("$defs").has(schema.path("x-methods").path(method).asText()));}
        assertEquals(schema.path("x-methods").size(),covered.size());
    }
    @org.junit.jupiter.api.Test void sharedQueryFixturesPreserveCellsSequencesAndUnicode() throws Exception {
        JsonNode fixture=FrameCodec.JSON.readTree(java.nio.file.Path.of(System.getProperty("kode.protocol.fixtures"),"query-fixtures.json").toFile());
        int sequence=0;
        for(JsonNode event:fixture.path("events")) {
            var envelope=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("method","query.event");envelope.set("params",event);
            assertEquals(envelope,FrameCodec.decode(FrameCodec.encode(envelope)));assertEquals(sequence++,event.path("sequence").intValue());
        }
        JsonNode rows=fixture.path("events").get(2).path("event").path("rows");assertEquals("null",rows.get(0).get(0).path("kind").asText());assertEquals("",rows.get(1).get(0).path("text").asText());assertEquals("AP8=",rows.get(0).get(1).path("base64").asText());assertEquals("12345678901234567890.000000000000000001",rows.get(2).get(0).path("text").asText());
    }
    @Test void sharedFixturesAndByteCountedConcatenatedFramesRoundtrip() throws Exception {
        JsonNode fixtures = FrameCodec.JSON.readTree(Files.readAllBytes(Path.of(System.getProperty("kode.protocol.fixtures"),"fixtures.json")));
        var output = new ByteArrayOutputStream();
        for (JsonNode value : fixtures.get("valid")) {
            byte[] bytes = FrameCodec.encode(value);
            assertEquals(value,FrameCodec.decode(bytes));
            FrameCodec.write(new DataOutputStream(output),bytes);
        }
        var input = new DataInputStream(new java.io.FilterInputStream(new ByteArrayInputStream(output.toByteArray())) {
            @Override public int read(byte[] bytes,int offset,int length) throws java.io.IOException {return super.read(bytes,offset,Math.min(1,length));}
        });
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
    @Test void depthNumberUtf8NameAndFrameBoundsAreEnforced() {
        var oversized=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id","x").put("result","x".repeat(FrameCodec.MAX_FRAME_BYTES));
        assertThrows(FrameCodec.ProtocolException.class,()->FrameCodec.encode(oversized));
        var name=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id","😀".repeat(33));name.putNull("result");assertThrows(FrameCodec.ProtocolException.class,()->FrameCodec.validate(name));
        JsonNode nested=FrameCodec.JSON.nullNode();for(int i=0;i<65;i++)nested=FrameCodec.JSON.createArrayNode().add(nested);
        var deep=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id","x");deep.set("result",nested);assertThrows(FrameCodec.ProtocolException.class,()->FrameCodec.validate(deep));
        String number="{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"result\":"+"7".repeat(1001)+"}";assertThrows(FrameCodec.ProtocolException.class,()->FrameCodec.decode(number.getBytes(java.nio.charset.StandardCharsets.UTF_8)));
    }
}
