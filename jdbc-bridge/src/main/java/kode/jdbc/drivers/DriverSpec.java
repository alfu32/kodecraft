package kode.jdbc.drivers;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import java.net.URI;
import java.nio.file.Path;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.*;

/** No credentials/settings lookup. This is the explicit parent-supplied spec. */
public record DriverSpec(String id,String providerId,String driverClass,String repositoryType,
                         String groupId,String artifactId,String version,String classifier,
                         String repository,String downloadUrl,String vendorPage,List<Path> localPaths) {
    public static final String CENTRAL="https://repo.maven.apache.org/maven2";
    public static DriverSpec parse(ObjectNode value) throws RpcException {
        String id=text(value,"id","custom-jdbc"),provider=text(value,"providerId",id);
        if(id.isBlank()||provider.isBlank()||id.length()>128||provider.length()>128)throw bad();
        JsonNode artifact=value.path("artifact");
        String coordinates=text(value,"coordinates","");String[] parts=coordinates.split(":",-1);
        String group=text(artifact,"groupId",parts.length>1?parts[0]:""),name=text(artifact,"artifactId",parts.length>1?parts[1]:"");
        String version=text(artifact,"version",parts.length>2?parts[2]:""),classifier=text(artifact,"classifier",parts.length>3?parts[3]:"");
        String jarPath=text(value,"jarPath","");
        List<Path> paths=new ArrayList<>();
        if(!jarPath.isBlank())for(String path:jarPath.split(java.util.regex.Pattern.quote(java.io.File.pathSeparator),-1)) {
            if(path.isBlank())throw bad();paths.add(Path.of(path).toAbsolutePath().normalize());
        }
        if(paths.size()>256)throw bad();
        String type=text(artifact,"repositoryType",group.isBlank()?"LOCAL_FILE":"MAVEN_CENTRAL");
        if(!Set.of("MAVEN_CENTRAL","MAVEN_CUSTOM","VENDOR_URL","LOCAL_FILE").contains(type))throw bad();
        String repository=text(artifact,"repository",CENTRAL),download=text(artifact,"downloadUrl",""),page=text(artifact,"vendorDownloadPage","");
        if(!repository.isBlank())safeUri(repository);if(!download.isBlank())safeUri(download);if(!page.isBlank())safeUri(page);
        if(!group.isBlank()&&!group.matches("[A-Za-z0-9_][A-Za-z0-9_.-]{0,255}"))throw bad();
        for(String part:List.of(name,version,classifier))if(!part.isEmpty()&&(!part.matches("[A-Za-z0-9_][A-Za-z0-9_.+-]{0,255}")||part.equals(".")||part.equals("..")))throw bad();
        return new DriverSpec(id,provider,text(value,"driverClass",""),type,group,name,version,classifier,repository,download,page,List.copyOf(paths));
    }
    static String text(JsonNode value,String key,String fallback) throws RpcException {
        JsonNode item=value.get(key);if(item==null||item.isNull())return fallback;
        if(!item.isTextual()||item.textValue().length()>65536)throw bad();return item.textValue();
    }
    static RpcException bad(){return new RpcException(-32602,"Invalid driver specification");}
    static URI safeUri(String text) throws RpcException {
        try {
            URI uri=URI.create(text);
            if(!Set.of("http","https","file").contains(uri.getScheme())||uri.getUserInfo()!=null||uri.getFragment()!=null
                ||text.matches("(?is).*(?:[?;&](?:password|pwd|token|secret|access_token|apikey|api_key)=).*"))throw bad();
            return uri;
        } catch(IllegalArgumentException e){throw bad();}
    }
    String providerDirectory(){return digest(providerId.getBytes(StandardCharsets.UTF_8));}
    String requestKey() throws Exception {
        // Alias IDs/display names/class discovery do not change install family.
        var value=FrameCodec.JSON.createObjectNode().put("providerId",providerId).put("repositoryType",repositoryType)
            .put("groupId",groupId).put("artifactId",artifactId).put("version",version).put("classifier",classifier)
            .put("repository",repository).put("downloadUrl",downloadUrl);
        var paths=value.putArray("localPaths");for(Path path:localPaths)paths.add(path.toString());
        return digest(FrameCodec.JSON.writeValueAsBytes(value));
    }
    static String digest(byte[] bytes) {try{return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(bytes));}catch(Exception e){throw new AssertionError(e);}}
}
