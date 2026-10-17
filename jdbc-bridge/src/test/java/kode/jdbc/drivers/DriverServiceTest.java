package kode.jdbc.drivers;

import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.*;
import java.net.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import com.sun.net.httpserver.HttpServer;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.io.TempDir;
import static org.junit.jupiter.api.Assertions.*;

@Timeout(15)
class DriverServiceTest {
    @TempDir Path root;
    static ObjectNode request(String id,ObjectNode driver){var request=FrameCodec.JSON.createObjectNode().put("resolutionId",id);request.set("driver",driver);return request;}
    static ObjectNode local(String id,Path... jars){return FrameCodec.JSON.createObjectNode().put("id",id).put("providerId",id).put("driverClass","org.h2.Driver").put("jarPath",String.join(File.pathSeparator,Arrays.stream(jars).map(Path::toString).toList()));}
    static ObjectNode vendor(String url){var driver=FrameCodec.JSON.createObjectNode().put("id","vendor").put("driverClass","org.h2.Driver");driver.putObject("artifact").put("repositoryType","VENDOR_URL").put("downloadUrl",url);return driver;}
    static RpcException failure(CompletionStage<JsonNode> future){var exception=assertThrows(ExecutionException.class,()->future.toCompletableFuture().get(5,TimeUnit.SECONDS));return assertInstanceOf(RpcException.class,exception.getCause());}
    static class Server implements AutoCloseable {
        final HttpServer server=HttpServer.create(new InetSocketAddress(InetAddress.getLoopbackAddress(),0),0);
        final ExecutorService executor=Executors.newFixedThreadPool(2,Thread.ofPlatform().daemon(true).name("jdbc-fixture-http-",0).factory());
        final AtomicInteger requests=new AtomicInteger();final CountDownLatch started=new CountDownLatch(1),release=new CountDownLatch(1),disconnected=new CountDownLatch(1);
        Server(byte[] bytes,boolean block,boolean truncated) throws IOException {
            server.setExecutor(executor);server.createContext("/driver.jar",exchange->{
                requests.incrementAndGet();exchange.sendResponseHeaders(200,bytes.length+(truncated?100:0));
                try(OutputStream output=exchange.getResponseBody()){
                    int first=Math.min(8192,bytes.length);output.write(bytes,0,first);output.flush();started.countDown();
                    if(block)try{release.await(5,TimeUnit.SECONDS);}catch(InterruptedException ignored){Thread.currentThread().interrupt();}
                    if(!truncated)output.write(bytes,first,bytes.length-first);
                }catch(IOException expectedOnCancel){disconnected.countDown();}finally{exchange.close();if(truncated)server.stop(0);}
            });server.start();
        }
        String url(){return "http://"+server.getAddress().getHostString()+":"+server.getAddress().getPort()+"/driver.jar";}
        @Override public void close(){release.countDown();server.stop(0);executor.shutdownNow();}
    }
    @Test void localMultiJarCopiesInspectAndReuseDoNotModifyTheOriginalClasspath() throws Exception {
        Path jar=DriverFixtures.h2(),extra=DriverFixtures.jar(root.resolve("extra.jar"),"extra.txt","synthetic");byte[] original=Files.readAllBytes(extra);
        try(DriverService service=new DriverService(root.resolve("cache"),null)){
            var first=service.resolve(request("local",local("h2-local",jar,extra)),null).toCompletableFuture().get();assertEquals(2,first.path("classpath").size());assertEquals("local-classpath",first.path("source").asText());
            var second=service.resolve(request("reuse",local("h2-local",jar,extra)),null).toCompletableFuture().get();assertTrue(second.path("reused").asBoolean());assertEquals(first.path("classpathDigest"),second.path("classpathDigest"));
            var inspect=FrameCodec.JSON.createObjectNode().put("providerId","h2-local").put("driverClass","org.h2.Driver");inspect.set("classpath",first.path("classpath"));var info=service.inspect(inspect,null).toCompletableFuture().get();assertEquals(first.path("classpathDigest"),info.path("classpathDigest"));assertArrayEquals(original,Files.readAllBytes(extra));
            var incomplete=local("missing",jar,root.resolve("does-not-exist.jar"));assertEquals(-32010,failure(service.resolve(request("missing",incomplete),null)).code);
            try(var files=Files.list(root.resolve("cache"))){assertFalse(files.anyMatch(path->path.getFileName().toString().startsWith(".work-")));}
        }
    }
    @Test void legacyProviderVersionAndJarsAreCopiedReadOnlyAndAliasesShareOneInstall() throws Exception {
        Path legacy=root.resolve("legacy/postgresql/9.9");Files.createDirectories(legacy);Path jar=legacy.resolve("driver.jar");Files.copy(DriverFixtures.h2(),jar);Path properties=legacy.getParent().resolve("resolved.properties");Files.writeString(properties,"synthetic.fixture\\:driver=9.9\n");byte[] before=Files.readAllBytes(properties);
        var base=FrameCodec.JSON.createObjectNode().put("id","postgresql").put("providerId","postgresql").put("driverClass","org.h2.Driver").put("coordinates","synthetic.fixture:driver");
        try(DriverService service=new DriverService(root.resolve("cache"),root.resolve("legacy"))){
            var installed=service.resolve(request("legacy",base),null).toCompletableFuture().get();assertEquals("legacy-classpath",installed.path("source").asText());assertEquals("9.9",installed.path("resolvedVersion").asText());
            var alias=base.deepCopy().put("id","cockroachdb");var reused=service.resolve(request("alias",alias),null).toCompletableFuture().get();assertTrue(reused.path("reused").asBoolean());assertEquals(installed.path("manifestPath"),reused.path("manifestPath"));
            assertArrayEquals(before,Files.readAllBytes(properties));assertTrue(Files.isRegularFile(jar));
        }
    }
    @Test void vendorPageWithoutDownloadUrlIsManualAndDoesNotCreateAnInstallRoot() throws Exception {
        Path cache=root.resolve("cache");var driver=FrameCodec.JSON.createObjectNode().put("id","vendor").put("driverClass","");driver.putObject("artifact").put("repositoryType","VENDOR_URL").put("vendorDownloadPage","https://example.invalid/synthetic-vendor-page");
        try(DriverService service=new DriverService(cache,null)){RpcException error=failure(service.resolve(request("manual",driver),null));assertEquals(-32013,error.code);assertTrue(error.data.path("manualRequired").asBoolean());assertTrue(error.data.path("notStarted").asBoolean());assertFalse(Files.exists(cache));}
    }
    @Test void readOnlySpecInspectionKeepsBuiltinVersionTruthfulAndNeverDownloadsMissingDrivers() throws Exception {
        Path cache=root.resolve("cache");try(DriverService service=new DriverService(cache,null)) {
            var spec=FrameCodec.JSON.createObjectNode().put("id","h2").put("driverClass","org.h2.Driver");var params=FrameCodec.JSON.createObjectNode();params.set("driver",spec);
            var builtin=service.inspect(params,null).toCompletableFuture().get();assertEquals("application-classpath",builtin.path("source").asText());assertEquals("2.2.224",builtin.path("resolvedVersion").asText());assertFalse(Files.exists(cache));
            spec.putObject("artifact").put("version","999.0");RpcException missing=assertThrows(RpcException.class,()->service.inspect(params,null));assertEquals(-32014,missing.code);assertFalse(Files.exists(cache));
        }
    }
    @Test void installedLocalClasspathRemainsUsableAfterTheOriginalImportFilesAreRemoved() throws Exception {
        Path original=root.resolve("original.jar");Files.copy(DriverFixtures.h2(),original);ObjectNode driver=local("local-h2",original);
        try(DriverService service=new DriverService(root.resolve("cache"),null)){
            var installed=service.resolve(request("import",driver),null).toCompletableFuture().get();Files.delete(original);
            var params=FrameCodec.JSON.createObjectNode();params.set("driver",driver);var info=service.inspect(params,null).toCompletableFuture().get();assertEquals(installed.path("classpathDigest"),info.path("classpathDigest"));assertEquals(installed.path("classpath"),info.path("classpath"));
            assertTrue(service.resolve(request("reuse-import",driver),null).toCompletableFuture().get().path("reused").asBoolean());
        }
    }
    @Test void successfulVendorDownloadPublishesAReusableClasspath() throws Exception {
        try(Server server=new Server(Files.readAllBytes(DriverFixtures.h2()),false,false);DriverService service=new DriverService(root.resolve("cache"),null)){
            var driver=vendor(server.url());var installed=service.resolve(request("vendor",driver),null).toCompletableFuture().get(5,TimeUnit.SECONDS);assertEquals("vendor",installed.path("source").asText());
            assertTrue(service.resolve(request("reuse",driver),null).toCompletableFuture().get().path("reused").asBoolean());assertEquals(1,server.requests.get());
        }
    }
    @Test void independentCancellationAbortsAStalledBodyAndCleansOnlyItsTemporaryInstall() throws Exception {
        try(Server server=new Server(Files.readAllBytes(DriverFixtures.h2()),true,false);DriverService service=new DriverService(root.resolve("cache"),null)){
            var future=service.resolve(request("cancel",vendor(server.url())),null);assertTrue(server.started.await(3,TimeUnit.SECONDS));assertEquals("requested",service.cancel("cancel").toCompletableFuture().get().path("status").asText());server.release.countDown();
            assertEquals(-32800,failure(future).code);assertTrue(server.disconnected.await(2,TimeUnit.SECONDS),"cancel closes stalled response body");try(var files=Files.list(root.resolve("cache"))){assertEquals(0,files.count());}
            assertEquals("alreadyFinished",service.cancel("cancel").toCompletableFuture().get().path("status").asText());
        }
    }
    @Test void truncatedVendorDownloadNeverPublishesAnInstall() throws Exception {
        try(Server server=new Server(Files.readAllBytes(DriverFixtures.h2()),false,true);DriverService service=new DriverService(root.resolve("cache"),null)){
            assertEquals(-32010,failure(service.resolve(request("truncated",vendor(server.url())),null)).code);try(var files=Files.list(root.resolve("cache"))){assertEquals(0,files.count());}
        }
    }
}
