package kode.jdbc.drivers;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import java.io.*;
import java.nio.channels.FileChannel;
import java.nio.file.*;
import java.nio.file.attribute.PosixFilePermissions;
import java.security.MessageDigest;
import java.util.*;

/** Immutable, content-addressed installs; a small atomic pointer selects one.
 * User/legacy/local JARs are only read. No in-use install is removed or replaced.
 */
public final class InstallStore {
    public static final long MAX_JAR=1024L*1024*1024,MAX_INSTALL=4L*MAX_JAR;
    public static final int MAX_JARS=256,MAX_MANIFEST=2*1024*1024;
    public record Source(Path path,String coordinate) {}
    public record Jar(String path,String sha256,long size,String coordinate) {}
    public record Manifest(int version,String providerId,String requestKey,String resolvedVersion,
                           String classpathDigest,String source,List<Jar> jars,Map<String,String> localSources) {}
    public record Installed(Path directory,Manifest manifest,List<Path> classpath) {
        public ObjectNode wire(){
            var result=FrameCodec.JSON.createObjectNode().put("providerId",manifest.providerId()).put("resolvedVersion",manifest.resolvedVersion())
                .put("classpathDigest",manifest.classpathDigest()).put("source",manifest.source()).put("manifestPath",directory.resolve("install-v1.json").toString());
            var paths=result.putArray("classpath");classpath.forEach(path->paths.add(path.toString()));return result;
        }
    }
    private final Path root;
    public InstallStore(Path root){this.root=root.toAbsolutePath().normalize();}
    public Path root(){return root;}
    static void rejectLinks(Path path) throws IOException {
        for(Path ancestor=path.toAbsolutePath().normalize();ancestor!=null;ancestor=ancestor.getParent())
            if(Files.isSymbolicLink(ancestor))throw new IOException("Driver install paths cannot traverse symlinks");
    }
    public Path temporary() throws IOException {
        rejectLinks(root);Files.createDirectories(root);
        try{return Files.createTempDirectory(root,".work-",PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------")));}
        catch(UnsupportedOperationException e){return Files.createTempDirectory(root,".work-");}
    }
    private Path provider(DriverSpec spec){return root.resolve("installs").resolve(spec.providerDirectory());}
    private Path pointer(DriverSpec spec) throws Exception{return provider(spec).resolve("selected-"+spec.requestKey()+".json");}
    public Installed cached(DriverSpec spec,TransferControl control) throws Exception {
        Path pointer=pointer(spec);rejectLinks(pointer);boolean compatible=false;JsonNode value;
        if(Files.exists(pointer,LinkOption.NOFOLLOW_LINKS))value=readJson(pointer);
        else {value=compatiblePointer(spec,control);if(value==null)return null;compatible=true;}
        String digest=value.path("classpathDigest").asText();
        if(!digest.matches("[0-9a-f]{64}"))throw new IOException("Invalid driver install pointer");
        Installed installed=read(provider(spec).resolve(digest),control);
        if(!installed.manifest().providerId().equals(spec.providerId())||!installed.manifest().classpathDigest().equals(digest)||!compatible&&!value.path("requestKey").asText().equals(spec.requestKey()))throw new IOException("Driver install pointer does not match selected spec");
        Map<String,String> selectedSources=FrameCodec.JSON.convertValue(value.path("localSources"),new com.fasterxml.jackson.core.type.TypeReference<Map<String,String>>(){});
        if(selectedSources==null||selectedSources.size()>MAX_JARS)throw new IOException("Invalid driver source manifest");
        // Explicit local paths can change in place; never reuse stale copied JARs.
        if(spec.repositoryType().equals("LOCAL_FILE")&&!spec.localPaths().isEmpty()){
            if(selectedSources.size()!=spec.localPaths().size())return null;
            for(Path source:spec.localPaths())if(Files.isRegularFile(source)&&!Objects.equals(selectedSources.get(source.toString()),hash(source,control)))return null;
        }
        Manifest original=installed.manifest();
        Manifest selected=new Manifest(1,original.providerId(),spec.requestKey(),value.path("resolvedVersion").asText(),original.classpathDigest(),value.path("source").asText(),original.jars(),Map.copyOf(selectedSources));
        return new Installed(installed.directory(),selected,installed.classpath());
    }
    private JsonNode compatiblePointer(DriverSpec spec,TransferControl control) throws Exception {
        if(spec.version().isBlank())return null;Path directory=provider(spec);rejectLinks(directory);if(!Files.isDirectory(directory,LinkOption.NOFOLLOW_LINKS))return null;
        JsonNode selected=null;int count=0;
        try(var pointers=Files.newDirectoryStream(directory,"selected-*.json")) {
            for(Path path:pointers){control.check();if(++count>512)throw new IOException("Driver selection cache exceeds 512 entries");JsonNode value=readJson(path);
                if(value.path("familyKey").asText().equals(spec.familyKey())&&value.path("resolvedVersion").asText().equals(spec.version())) {
                    if(selected!=null&&!selected.path("classpathDigest").equals(value.path("classpathDigest")))return null;
                    selected=value;
                }
            }
        }
        return selected;
    }
    public Installed publish(DriverSpec spec,String version,String source,List<Source> inputs,Path work,TransferControl control) throws Exception {
        if(inputs.isEmpty()||inputs.size()>MAX_JARS)throw new RpcException(-32010,"Driver classpath is empty or exceeds 256 artifacts");
        Path staged=work.resolve("install"),jars=staged.resolve("jars");Files.createDirectories(jars);
        List<Jar> entries=new ArrayList<>();Map<String,String> localSources=new TreeMap<>();long total=0;
        for(int i=0;i<inputs.size();i++){
            control.check();Source input=inputs.get(i);Path path=input.path().toAbsolutePath().normalize();
            String filename=path.getFileName().toString().replaceAll("[^A-Za-z0-9_.+-]","_");if(filename.length()>128)filename="driver.jar";
            String relative="jars/"+String.format(Locale.ROOT,"%04d-",i)+filename;Path target=staged.resolve(relative);
            String digest=copy(path,target,control);long size=Files.size(target);total+=size;if(total>MAX_INSTALL)throw new IOException("Driver install exceeds 4 GiB");
            // Verify a real ZIP/JAR without eagerly extracting its contents.
            try(var jar=new java.util.jar.JarFile(target.toFile())){jar.size();}
            entries.add(new Jar(relative,digest,size,input.coordinate()));
            if(spec.repositoryType().equals("LOCAL_FILE"))localSources.put(path.toString(),digest);
        }
        String classpathDigest=classpathDigest(entries);
        Manifest manifest=new Manifest(1,spec.providerId(),spec.requestKey(),version,classpathDigest,source,List.copyOf(entries),Map.copyOf(localSources));
        byte[] bytes=FrameCodec.JSON.writeValueAsBytes(manifest);if(bytes.length>MAX_MANIFEST)throw new IOException("Driver manifest too large");
        writeSynced(staged.resolve("install-v1.json"),bytes);control.check();
        Path provider=provider(spec);rejectLinks(provider);Files.createDirectories(provider);
        Path lockPath=provider.resolve(".install.lock");rejectLinks(lockPath);
        if(Files.exists(lockPath,LinkOption.NOFOLLOW_LINKS)&&!Files.isRegularFile(lockPath,LinkOption.NOFOLLOW_LINKS))throw new IOException("Driver lock target is not regular");
        try(FileChannel channel=FileChannel.open(lockPath,StandardOpenOption.CREATE,StandardOpenOption.WRITE)){
            java.nio.channels.FileLock lock;
            try{lock=channel.tryLock();}catch(java.nio.channels.OverlappingFileLockException e){lock=null;}
            if(lock==null)throw new RpcException(-32003,"Another driver install is publishing; retry after it finishes",FrameCodec.JSON.createObjectNode().put("notStarted",true));
            try {
                control.check();Path destination=provider.resolve(classpathDigest);rejectLinks(destination);
                if(Files.exists(destination,LinkOption.NOFOLLOW_LINKS)){
                    Installed existing=read(destination,control);
                    // Identical bytes may be selected by several requests/specs.
                    // A pointer carries the spec identity; immutable content does not.
                    if(!existing.manifest().classpathDigest().equals(classpathDigest))throw new IOException("Driver content-address collision");
                    manifest=new Manifest(1,spec.providerId(),spec.requestKey(),version,classpathDigest,source,existing.manifest().jars(),Map.copyOf(localSources));
                }else Files.move(staged,destination,StandardCopyOption.ATOMIC_MOVE);
                control.check();
                ObjectNode pointer=FrameCodec.JSON.createObjectNode().put("classpathDigest",classpathDigest).put("requestKey",spec.requestKey()).put("familyKey",spec.familyKey())
                    .put("resolvedVersion",version).put("source",source);
                pointer.set("localSources",FrameCodec.JSON.valueToTree(localSources));
                atomicWrite(pointer(spec),FrameCodec.JSON.writeValueAsBytes(pointer));
                // Return selected request metadata even when identical classpath
                // bytes already existed under another coordinate/request.
                return new Installed(destination,manifest,manifest.jars().stream().map(jar->destination.resolve(jar.path())).toList());
            } finally {lock.release();}
        }
    }
    public Installed read(Path directory,TransferControl control) throws Exception {
        rejectLinks(directory);Manifest manifest=FrameCodec.JSON.treeToValue(readJson(directory.resolve("install-v1.json")),Manifest.class);
        if(manifest.version()!=1||manifest.jars()==null||manifest.jars().isEmpty()||manifest.jars().size()>MAX_JARS||manifest.localSources()==null)throw new IOException("Invalid driver install manifest");
        List<Path> paths=new ArrayList<>();long total=0;
        for(Jar jar:manifest.jars()){
            control.check();Path relative=Path.of(jar.path()).normalize();
            if(relative.isAbsolute()||relative.getNameCount()!=2||!relative.getName(0).toString().equals("jars")||relative.toString().contains(".."))throw new IOException("Invalid manifest artifact path");
            Path path=directory.resolve(relative);rejectLinks(path);
            if(!Files.isRegularFile(path,LinkOption.NOFOLLOW_LINKS)||Files.size(path)!=jar.size()||!hash(path,control).equals(jar.sha256()))throw new IOException("Installed driver JAR changed; reinstall explicitly");
            total+=jar.size();if(total>MAX_INSTALL)throw new IOException("Installed classpath exceeds 4 GiB");paths.add(path);
        }
        if(!classpathDigest(manifest.jars()).equals(manifest.classpathDigest()))throw new IOException("Invalid installed classpath digest");
        return new Installed(directory,manifest,List.copyOf(paths));
    }
    private static JsonNode readJson(Path path) throws IOException {
        rejectLinks(path);if(!Files.isRegularFile(path,LinkOption.NOFOLLOW_LINKS)||Files.size(path)>MAX_MANIFEST)throw new IOException("Driver manifest must be a bounded regular file");
        try(InputStream input=Files.newInputStream(path)){byte[] bytes=input.readNBytes(MAX_MANIFEST+1);if(bytes.length>MAX_MANIFEST)throw new IOException("Driver manifest too large");return FrameCodec.JSON.readTree(bytes);}
    }
    static String classpathDigest(List<Jar> jars) throws IOException {
        var value=FrameCodec.JSON.createArrayNode();for(Jar jar:jars)value.addObject().put("sha256",jar.sha256()).put("size",jar.size());
        return DriverSpec.digest(FrameCodec.JSON.writeValueAsBytes(value));
    }
    static String hash(Path source,TransferControl control) throws Exception {return copy(source,null,control);}
    private static String copy(Path source,Path target,TransferControl control) throws Exception {
        if(!Files.isRegularFile(source)||Files.size(source)>MAX_JAR)throw new IOException("Driver JAR is missing, not regular, or exceeds 1 GiB");
        MessageDigest digest=MessageDigest.getInstance("SHA-256");long total=0;
        try(InputStream input=Files.newInputStream(source);OutputStream output=target==null?OutputStream.nullOutputStream():Files.newOutputStream(target,StandardOpenOption.CREATE_NEW)){
            byte[] buffer=new byte[65536];int count;while((count=input.read(buffer))!=-1){control.check();total+=count;if(total>MAX_JAR)throw new IOException("Driver JAR exceeds 1 GiB");digest.update(buffer,0,count);output.write(buffer,0,count);}
        }
        if(target!=null)try(FileChannel file=FileChannel.open(target,StandardOpenOption.WRITE)){file.force(true);}
        return HexFormat.of().formatHex(digest.digest());
    }
    private static void writeSynced(Path path,byte[] bytes) throws IOException {
        try(FileChannel file=FileChannel.open(path,StandardOpenOption.CREATE_NEW,StandardOpenOption.WRITE)){var buffer=java.nio.ByteBuffer.wrap(bytes);while(buffer.hasRemaining())file.write(buffer);file.force(true);}
    }
    private static void atomicWrite(Path path,byte[] bytes) throws IOException {
        rejectLinks(path);Path temporary=Files.createTempFile(path.getParent(),".pointer-",".tmp");
        try {try(FileChannel file=FileChannel.open(temporary,StandardOpenOption.WRITE,StandardOpenOption.TRUNCATE_EXISTING)){var buffer=java.nio.ByteBuffer.wrap(bytes);while(buffer.hasRemaining())file.write(buffer);file.force(true);}Files.move(temporary,path,StandardCopyOption.ATOMIC_MOVE,StandardCopyOption.REPLACE_EXISTING);}
        finally {Files.deleteIfExists(temporary);}
    }
    public static void cleanup(Path work) throws IOException {
        if(!Files.exists(work,LinkOption.NOFOLLOW_LINKS))return;
        // walk does not follow symlinks; only this resolution's private work root.
        Files.walkFileTree(work,new SimpleFileVisitor<>() {
            @Override public FileVisitResult visitFile(Path path,java.nio.file.attribute.BasicFileAttributes attributes) throws IOException {Files.delete(path);return FileVisitResult.CONTINUE;}
            @Override public FileVisitResult postVisitDirectory(Path path,IOException failure) throws IOException {if(failure!=null)throw failure;Files.delete(path);return FileVisitResult.CONTINUE;}
        });
    }
}
