package kode.jdbc.drivers;

import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import java.nio.file.*;
import java.util.*;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.io.TempDir;
import static org.junit.jupiter.api.Assertions.*;

@Timeout(25)
class MavenArtifactsTest {
    @TempDir Path root;
    static String pom(String artifact,String version,String content){return "<project xmlns=\"http://maven.apache.org/POM/4.0.0\"><modelVersion>4.0.0</modelVersion><groupId>synthetic.fixture</groupId><artifactId>"+artifact+"</artifactId><version>"+version+"</version>"+content+"</project>";}
    Path coordinate(String artifact,String version){return root.resolve("repository/synthetic/fixture").resolve(artifact).resolve(version);}
    void artifact(String name,String version,String classifier,String pom,Path jar) throws Exception {
        Path directory=coordinate(name,version);Files.createDirectories(directory);Files.writeString(directory.resolve(name+"-"+version+".pom"),pom);
        if(jar!=null)Files.copy(jar,directory.resolve(name+"-"+version+(classifier.isBlank()?"":"-"+classifier)+".jar"));
    }
    DriverSpec spec(String version,String classifier) throws Exception {
        var value=FrameCodec.JSON.createObjectNode().put("id","fixture").put("providerId","fixture").put("driverClass","synthetic.FixtureDriver");
        value.putObject("artifact").put("repositoryType","MAVEN_CUSTOM").put("groupId","synthetic.fixture").put("artifactId","driver").put("version",version).put("classifier",classifier).put("repository",root.resolve("repository").toUri().toString());return DriverSpec.parse(value);
    }
    Path dependency() throws Exception {return DriverFixtures.dependency(root.resolve("dep"),42);}
    @Test void parentPomImportedBomRuntimeScopeClassifierAndOptionalExclusionsResolveAsACompleteGraph() throws Exception {
        Path dependency=dependency(),driver=DriverFixtures.driver(root.resolve("driver"),dependency,"kode.synthetic.maven.cleanup");
        artifact("parent","1.0","",pom("parent","1.0","<packaging>pom</packaging><properties><fixture.version>2.0</fixture.version></properties>"),null);
        artifact("bom","1.0","",pom("bom","1.0","<packaging>pom</packaging><dependencyManagement><dependencies><dependency><groupId>synthetic.fixture</groupId><artifactId>helper</artifactId><version>2.0</version></dependency></dependencies></dependencyManagement>"),null);
        artifact("helper","2.0","",pom("helper","2.0",""),dependency);
        String project="""
            <project xmlns="http://maven.apache.org/POM/4.0.0"><modelVersion>4.0.0</modelVersion>
            <parent><groupId>synthetic.fixture</groupId><artifactId>parent</artifactId><version>1.0</version><relativePath/></parent>
            <artifactId>driver</artifactId><version>1.0</version>
            <dependencyManagement><dependencies><dependency><groupId>synthetic.fixture</groupId><artifactId>bom</artifactId><version>1.0</version><type>pom</type><scope>import</scope></dependency></dependencies></dependencyManagement>
            <dependencies>
            <dependency><groupId>synthetic.fixture</groupId><artifactId>helper</artifactId><scope>runtime</scope></dependency>
            <dependency><groupId>synthetic.fixture</groupId><artifactId>absent-optional</artifactId><version>1.0</version><optional>true</optional></dependency>
            <dependency><groupId>synthetic.fixture</groupId><artifactId>absent-provided</artifactId><version>1.0</version><scope>provided</scope></dependency>
            <dependency><groupId>synthetic.fixture</groupId><artifactId>absent-test</artifactId><version>1.0</version><scope>test</scope></dependency>
            </dependencies></project>
            """;
        artifact("driver","1.0","all",project,driver);
        try(MavenArtifacts maven=new MavenArtifacts();LoaderCatalog loaders=new LoaderCatalog()){
            Path work=root.resolve("work");Files.createDirectories(work);var resolved=maven.resolve(spec("1.0","all"),null,work,DriverFixtures.control());assertEquals("1.0",resolved.version());assertEquals(2,resolved.sources().size());assertTrue(resolved.sources().getFirst().coordinate().contains(":all:"));
            var loaded=loaders.inspect("fixture",resolved.sources().stream().map(InstallStore.Source::path).toList(),"synthetic.FixtureDriver",DriverFixtures.control());assertEquals(42,loaded.path("driverMajorVersion").intValue());
        }finally{System.clearProperty("kode.synthetic.maven.cleanup");}
    }
    @Test void latestSelectionFiltersSourcePrereleasesAndUsesNumericOrderThenPinnedManifestReusesOffline() throws Exception {
        Path jar=DriverFixtures.h2();String metadata="<metadata><groupId>synthetic.fixture</groupId><artifactId>driver</artifactId><versioning><versions><version>1.9</version><version>1.10</version><version>1.11-RC1</version><version>2.0-SNAPSHOT</version></versions></versioning></metadata>";
        Path directory=root.resolve("repository/synthetic/fixture/driver");Files.createDirectories(directory);Files.writeString(directory.resolve("maven-metadata.xml"),metadata);
        artifact("driver","1.10","",pom("driver","1.10",""),jar);
        try(MavenArtifacts maven=new MavenArtifacts()){
            Path work=root.resolve("work");Files.createDirectories(work);var spec=spec("","");var resolved=maven.resolve(spec,null,work,DriverFixtures.control());assertEquals("1.10",resolved.version());
            InstallStore store=new InstallStore(root.resolve("cache"));Path publication=store.temporary();var installed=store.publish(spec,resolved.version(),"maven",resolved.sources(),publication,DriverFixtures.control());InstallStore.cleanup(publication);
            InstallStore.cleanup(root.resolve("repository"));assertEquals("1.10",store.cached(spec,DriverFixtures.control()).manifest().resolvedVersion());assertEquals(installed.classpath(),store.cached(spec,DriverFixtures.control()).classpath());
        }
    }
    @Test void missingRequiredDependencyFailsWithoutPublishingOrLeavingResolutionTemporaries() throws Exception {
        Path jar=DriverFixtures.h2();artifact("driver","1.0","",pom("driver","1.0","<dependencies><dependency><groupId>synthetic.fixture</groupId><artifactId>missing-required</artifactId><version>9.9</version></dependency></dependencies>"),jar);
        try(DriverService service=new DriverService(root.resolve("cache"),null)){
            var params=FrameCodec.JSON.createObjectNode().put("resolutionId","failure");params.set("driver",FrameCodec.JSON.valueToTree(Map.of("id","fixture","driverClass","org.h2.Driver","artifact",Map.of("repositoryType","MAVEN_CUSTOM","groupId","synthetic.fixture","artifactId","driver","version","1.0","repository",root.resolve("repository").toUri().toString()))));
            var failure=assertThrows(java.util.concurrent.ExecutionException.class,()->service.resolve(params,null).toCompletableFuture().get());assertInstanceOf(RpcException.class,failure.getCause());
            try(var files=Files.list(root.resolve("cache"))){assertEquals(0,files.count());}
        }
    }
}
