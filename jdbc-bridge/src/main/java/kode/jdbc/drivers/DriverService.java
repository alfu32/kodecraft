package kode.jdbc.drivers;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import kode.jdbc.protocol.RpcServer;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicBoolean;

/** Feature handlers return stages immediately; JDBC dispatch/input remain free
 * while the separately bounded resolver workers download and publish artifacts.
 */
public final class DriverService implements AutoCloseable {
    private final InstallStore store;
    private final Path legacyRoot;
    private final MavenArtifacts maven=new MavenArtifacts();
    public final LoaderCatalog loaders=new LoaderCatalog();
    private final Map<String,Job> active=new ConcurrentHashMap<>();
    private final AtomicBoolean closing=new AtomicBoolean();
    private final ThreadPoolExecutor workers=new ThreadPoolExecutor(2,2,0,TimeUnit.MILLISECONDS,new ArrayBlockingQueue<>(8),
        Thread.ofPlatform().daemon(true).name("jdbc-driver-install-",0).factory(),new ThreadPoolExecutor.AbortPolicy());
    public DriverService(Path root,Path legacyRoot){store=root==null?null:new InstallStore(root);this.legacyRoot=legacyRoot==null?null:legacyRoot.toAbsolutePath().normalize();}
    public static DriverService fromEnvironment(){return new DriverService(environment("KODE_RS_JDBC_HOME"),environment("KODE_LEGACY_JDBC_HOME"));}
    private static Path environment(String key){String value=System.getenv(key);return value==null||value.isBlank()?null:Path.of(value);}
    public void register(RpcServer server){
        server.register("driver.resolve",false,this::resolve);server.register("driver.inspect",false,this::inspect);
        server.register("driver.cancel",true,(params,context)->cancel(id(params,"resolutionId")));server.onClose(this);
    }
    public CompletionStage<JsonNode> resolve(ObjectNode params,RpcServer.Context context) throws Exception {
        String id=id(params,"resolutionId");
        if(!(params.get("driver") instanceof ObjectNode value))throw DriverSpec.bad();
        DriverSpec spec=DriverSpec.parse(value);
        if(closing.get())throw new RpcException(-32010,"Driver service is stopping");
        Job job=new Job(id,spec,context);if(active.putIfAbsent(id,job)!=null)throw new RpcException(-32602,"Resolution ID is already active");
        try{workers.execute(job);}catch(RejectedExecutionException failure){active.remove(id,job);throw new RpcException(-32003,"Driver resolution queue is full",FrameCodec.JSON.createObjectNode().put("notStarted",true));}
        return job.future;
    }
    public CompletionStage<JsonNode> cancel(String id){Job job=active.get(id);if(job!=null)job.control.cancel();return RpcServer.result(FrameCodec.JSON.createObjectNode().put("status",job==null?"alreadyFinished":"requested"));}
    public CompletionStage<JsonNode> inspect(ObjectNode params,RpcServer.Context context) throws Exception {
        String provider=id(params,"providerId"),selected=DriverSpec.text(params,"driverClass","");
        if(!params.path("classpath").isArray()||params.path("classpath").size()>InstallStore.MAX_JARS)throw DriverSpec.bad();
        List<Path> paths=new ArrayList<>();for(JsonNode path:params.path("classpath")){if(!path.isTextual()||path.textValue().isBlank())throw DriverSpec.bad();paths.add(Path.of(path.textValue()).toAbsolutePath().normalize());}
        var control=new TransferControl("inspect",()->closing.get()||(context!=null&&context.stopped()),null);
        ObjectNode result=loaders.inspect(provider,paths,selected,control);var classpath=result.putArray("classpath");paths.forEach(path->classpath.add(path.toString()));return RpcServer.result(result);
    }
    static String id(ObjectNode value,String key) throws RpcException {
        String id=DriverSpec.text(value,key,"");if(id.isBlank()||id.getBytes(java.nio.charset.StandardCharsets.UTF_8).length>128||id.codePoints().anyMatch(Character::isISOControl))throw DriverSpec.bad();return id;
    }
    private final class Job implements Runnable {
        final String id;final DriverSpec spec;final TransferControl control;final CompletableFuture<JsonNode> future=new CompletableFuture<>();final AtomicBoolean started=new AtomicBoolean();
        Job(String id,DriverSpec spec,RpcServer.Context context){this.id=id;this.spec=spec;control=new TransferControl(id,()->closing.get()||(context!=null&&context.stopped()),context);}
        @Override public void run(){
            if(!started.compareAndSet(false,true))return;
            control.attach();Path work=null;ObjectNode success=null;Throwable failure=null;boolean cleanupWarning=false;
            try {
                control.check();
                if(spec.localPaths().isEmpty()&&spec.repositoryType().equals("VENDOR_URL")&&spec.downloadUrl().isBlank())DriverDownloads.vendor(spec,Path.of("."),control);
                if(spec.localPaths().isEmpty()&&spec.repositoryType().equals("LOCAL_FILE"))throw new RpcException(-32602,"Select one or more local JDBC JARs");
                if(store==null)throw new RpcException(-32010,"Driver install root is missing; configure KODE_RS_JDBC_HOME");
                InstallStore.Installed cached=store.cached(spec,control);
                if(cached!=null){success=cached.wire();success.put("reused",true);}
                else {
                    work=store.temporary();List<InstallStore.Source> sources;String version=spec.version(),source;
                    if(!spec.localPaths().isEmpty()){sources=spec.localPaths().stream().map(path->new InstallStore.Source(path,"local")).toList();source="local-classpath";if(version.isBlank())version="local";}
                    else if(spec.repositoryType().equals("VENDOR_URL")){sources=DriverDownloads.vendor(spec,work,control);source="vendor";if(version.isBlank())version="vendor";}
                    else {
                        String legacyVersion=version.isBlank()?legacyVersion(spec):version;
                        sources=legacyJars(spec,legacyVersion);
                        if(!sources.isEmpty()){source="legacy-classpath";version=legacyVersion==null?"legacy":legacyVersion;}
                        else {var resolved=maven.resolve(spec,legacyVersion,work,control);sources=resolved.sources();version=resolved.version();source="maven";}
                    }
                    control.check();control.progress("publish",0,-1);
                    success=store.publish(spec,version,source,sources,work,control).wire();success.put("reused",false);
                    // Publication has happened. A late cancel must not change a
                    // real successful install into a fictitious rolled-back one.
                }
            }catch(Throwable exception){failure=exception;}
            finally {
                control.detach();
                if(work!=null)try{InstallStore.cleanup(work);}catch(IOException exception){cleanupWarning=true;}
                active.remove(id,this);
            }
            if(success!=null){if(cleanupWarning)success.put("cleanupWarning",true);future.complete(success);}
            else {
                RpcException error;
                if(failure instanceof RpcException rpc)error=rpc;
                else if(control.cancelled())error=new RpcException(-32800,"Driver resolution cancelled");
                else error=new RpcException(-32010,"Driver installation failed; check the source, classpath and available disk space");
                if(cleanupWarning){ObjectNode data=error.data instanceof ObjectNode object?object.deepCopy():FrameCodec.JSON.createObjectNode();data.put("cleanupWarning",true);error=new RpcException(error.code,error.getMessage(),data);}
                future.completeExceptionally(error);
            }
        }
        void cancelQueued(){control.cancel();if(started.compareAndSet(false,true)){active.remove(id,this);future.completeExceptionally(new RpcException(-32800,"Driver service stopped before resolution started"));}}
    }
    private Path legacyProvider(DriverSpec spec) {
        if(legacyRoot==null||!spec.providerId().matches("[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}")||spec.providerId().equals(".")||spec.providerId().equals(".."))return null;
        return legacyRoot.resolve(spec.providerId());
    }
    private String legacyVersion(DriverSpec spec) throws Exception {
        Path provider=legacyProvider(spec);if(provider==null)return null;Path file=provider.resolve("resolved.properties");if(!Files.isRegularFile(file))return null;
        if(Files.size(file)>1024*1024)throw new RpcException(-32010,"Legacy driver version file exceeds its size limit; original retained");
        Properties values=new Properties();try(InputStream stream=Files.newInputStream(file)){byte[] bytes=stream.readNBytes(1024*1024+1);if(bytes.length>1024*1024)throw new IOException("Legacy version file grew past its limit");values.load(new ByteArrayInputStream(bytes));}
        String key=spec.groupId()+":"+spec.artifactId()+(spec.classifier().isBlank()?"":":"+spec.classifier());String version=values.getProperty(key);
        if(version!=null&&!version.matches("[A-Za-z0-9_][A-Za-z0-9_.+-]{0,255}"))throw new RpcException(-32010,"Legacy resolved driver version is invalid; original retained");return version;
    }
    private List<InstallStore.Source> legacyJars(DriverSpec spec,String version) throws Exception {
        Path provider=legacyProvider(spec);if(provider==null)return List.of();Path selected=version==null?provider:provider.resolve(version);
        if(!Files.isDirectory(selected))return List.of();
        // Without a persisted/explicit version, never mix arbitrary version
        // subdirectories into one classpath as the source's broad walk could.
        try(var stream=Files.walk(selected,version==null?1:8)) {
            List<Path> paths=stream.filter(path->Files.isRegularFile(path)&&path.getFileName().toString().endsWith(".jar")).sorted().limit(InstallStore.MAX_JARS+1L).toList();
            if(paths.size()>InstallStore.MAX_JARS)throw new RpcException(-32010,"Legacy driver classpath exceeds 256 JARs");
            return paths.stream().map(path->new InstallStore.Source(path,"legacy")).toList();
        }
    }
    @Override public void close(){
        if(!closing.compareAndSet(false,true))return;active.values().forEach(job->job.control.cancel());
        for(Runnable pending:workers.shutdownNow())if(pending instanceof Job job)job.cancelQueued();
        try{workers.awaitTermination(500,TimeUnit.MILLISECONDS);}catch(InterruptedException failure){Thread.currentThread().interrupt();}
        loaders.close();maven.close();
    }
}
