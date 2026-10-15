package kode.jdbc.protocol;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Map;
import java.util.TreeSet;
import java.util.UUID;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Semaphore;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/** Stdio reader, independent bounded control/work dispatch and one output writer. */
public final class RpcServer implements AutoCloseable {
    @FunctionalInterface public interface Handler { CompletionStage<JsonNode> call(ObjectNode params, Context context) throws Exception; }
    public final class Context {
        public final String requestId;
        private Context(String id) { requestId = id; }
        public boolean stopped() { return stopped.get(); }
        /** Query producers use this same FIFO as their own final response. */
        public boolean notify(String method, ObjectNode params, java.util.function.BooleanSupplier cancelled) throws Exception {
            ObjectNode message = FrameCodec.JSON.createObjectNode().put("jsonrpc", "2.0").put("method", method);
            message.set("params", params);
            return output.send(message, false, () -> stopped() || cancelled.getAsBoolean());
        }
    }
    private record Method(boolean control, Handler handler) {}
    private final Map<String,Method> methods = new ConcurrentHashMap<>();
    private final Map<String,Boolean> active = new ConcurrentHashMap<>();
    private final Semaphore bodyBytes = new Semaphore(32 * 1024 * 1024);
    private final Semaphore controlBytes = new Semaphore(1024 * 1024);
    private final Semaphore calls = new Semaphore(64), controlCalls = new Semaphore(16);
    private final ThreadPoolExecutor workers = pool("jdbc-dispatch", 4, 32);
    private final ThreadPoolExecutor controls = pool("jdbc-control", 2, 16);
    // Active request permits remain held until a reply is queued, bounding this
    // executor to 64 ordinary completions. A slow pipe must not pin a JDBC owner.
    private final java.util.concurrent.ExecutorService replies = java.util.concurrent.Executors.newVirtualThreadPerTaskExecutor();
    private final CountDownLatch done = new CountDownLatch(1);
    private final AtomicBoolean stopped = new AtomicBoolean();
    private final AtomicBoolean closed = new AtomicBoolean();
    private final java.util.List<AutoCloseable> closeActions=new java.util.ArrayList<>();
    private final DataInputStream input;
    private final Outbound output;
    private final String epoch = UUID.randomUUID().toString();
    private volatile boolean initialized;
    private volatile int exitCode;
    private volatile boolean started;
    private Thread reader;

