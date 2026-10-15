package kode.jdbc.drivers;

import kode.jdbc.protocol.RpcException;
import java.io.*;
import java.net.http.*;
import java.nio.file.*;
import java.time.Duration;
import java.util.List;
import java.util.concurrent.*;

/** Vendor-only entries with a page remain manual; no download URL is guessed. */
public final class DriverDownloads {
    private static final ScheduledThreadPoolExecutor DEADLINES=new ScheduledThreadPoolExecutor(1,Thread.ofPlatform().daemon(true).name("jdbc-download-deadline").factory());
    static {DEADLINES.setRemoveOnCancelPolicy(true);}
    private DriverDownloads(){}
    public static List<InstallStore.Source> vendor(DriverSpec spec,Path work,TransferControl control) throws Exception {
        if(spec.downloadUrl().isBlank()) {
            var data=kode.jdbc.protocol.FrameCodec.JSON.createObjectNode().put("manualRequired",true);
            if(!spec.vendorPage().isBlank())data.put("vendorDownloadPage",spec.vendorPage());
            throw new RpcException(-32013,"This driver requires a vendor JAR; download it from the vendor page and select its local classpath",data);
        }
        var uri=DriverSpec.safeUri(spec.downloadUrl());
        if(!List.of("http","https").contains(uri.getScheme()))throw DriverSpec.bad();
        control.check();Path target=work.resolve("vendor-driver.jar");
        ScheduledFuture<?> deadline=DEADLINES.schedule(control::timeout,30,TimeUnit.SECONDS);
        try(HttpClient client=HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(20)).followRedirects(HttpClient.Redirect.NORMAL).build()) {
            var request=HttpRequest.newBuilder(uri).timeout(Duration.ofSeconds(30)).GET().build();
            var response=client.sendAsync(request,HttpResponse.BodyHandlers.ofInputStream());
            try {
                HttpResponse<InputStream> received;
                while(true){control.check();try{received=response.get(50,TimeUnit.MILLISECONDS);break;}catch(TimeoutException waiting){}}
                if(received.statusCode()<200||received.statusCode()>=300){received.body().close();throw new RpcException(-32010,"Vendor download failed with HTTP "+received.statusCode());}
                long expected=received.headers().firstValueAsLong("Content-Length").orElse(-1);
                if(expected>InstallStore.MAX_JAR){received.body().close();throw new RpcException(-32010,"Vendor driver exceeds 1 GiB");}
                try(InputStream input=received.body();var abort=control.abortOnCancel(()->{try{input.close();}catch(IOException ignored){}});OutputStream output=Files.newOutputStream(target,StandardOpenOption.CREATE_NEW)){
                    long count=0;byte[] bytes=new byte[65536];int read;
                    while((read=input.read(bytes))!=-1){control.check();count+=read;if(count>InstallStore.MAX_JAR)throw new RpcException(-32010,"Vendor driver exceeds 1 GiB");output.write(bytes,0,read);control.progress("download",count,expected);}
                    if(expected>=0&&count!=expected)throw new RpcException(-32010,"Vendor driver download was truncated");
                }
            } finally {response.cancel(true);}
        } catch(Exception failure){control.check();throw failure;}finally {deadline.cancel(false);}
        control.check();return List.of(new InstallStore.Source(target,"vendor"));
    }
}
