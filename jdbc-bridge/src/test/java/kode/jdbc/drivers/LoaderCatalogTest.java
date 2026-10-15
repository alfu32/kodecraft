package kode.jdbc.drivers;

import kode.jdbc.protocol.RpcException;
import java.nio.file.*;
import java.util.*;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.api.Timeout;
import static org.junit.jupiter.api.Assertions.*;

@Timeout(15)
class LoaderCatalogTest {
    @TempDir Path root;
    @Test void visibleSourceH2AndIsolatedH2LoadAndQueryThroughTheIntendedDriverInstance() throws Exception {
        try(LoaderCatalog loaders=new LoaderCatalog()){
            var visible=loaders.inspect("h2",List.of(),"org.h2.Driver",DriverFixtures.control());
            try(var lease=loaders.acquire(visible.path("loaderId").asText(),"org.h2.Driver");var connection=lease.driver().connect("jdbc:h2:mem:builtin_synthetic",new Properties());var statement=connection.createStatement();var rows=statement.executeQuery("select 1")){assertTrue(rows.next());assertEquals(1,rows.getInt(1));}
            var isolated=loaders.inspect("h2-isolated",List.of(DriverFixtures.h2()),"org.h2.Driver",DriverFixtures.control());
            try(var lease=loaders.acquire(isolated.path("loaderId").asText(),"org.h2.Driver");var connection=lease.driver().connect("jdbc:h2:mem:isolated_synthetic",new Properties());var statement=connection.createStatement();var rows=statement.executeQuery("select 'é界😀'")){
                assertNotEquals(org.h2.Driver.class.getClassLoader(),lease.driver().getClass().getClassLoader());assertTrue(rows.next());assertEquals("é界😀",rows.getString(1));
            }
            assertThrows(RpcException.class,()->loaders.inspect("postgresql",List.of(),"org.postgresql.Driver",DriverFixtures.control()));
        }
    }
    @Test void separateJarDependencyIsRequiredAndChildDriverManagerRegistrationsAreReleased() throws Exception {
        String marker="kode.synthetic.cleanup."+UUID.randomUUID();Path dependency=DriverFixtures.dependency(root.resolve("dep"),42),driver=DriverFixtures.driver(root.resolve("driver"),dependency,marker);
        try(LoaderCatalog loaders=new LoaderCatalog()){
            assertThrows(RpcException.class,()->loaders.inspect("missing-dependency",List.of(driver),"synthetic.FixtureDriver",DriverFixtures.control()));
            var loaded=loaders.inspect("synthetic",List.of(driver,dependency),"synthetic.FixtureDriver",DriverFixtures.control());assertEquals(42,loaded.path("driverMajorVersion").intValue());
            try(var lease=loaders.acquire(loaded.path("loaderId").asText(),"synthetic.FixtureDriver")){assertTrue(lease.driver().acceptsURL("jdbc:synthetic:one"));}
            System.clearProperty(marker);
        }
        assertEquals("closed",System.getProperty(marker));System.clearProperty(marker);
    }
    @Test void sameProviderVersionsRemainIsolatedAndActiveLeasesSurviveVersionSelection() throws Exception {
        String marker="kode.synthetic.cleanup."+UUID.randomUUID();Path a=DriverFixtures.dependency(root.resolve("dep-a"),1),b=DriverFixtures.dependency(root.resolve("dep-b"),2),driver=DriverFixtures.driver(root.resolve("driver"),a,marker);
        try(LoaderCatalog loaders=new LoaderCatalog()){
            var first=loaders.inspect("provider",List.of(driver,a),"synthetic.FixtureDriver",DriverFixtures.control());String oldId=first.path("loaderId").asText();
            try(var lease=loaders.acquire(oldId,"synthetic.FixtureDriver")){
                var updated=loaders.inspect("provider",List.of(driver,b),"synthetic.FixtureDriver",DriverFixtures.control());assertEquals(2,updated.path("driverMajorVersion").intValue());assertTrue(updated.path("restartRecommended").asBoolean());
                try(var second=loaders.acquire(updated.path("loaderId").asText(),"synthetic.FixtureDriver")){assertEquals(1,lease.driver().getMajorVersion());assertEquals(2,second.driver().getMajorVersion());assertNotEquals(lease.driver().getClass().getClassLoader(),second.driver().getClass().getClassLoader());}
                assertEquals(oldId,loaders.inspect("provider",List.of(driver,a),"synthetic.FixtureDriver",DriverFixtures.control()).path("loaderId").asText());
            }
            loaders.close();assertThrows(RpcException.class,()->loaders.acquire(oldId,"synthetic.FixtureDriver"));
        }finally{System.clearProperty(marker);}
    }
    @Test void providersKeepConflictingDependencyVersionsSeparateAndCloseDefersUntilBothLeasesEnd() throws Exception {
        String marker="kode.synthetic.cleanup."+UUID.randomUUID();Path a=DriverFixtures.dependency(root.resolve("a"),1),b=DriverFixtures.dependency(root.resolve("b"),2),driver=DriverFixtures.driver(root.resolve("driver"),a,marker);
        LoaderCatalog loaders=new LoaderCatalog();var left=loaders.inspect("left",List.of(driver,a),"synthetic.FixtureDriver",DriverFixtures.control());var right=loaders.inspect("right",List.of(driver,b),"synthetic.FixtureDriver",DriverFixtures.control());
        try(var one=loaders.acquire(left.path("loaderId").asText(),"synthetic.FixtureDriver");var two=loaders.acquire(right.path("loaderId").asText(),"synthetic.FixtureDriver")){
            assertEquals(1,one.driver().getMajorVersion());assertEquals(2,two.driver().getMajorVersion());assertNotEquals(one.driver().getClass().getClassLoader(),two.driver().getClass().getClassLoader());
            System.clearProperty(marker);loaders.close();assertNull(System.getProperty(marker));one.close();assertEquals("closed",System.getProperty(marker));System.clearProperty(marker);two.close();assertEquals("closed",System.getProperty(marker));
        }finally{loaders.close();System.clearProperty(marker);}
    }
    @Test void nativeLinkageFailureWrappedByServiceLoadingRequiresExplicitHelperRestart() throws Exception {
        Path dependency=DriverFixtures.dependency(root.resolve("dependency"),1),driver=DriverFixtures.driver(root.resolve("native"),dependency,"kode.synthetic.native",true);
        try(LoaderCatalog loaders=new LoaderCatalog()) {RpcException failure=assertThrows(RpcException.class,()->loaders.inspect("native",List.of(driver,dependency),"synthetic.FixtureDriver",DriverFixtures.control()));assertEquals(-32011,failure.code);assertTrue(failure.data.path("restartRequired").asBoolean());assertFalse(failure.getMessage().contains("synthetic native library"));}
    }
    @Test void cachePressureEvictsOnlyUnusedLoadersAndNeverInvalidatesAnActiveConnectionLease() throws Exception {
        try(LoaderCatalog loaders=new LoaderCatalog()){
            var first=loaders.inspect("active",List.of(DriverFixtures.h2()),"org.h2.Driver",DriverFixtures.control());String evicted=null;
            try(var active=loaders.acquire(first.path("loaderId").asText(),"org.h2.Driver")){
                for(int index=0;index<64;index++){var info=loaders.inspect("unused-"+index,List.of(DriverFixtures.h2()),"org.h2.Driver",DriverFixtures.control());if(index==0)evicted=info.path("loaderId").asText();}
                assertEquals(2,active.driver().getMajorVersion());String oldId=evicted;assertEquals(-32012,assertThrows(RpcException.class,()->loaders.acquire(oldId,"org.h2.Driver")).code);
                assertEquals(first.path("loaderId"),loaders.inspect("active",List.of(DriverFixtures.h2()),"org.h2.Driver",DriverFixtures.control()).path("loaderId"));
            }
        }
    }
}
