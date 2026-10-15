package kode.jdbc.protocol;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import java.io.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicBoolean;
import static org.junit.jupiter.api.Assertions.*;

@Timeout(10)
class RpcServerTest {
    static ObjectNode request(String id,String method,ObjectNode params) {
        ObjectNode value=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id",id).put("method",method);
        value.set("params",params);return value;
    }
    static ObjectNode empty() {return FrameCodec.JSON.createObjectNode();}
    static class Peer implements AutoCloseable {
        final PipedOutputStream send=new PipedOutputStream();
        final DataInputStream receive;
        final RpcServer server;
        final FutureTask<Integer> task;
        Peer() throws IOException {
            PipedInputStream input=new PipedInputStream(send,65536);
            PipedOutputStream output=new PipedOutputStream();receive=new DataInputStream(new PipedInputStream(output,65536));
            server=new RpcServer(input,output);task=new FutureTask<>(server::serve);
        }
        void start() {Thread.ofPlatform().daemon(true).start(task);}
        void send(String id,String method,ObjectNode params) throws IOException {FrameCodec.write(new DataOutputStream(send),FrameCodec.encode(request(id,method,params)));}
        ObjectNode read() throws IOException {return FrameCodec.read(receive).value();}
        void hello() throws IOException {send("hello","hello",empty().put("protocolVersion",1));assertEquals(1,read().path("result").path("protocolVersion").intValue());}
        @Override public void close() throws Exception {send.close();server.close();receive.close();}
    }
    @Test void blockedWorkersCannotPreventCancellationPingOrShutdown() throws Exception {
        try (Peer peer=new Peer()) {
            CountDownLatch entered=new CountDownLatch(4),release=new CountDownLatch(1);
            peer.server.register("fake.block",false,(params,context)->{entered.countDown();release.await();return RpcServer.result(empty().put("finished",true));});
            peer.server.register("query.cancel",true,(params,context)->RpcServer.result(empty().put("status","requested")));
            peer.start();peer.hello();
            for(int i=0;i<5;i++) peer.send("block-"+i,"fake.block",empty());
            assertTrue(entered.await(2,TimeUnit.SECONDS));
            peer.send("cancel","query.cancel",empty().put("executionId","blocked"));assertEquals("requested",peer.read().path("result").path("status").asText());
            peer.send("ping","ping",empty());assertEquals("ok",peer.read().path("result").path("status").asText());
            peer.send("shutdown","shutdown",empty());assertTrue(peer.read().path("result").path("closing").asBoolean());
            assertEquals(0,peer.task.get(3,TimeUnit.SECONDS));release.countDown();
        }
    }
    @Test void helloVersionUnknownMethodOutOfOrderCompletionsAndSafeErrorBodies() throws Exception {
        try(Peer peer=new Peer()) {
            CompletableFuture<JsonNode> slow=new CompletableFuture<>();
            peer.server.register("fake.slow",false,(params,context)->slow);
            peer.server.register("fake.error",false,(params,context)->{throw new IOException("synthetic-password-marker");});
            peer.start();peer.send("pre","ping",empty());assertEquals(-32002,peer.read().path("error").path("code").intValue());
            peer.send("wrong","hello",empty().put("protocolVersion",99));assertEquals(-32001,peer.read().path("error").path("code").intValue());
            peer.hello();peer.send("slow","fake.slow",empty());peer.send("unknown","future.optional.method",empty());assertEquals(-32601,peer.read().path("error").path("code").intValue());
            peer.send("error","fake.error",empty());ObjectNode error=peer.read();assertFalse(error.toString().contains("synthetic-password-marker"));assertEquals(-32603,error.path("error").path("code").intValue());
            slow.complete(empty().put("text","é界😀"));assertEquals("slow",peer.read().path("id").asText());
            peer.send("shutdown","shutdown",empty());peer.read();assertEquals(0,peer.task.get(3,TimeUnit.SECONDS));
        }
    }
    @Test void boundedOutputWaitIsCancellableControlHasPriorityAndOwnFinalFollowsRows() throws Exception {
        CountDownLatch writing=new CountDownLatch(1),release=new CountDownLatch(1);
        ByteArrayOutputStream bytes=new ByteArrayOutputStream();AtomicBoolean failed=new AtomicBoolean(),cancelled=new AtomicBoolean();
        OutputStream blocked=new OutputStream() {
            @Override public void write(int b) throws IOException {gate();bytes.write(b);}
            @Override public void write(byte[] b,int off,int len) throws IOException {gate();bytes.write(b,off,len);}
            void gate() throws IOException {writing.countDown();try {if(!release.await(3,TimeUnit.SECONDS))throw new IOException("test output deadline");}catch(InterruptedException e){throw new IOException(e);}}
        };
        try(Outbound output=new Outbound(new DataOutputStream(blocked),()->failed.set(true))) {
            output.start();
            ObjectNode row=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("method","query.event");row.set("params",empty().put("sequence",0));
            assertTrue(output.send(row,false,()->false));assertTrue(writing.await(1,TimeUnit.SECONDS));
            for(int i=1;i<=31;i++){ObjectNode next=row.deepCopy();next.withObject("params").put("sequence",i);assertTrue(output.send(next,false,()->false));}
            ObjectNode last=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id","execute");last.set("result",empty().put("outcome","completed"));assertTrue(output.send(last,false,()->false));
            FutureTask<Boolean> waiting=new FutureTask<>(()->output.send(row,false,cancelled::get));Thread.ofPlatform().daemon(true).start(waiting);
            Thread.sleep(30);assertFalse(waiting.isDone());cancelled.set(true);assertFalse(waiting.get(1,TimeUnit.SECONDS));
            ObjectNode control=FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id","cancel");control.set("result",empty().put("status","requested"));assertTrue(output.send(control,true,()->false));release.countDown();
        }
        assertFalse(failed.get());DataInputStream input=new DataInputStream(new ByteArrayInputStream(bytes.toByteArray()));
        assertEquals(0,FrameCodec.read(input).value().path("params").path("sequence").intValue());assertEquals("cancel",FrameCodec.read(input).value().path("id").asText());
        for(int i=1;i<=31;i++)assertEquals(i,FrameCodec.read(input).value().path("params").path("sequence").intValue());
        assertEquals("execute",FrameCodec.read(input).value().path("id").asText());assertNull(FrameCodec.read(input));
    }
}
