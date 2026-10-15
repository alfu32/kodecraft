package kode.jdbc.drivers;

import java.io.*;
import java.nio.file.*;
import java.util.*;
import java.util.jar.*;
import javax.tools.ToolProvider;
import static org.junit.jupiter.api.Assertions.*;

final class DriverFixtures {
    static Path jar(Path path,String entry,String value) throws Exception {
        Files.createDirectories(path.getParent());try(var jar=new JarOutputStream(Files.newOutputStream(path))){jar.putNextEntry(new JarEntry(entry));jar.write(value.getBytes(java.nio.charset.StandardCharsets.UTF_8));jar.closeEntry();}return path;
    }
    static Path h2() throws Exception {return Path.of(org.h2.Driver.class.getProtectionDomain().getCodeSource().getLocation().toURI());}
    static Path dependency(Path root,int version) throws Exception {
        Path source=root.resolve("source/synthetic/Dependency.java");Files.createDirectories(source.getParent());Files.writeString(source,"package synthetic; public class Dependency { public static int value(){return "+version+";} }");
        Path classes=root.resolve("classes");Files.createDirectories(classes);assertEquals(0,ToolProvider.getSystemJavaCompiler().run(null,null,null,"--release","21","-d",classes.toString(),source.toString()));return pack(classes,root.resolve("dependency.jar"),null);
    }
    static Path driver(Path root,Path dependency,String cleanupKey) throws Exception {
        String source="""
            package synthetic;
            public class FixtureDriver implements java.sql.Driver {
                static { try {java.sql.DriverManager.registerDriver(new FixtureDriver(),()->System.setProperty("%s","closed"));}catch(java.sql.SQLException e){throw new RuntimeException(e);} }
                public java.sql.Connection connect(String url,java.util.Properties properties){return null;}
                public boolean acceptsURL(String url){return url.startsWith("jdbc:synthetic:");}
                public java.sql.DriverPropertyInfo[] getPropertyInfo(String url,java.util.Properties properties){return new java.sql.DriverPropertyInfo[0];}
                public int getMajorVersion(){return synthetic.Dependency.value();}
                public int getMinorVersion(){return 0;}
                public boolean jdbcCompliant(){return false;}
                public java.util.logging.Logger getParentLogger(){return java.util.logging.Logger.getGlobal();}
            }
            """.formatted(cleanupKey);
        Path file=root.resolve("source/synthetic/FixtureDriver.java");Files.createDirectories(file.getParent());Files.writeString(file,source);Path classes=root.resolve("classes");Files.createDirectories(classes);
        assertEquals(0,ToolProvider.getSystemJavaCompiler().run(null,null,null,"--release","21","-cp",dependency.toString(),"-d",classes.toString(),file.toString()));
        return pack(classes,root.resolve("driver.jar"),"synthetic.FixtureDriver");
    }
    private static Path pack(Path classes,Path output,String service) throws Exception {
        try(var jar=new JarOutputStream(Files.newOutputStream(output));var paths=Files.walk(classes)){
            for(Path file:paths.filter(Files::isRegularFile).sorted().toList()){jar.putNextEntry(new JarEntry(classes.relativize(file).toString().replace('\\','/')));Files.copy(file,jar);jar.closeEntry();}
            if(service!=null){jar.putNextEntry(new JarEntry("META-INF/services/java.sql.Driver"));jar.write((service+"\n").getBytes(java.nio.charset.StandardCharsets.UTF_8));jar.closeEntry();}
        }
        return output;
    }
    static TransferControl control(){return new TransferControl("synthetic",()->false,null);}
    static DriverSpec spec(String id,String provider,Path... paths) throws Exception {
        var value=kode.jdbc.protocol.FrameCodec.JSON.createObjectNode().put("id",id).put("providerId",provider).put("driverClass","org.h2.Driver");
        if(paths.length>0)value.put("jarPath",String.join(java.io.File.pathSeparator,Arrays.stream(paths).map(Path::toString).toList()));return DriverSpec.parse(value);
    }
}
