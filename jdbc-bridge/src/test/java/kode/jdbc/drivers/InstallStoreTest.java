package kode.jdbc.drivers;

import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import java.nio.file.*;
import java.util.*;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import static org.junit.jupiter.api.Assertions.*;

class InstallStoreTest {
    @TempDir Path root;
    @Test void completeMultiJarManifestCopiesSourcesPinsDigestsReusesAliasesAndRejectsTampering() throws Exception {
        Path one=DriverFixtures.jar(root.resolve("source/a.jar"),"one.txt","one"),two=DriverFixtures.jar(root.resolve("source/b.jar"),"two.txt","two");
        byte[] original=Files.readAllBytes(one);InstallStore store=new InstallStore(root.resolve("installs"));
        DriverSpec base=DriverFixtures.spec("postgresql","postgresql",one,two),alias=DriverFixtures.spec("cockroachdb","postgresql",one,two);
        assertEquals(base.requestKey(),alias.requestKey());assertNull(store.cached(base,DriverFixtures.control()));
        Path work=store.temporary();var installed=store.publish(base,"local","local-classpath",List.of(new InstallStore.Source(one,"local"),new InstallStore.Source(two,"local")),work,DriverFixtures.control());InstallStore.cleanup(work);
        assertEquals(2,installed.classpath().size());assertArrayEquals(original,Files.readAllBytes(one));assertArrayEquals(original,Files.readAllBytes(installed.classpath().getFirst()));
        assertEquals(installed.manifest().classpathDigest(),store.cached(alias,DriverFixtures.control()).manifest().classpathDigest());
        Files.writeString(installed.classpath().getFirst(),"corrupt");assertThrows(Exception.class,()->store.cached(base,DriverFixtures.control()));assertArrayEquals(original,Files.readAllBytes(one));
    }
    @Test void changedLocalSourceGetsNewImmutableInstallWhileOldRemainsAndFailureNeverRepoints() throws Exception {
        Path jar=DriverFixtures.jar(root.resolve("source/a.jar"),"one.txt","one");InstallStore store=new InstallStore(root.resolve("cache"));DriverSpec spec=DriverFixtures.spec("custom","custom",jar);
        Path work=store.temporary();var old=store.publish(spec,"local","local-classpath",List.of(new InstallStore.Source(jar,"local")),work,DriverFixtures.control());InstallStore.cleanup(work);byte[] before=Files.readAllBytes(old.classpath().getFirst());
        Files.delete(jar);DriverFixtures.jar(jar,"one.txt","two");assertNull(store.cached(spec,DriverFixtures.control()));work=store.temporary();var updated=store.publish(spec,"local","local-classpath",List.of(new InstallStore.Source(jar,"local")),work,DriverFixtures.control());InstallStore.cleanup(work);assertNotEquals(old.manifest().classpathDigest(),updated.manifest().classpathDigest());assertArrayEquals(before,Files.readAllBytes(old.classpath().getFirst()));
        Path failureWork=store.temporary();assertThrows(Exception.class,()->store.publish(spec,"broken","local-classpath",List.of(new InstallStore.Source(root.resolve("missing.jar"),"local")),failureWork,DriverFixtures.control()));InstallStore.cleanup(failureWork);
        assertEquals(updated.manifest().classpathDigest(),store.cached(spec,DriverFixtures.control()).manifest().classpathDigest());
    }
    @Test void sameClasspathBytesWithDifferentSourceNamesReuseContentWithoutBrokenPaths() throws Exception {
        Path one=DriverFixtures.jar(root.resolve("a.jar"),"value.txt","same"),two=root.resolve("renamed.jar");Files.copy(one,two);
        InstallStore store=new InstallStore(root.resolve("cache"));var a=DriverFixtures.spec("a","provider",one);var b=DriverFixtures.spec("b","provider",two);
        Path work=store.temporary();var first=store.publish(a,"local","local-classpath",List.of(new InstallStore.Source(one,"local")),work,DriverFixtures.control());InstallStore.cleanup(work);
        work=store.temporary();var second=store.publish(b,"local","local-classpath",List.of(new InstallStore.Source(two,"local")),work,DriverFixtures.control());InstallStore.cleanup(work);
        assertEquals(first.directory(),second.directory());assertTrue(Files.isRegularFile(second.classpath().getFirst()));assertEquals(second.classpath(),store.cached(b,DriverFixtures.control()).classpath());
    }
    @Test void cancellationAndSymlinkDestinationsCannotPublishPartialInstalls() throws Exception {
        Path source=DriverFixtures.jar(root.resolve("a.jar"),"value.txt","same");var spec=DriverFixtures.spec("a","provider",source);InstallStore store=new InstallStore(root.resolve("cache"));Path work=store.temporary();TransferControl control=DriverFixtures.control();control.cancel();
        assertThrows(RpcException.class,()->store.publish(spec,"local","local-classpath",List.of(new InstallStore.Source(source,"local")),work,control));InstallStore.cleanup(work);assertNull(store.cached(spec,DriverFixtures.control()));
        Path external=root.resolve("external");Files.createDirectories(external);Path link=root.resolve("link");try{Files.createSymbolicLink(link,external);}catch(UnsupportedOperationException e){return;}
        assertThrows(Exception.class,()->new InstallStore(link).temporary());assertEquals(0,Files.list(external).count());
    }
    @Test void safeProviderDirectoryAndSourceStableVersionPolicyPreserveSelection() throws Exception {
        var value=FrameCodec.JSON.createObjectNode().put("id","alias").put("providerId","../../outside").put("driverClass","x");DriverSpec spec=DriverSpec.parse(value);assertTrue(spec.providerDirectory().matches("[0-9a-f]{64}"));
        assertFalse(MavenArtifacts.stable("3.0-RC1"));assertFalse(MavenArtifacts.stable("3.0-SNAPSHOT"));assertTrue(MavenArtifacts.stable("2.4.240"));assertTrue(MavenArtifacts.compareVersions("2.10","2.9")>0);assertTrue(MavenArtifacts.compareVersions("2.4-final","2.4")>0);
    }
}