    public RpcServer(InputStream input, OutputStream output) {
        this.input = new DataInputStream(input);
        this.output = new Outbound(new DataOutputStream(output), this::fatal);
    }
    private static ThreadPoolExecutor pool(String name, int threads, int queued) {
        return new ThreadPoolExecutor(threads, threads, 0, TimeUnit.MILLISECONDS, new ArrayBlockingQueue<>(queued),
            Thread.ofPlatform().daemon(true).name(name+"-",0).factory(), new ThreadPoolExecutor.AbortPolicy());
    }
    public void register(String name, boolean control, Handler handler) {
        if (started || name.equals("hello") || name.equals("ping") || name.equals("shutdown")
            || methods.putIfAbsent(name, new Method(control,handler)) != null) throw new IllegalStateException("Duplicate or late method registration");
    }
    public static CompletionStage<JsonNode> result(JsonNode value) { return CompletableFuture.completedFuture(value); }
    public void onClose(AutoCloseable action){if(started||closeActions.size()>=16)throw new IllegalStateException("Late or excessive close action");closeActions.add(action);}
    public int serve() throws InterruptedException {
        started = true; output.start();
        reader = Thread.ofPlatform().daemon(true).name("jdbc-protocol-reader").start(this::readLoop);
        done.await(); close(); return exitCode;
    }
    private void fatal() { exitCode = 2; stopped.set(true); done.countDown(); }
    private void readLoop() {
        try {
            while (!stopped.get()) {
                FrameCodec.Message frame = FrameCodec.read(input);
                if (frame == null) { stopped.set(true); done.countDown(); return; }
                ObjectNode message = frame.value();
                if (!message.has("method") || !message.has("id")) throw new FrameCodec.ProtocolException("Client messages must be requests");
                String id = message.get("id").textValue(), method = message.get("method").textValue();
                if (active.containsKey(id)) throw new FrameCodec.ProtocolException("Duplicate active request ID");
                ObjectNode params = message.has("params") ? (ObjectNode) message.get("params") : FrameCodec.JSON.createObjectNode();
                if (method.equals("hello")) { hello(id, params); continue; }
                if (!initialized) { error(id,-32002,"Hello must be the first request",null,true); continue; }
                if (method.equals("ping")) { respond(id,FrameCodec.JSON.createObjectNode().put("status","ok").put("helperEpoch",epoch),true); continue; }
                if (method.equals("shutdown")) {
                    respond(id,FrameCodec.JSON.createObjectNode().put("closing",true),true);
                    stopped.set(true); done.countDown(); return;
                }
                Method target = methods.get(method);
                if (target == null) { error(id,-32601,"Unknown method",null,true); continue; }
                Semaphore bytes = target.control() ? controlBytes : bodyBytes;
                Semaphore slots = target.control() ? controlCalls : calls;
                boolean slot = slots.tryAcquire(), budget = slot && bytes.tryAcquire(frame.bytes());
                if (!budget) {
                    if (slot) slots.release();
                    error(id,-32003,"Helper request capacity exhausted",FrameCodec.JSON.createObjectNode().put("notStarted",true),true); continue;
                }
                if (active.putIfAbsent(id,true) != null) { bytes.release(frame.bytes()); slots.release(); throw new FrameCodec.ProtocolException("Duplicate active request ID"); }
                Runnable release = () -> { active.remove(id); bytes.release(frame.bytes()); slots.release(); };
                try {
                    (target.control() ? controls : workers).execute(() -> dispatch(id,params,target,release));
                } catch (java.util.concurrent.RejectedExecutionException failure) {
                    release.run(); error(id,-32003,"Helper work queue exhausted",FrameCodec.JSON.createObjectNode().put("notStarted",true),true);
                }
            }
        } catch (Exception failure) {
            // JSON/JDBC data must never be emitted as diagnostic text.
            System.err.println("JDBC bridge input/protocol failure ("+failure.getClass().getSimpleName()+")");
            fatal();
        }
    }
    private void hello(String id,ObjectNode params) throws Exception {
        JsonNode version = params.get("protocolVersion");
        if (initialized) { error(id,-32002,"Hello already completed",null,true); return; }
        if (version == null || !version.isIntegralNumber() || !version.canConvertToInt()) { error(id,-32602,"Missing/invalid protocolVersion",null,true); return; }
        if (version.intValue() != 1) { error(id,-32001,"Incompatible JDBC protocol version",null,true); return; }
        if (Runtime.version().feature() < 21) { error(id,-32001,"Java 21 or newer is required",null,true); return; }
        ObjectNode value = FrameCodec.JSON.createObjectNode().put("protocolVersion",1).put("helperVersion","0.1.0")
            .put("javaVersion",Runtime.version().toString()).put("helperEpoch",epoch).put("maxFrameBytes",FrameCodec.MAX_FRAME_BYTES);
        var capabilities = value.putArray("capabilities");
        var names = new TreeSet<>(methods.keySet()); names.addAll(java.util.List.of("hello","ping","shutdown"));
        names.forEach(capabilities::add);
        respond(id,value,true); initialized = true;
    }
    private void dispatch(String id,ObjectNode params,Method target,Runnable release) {
        try {
            target.handler().call(params,new Context(id)).whenComplete((value,failure) -> {
                finish(id,value,failure,target.control(),release);
            });
        } catch (Throwable failure) {
            finish(id,null,failure,target.control(),release);
        }
    }
    private void finish(String id,JsonNode value,Throwable failure,boolean control,Runnable release) {
        Runnable reply=()->{
            try {if(failure==null)respond(id,value,control);else operationError(id,failure,control);}
            catch(Exception exception){if(!stopped.get())fatal();}
            finally{release.run();}
        };
        if(control){reply.run();return;}
        try{replies.execute(reply);}
        catch(java.util.concurrent.RejectedExecutionException rejected){release.run();if(!stopped.get())fatal();}
    }
    private void operationError(String id,Throwable failure,boolean priority) throws Exception {
        while (failure instanceof java.util.concurrent.CompletionException && failure.getCause()!=null) failure=failure.getCause();
        if (failure instanceof RpcException rpc) error(id,rpc.code,rpc.getMessage(),rpc.data,priority);
        else if (failure instanceof IllegalArgumentException) error(id,-32602,"Invalid parameters",null,priority);
        else error(id,-32603,"Operation failed",FrameCodec.JSON.createObjectNode().put("exceptionType",failure.getClass().getSimpleName()),priority);
    }
    private void respond(String id,JsonNode result,boolean priority) throws Exception {
        ObjectNode response = FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id",id);
        response.set("result",result == null ? FrameCodec.JSON.nullNode() : result);
        output.send(response,priority,stopped::get);
    }
    private void error(String id,int code,String message,JsonNode data,boolean priority) throws Exception {
        ObjectNode response = FrameCodec.JSON.createObjectNode().put("jsonrpc","2.0").put("id",id);
        ObjectNode error = response.putObject("error").put("code",code).put("message",message);
        if (data != null) error.set("data",data);
        output.send(response,priority,stopped::get);
    }
    @Override public void close() {
        if(!closed.compareAndSet(false,true))return;
        stopped.set(true); workers.shutdownNow(); controls.shutdownNow(); replies.shutdownNow();
        try { workers.awaitTermination(500,TimeUnit.MILLISECONDS); controls.awaitTermination(500,TimeUnit.MILLISECONDS); replies.awaitTermination(500,TimeUnit.MILLISECONDS); }
        catch (InterruptedException failure) { Thread.currentThread().interrupt(); }
        for(int index=closeActions.size()-1;index>=0;index--)try{closeActions.get(index).close();}catch(Exception failure){System.err.println("JDBC component close failed ("+failure.getClass().getSimpleName()+")");}
        output.close();
        try { input.close(); } catch (Exception ignored) { }
    }
}
