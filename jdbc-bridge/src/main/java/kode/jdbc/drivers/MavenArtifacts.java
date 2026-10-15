package kode.jdbc.drivers;

import kode.jdbc.protocol.RpcException;
import org.eclipse.aether.RepositorySystem;
import org.eclipse.aether.artifact.DefaultArtifact;
import org.eclipse.aether.collection.CollectRequest;
import org.eclipse.aether.graph.Dependency;
import org.eclipse.aether.repository.RemoteRepository;
import org.eclipse.aether.resolution.DependencyRequest;
import org.eclipse.aether.resolution.VersionRangeRequest;
import org.eclipse.aether.supplier.RepositorySystemSupplier;
import org.eclipse.aether.supplier.SessionBuilderSupplier;
import org.eclipse.aether.transfer.*;
import org.eclipse.aether.util.artifact.JavaScopes;
import org.eclipse.aether.util.filter.DependencyFilterUtils;
import java.nio.file.Path;
import java.util.*;

/** Real Maven model/graph resolution, never a hand-written POM parser or CLI. */
public final class MavenArtifacts implements AutoCloseable {
    public record Resolved(String version,List<InstallStore.Source> sources) {}
    private RepositorySystem system;
    private boolean closed;
    private synchronized RepositorySystem system(){
        if(closed)throw new IllegalStateException("Maven resolver is closed");
        if(system==null)system=new RepositorySystemSupplier().get();return system;
    }
    public Resolved resolve(DriverSpec spec,String legacyVersion,Path work,TransferControl control) throws Exception {
        control.check();
        if(spec.groupId().isBlank()||spec.artifactId().isBlank())throw new RpcException(-32602,"Maven groupId and artifactId are required");
        RepositorySystem system=system();Limits transfers=new Limits(control);
        var builder=new SessionBuilderSupplier(system).get().withLocalRepositoryBaseDirectories(work.resolve("maven"))
            .setTransferListener(transfers).setConfigProperty("aether.connector.connectTimeout",20_000)
            .setConfigProperty("aether.connector.requestTimeout",30_000);
        // Do not expose arbitrary environment/Maven user secrets to downloaded
        // POM property interpolation. Keep only ordinary platform activation data.
        Map<String,String> platform=new HashMap<>();
        for(String key:List.of("java.version","java.vendor","os.name","os.arch","os.version"))platform.put(key,System.getProperty(key,""));
        builder.setSystemProperties(platform).setUserProperties(Map.of());
        try(var session=builder.build()) {
            String repository=spec.repositoryType().equals("MAVEN_CUSTOM")?spec.repository():DriverSpec.CENTRAL;
            if(repository.isBlank())throw new RpcException(-32602,"A custom Maven repository URL is required");
            DriverSpec.safeUri(repository);
            List<RemoteRepository> repositories=List.of(new RemoteRepository.Builder("kode-driver","default",repository).build());
            String version=spec.version().isBlank()?legacyVersion:spec.version();
            if(version==null||version.isBlank()) {
                control.progress("resolve-version",0,-1);
                var range=new VersionRangeRequest(new DefaultArtifact(spec.groupId(),spec.artifactId(),spec.classifier(),"jar","[0,)"),repositories,"kode-jdbc");
                version=system.resolveVersionRange(session,range).getVersions().stream().map(Object::toString)
                    .filter(MavenArtifacts::stable).max(MavenArtifacts::compareVersions).orElseThrow(()->new RpcException(-32010,"No stable Maven version was found"));
            }
            control.check();control.progress("resolve-graph",0,-1);
            var artifact=new DefaultArtifact(spec.groupId(),spec.artifactId(),spec.classifier(),"jar",version);
            var collect=new CollectRequest(new Dependency(artifact,JavaScopes.RUNTIME),repositories).setRequestContext("kode-jdbc");
            var result=system.resolveDependencies(session,new DependencyRequest(collect,DependencyFilterUtils.classpathFilter(JavaScopes.RUNTIME)));
            control.check();
            if(!result.getCollectExceptions().isEmpty()||result.getArtifactResults().isEmpty()||result.getArtifactResults().size()>InstallStore.MAX_JARS)
                throw new RpcException(-32010,"Maven runtime graph is incomplete or exceeds 256 artifacts");
            var artifacts=new ArrayList<>(result.getArtifactResults());
            java.util.function.Predicate<org.eclipse.aether.artifact.Artifact> root=entry->entry.getGroupId().equals(spec.groupId())&&entry.getArtifactId().equals(spec.artifactId())&&entry.getClassifier().equals(spec.classifier())&&entry.getExtension().equals("jar");
            artifacts.sort(Comparator.comparingInt(entry->root.test(entry.getArtifact())?0:1));
            List<InstallStore.Source> sources=new ArrayList<>();String resolved=null;
            for(var entry:artifacts){
                control.check();if(!entry.isResolved()||entry.getArtifact().getPath()==null)throw new RpcException(-32010,"Maven runtime graph contains an unresolved artifact");
                if(!entry.getArtifact().getExtension().equals("jar"))continue;
                if(root.test(entry.getArtifact()))resolved=entry.getArtifact().getVersion();
                sources.add(new InstallStore.Source(entry.getArtifact().getPath(),entry.getArtifact().toString()));
            }
            if(resolved==null)throw new RpcException(-32010,"Maven runtime graph did not include its root driver");
            return new Resolved(resolved,List.copyOf(sources));
        } catch(RpcException failure){throw failure;}
        catch(Exception failure){
            control.check();
            if(transfers.limitExceeded)throw new RpcException(-32010,"Maven download exceeds artifact, graph or byte limits");
            RpcException error=new RpcException(-32010,"Maven driver resolution failed; check the coordinates and repository");
            // Internal cause supports synthetic fixture debugging. RpcServer
            // serializes only the sanitized outer message/data, never causes.
            error.initCause(failure);throw error;
        }
    }
    static boolean stable(String version) {
        String upper=version.toUpperCase(Locale.ROOT);
        return !version.isBlank()&&List.of("SNAPSHOT","ALPHA","BETA","MILESTONE","PREVIEW","RC","-EA",".EA","-DEV",".DEV").stream().noneMatch(upper::contains);
    }
    /** Preserve the source's latest-stable selection order; Maven resolves the
     * graph and version metadata, while this policy chooses among its versions. */
    static int compareVersions(String left,String right) {
        String[] a=left.split("[.-]",-1),b=right.split("[.-]",-1);
        for(int i=0;i<Math.max(a.length,b.length);i++) {
            String x=i<a.length?a[i]:"",y=i<b.length?b[i]:"";Long xn=number(x),yn=number(y);int result;
            if(xn!=null&&yn!=null)result=Long.compare(xn,yn);else if(xn!=null)result=1;else if(yn!=null)result=-1;
            else {result=Integer.compare(rank(x),rank(y));if(result==0)result=x.compareToIgnoreCase(y);}
            if(result!=0)return result;
        }
        return 0;
    }
    private static Long number(String value){try{return Long.valueOf(value);}catch(NumberFormatException e){return null;}}
    private static int rank(String value){return switch(value.toLowerCase(Locale.ROOT)){case "snapshot"->-5;case "alpha","a"->-4;case "beta","b"->-3;case "rc"->-2;case "final","ga","release"->1;default->0;};}
    private static final class Limits extends AbstractTransferListener {
        private final TransferControl control;
        private final Map<TransferResource,Long> received=new IdentityHashMap<>();
        private long bytes;private int attempts;volatile boolean limitExceeded;
        Limits(TransferControl control){this.control=control;}
        private synchronized void check(TransferEvent event,boolean first) throws TransferCancelledException {
            try {
                control.check();DriverSpec.safeUri(event.getResource().getRepositoryUrl());
                if(first)attempts++;
                long current=event.getTransferredBytes(),previous=received.getOrDefault(event.getResource(),0L);
                bytes+=Math.max(0,current-previous);received.put(event.getResource(),current);
                long limit=event.getResource().getResourceName().endsWith(".jar")?InstallStore.MAX_JAR:8L*1024*1024;
                if(attempts>4096||current>limit||event.getResource().getContentLength()>limit||bytes>InstallStore.MAX_INSTALL){limitExceeded=true;throw new TransferCancelledException("Driver transfer limit exceeded");}
                control.progress("download",bytes,-1);
            }catch(TransferCancelledException failure){throw failure;}catch(Exception failure){throw new TransferCancelledException("Driver transfer cancelled or invalid");}
        }
        @Override public void transferInitiated(TransferEvent event) throws TransferCancelledException{check(event,true);}
        @Override public void transferStarted(TransferEvent event) throws TransferCancelledException{check(event,false);}
        @Override public void transferProgressed(TransferEvent event) throws TransferCancelledException{check(event,false);}
    }
    @Override public synchronized void close(){closed=true;if(system!=null)system.close();}
}
