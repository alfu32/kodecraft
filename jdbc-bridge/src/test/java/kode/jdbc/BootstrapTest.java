package kode.jdbc;
import kode.jdbc.protocol.FrameCodec;
import java.io.*;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import static org.junit.jupiter.api.Assertions.*;
@Timeout(10)
class BootstrapTest {
    @Test void ordinaryDriverStdoutGoesToStderrAndEofShutsDown() throws Exception {
        Process child=new ProcessBuilder(java.nio.file.Path.of(System.getProperty("java.home"),"bin","java").toString(),"-cp",System.getProperty("java.class.path"),"kode.jdbc.StdoutProbeMain").start();
        try {
            DataOutputStream output=new DataOutputStream(child.getOutputStream());DataInputStream input=new DataInputStream(child.getInputStream());
            var hello=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id","hello").put("method","hello");hello.putObject("params").put("protocolVersion",1);FrameCodec.write(output,FrameCodec.encode(hello));assertEquals(1,FrameCodec.read(input).value().path("result").path("protocolVersion").intValue());
            var request=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id","stdout").put("method","fake.stdout");request.putObject("params");FrameCodec.write(output,FrameCodec.encode(request));assertTrue(FrameCodec.read(input).value().path("result").path("ok").asBoolean());
            output.close();assertTrue(child.waitFor(3,TimeUnit.SECONDS));assertEquals(0,child.exitValue());assertNull(FrameCodec.read(input));
            assertTrue(new String(child.getErrorStream().readAllBytes(),java.nio.charset.StandardCharsets.UTF_8).contains("synthetic-driver-stdout-marker"));
        } finally {child.destroyForcibly();child.waitFor(3,TimeUnit.SECONDS);}
    }
}
