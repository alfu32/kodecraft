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
    @Test void activeConnectionLeasePreventsReplacementAndOldIdIsStaleAfterReleaseAndUpdate() throws Exception {
        String marker="kode.synthetic.cleanup."+UUID.randomUUID();Path a=DriverFixtures.dependency(root.resolve("dep-a"),1),b=DriverFixtures.dependency(root.resolve("dep-b"),2),driver=DriverFixtures.driver(root.resolve("driver"),a,marker);
        try(LoaderCatalog loaders=new LoaderCatalog()){
            var first=loaders.inspect("provider",List.of(driver,a),"synthetic.FixtureDriver",DriverFixtures.control());String oldId=first.path("loaderId").asText();
            try(var lease=loaders.acquire(oldId,"synthetic.FixtureDriver")){RpcException error=assertThrows(RpcException.class,()->loaders.inspect("provider",List.of(driver,b),"synthetic.FixtureDriver",DriverFixtures.control()));assertEquals(-32011,error.code);assertEquals(1,lease.driver().getMajorVersion());}
            var updated=loaders.inspect("provider",List.of(driver,b),"synthetic.FixtureDriver",DriverFixtures.control());assertEquals(2,updated.path("driverMajorVersion").intValue());assertTrue(updated.path("restartRecommended").asBoolean());assertThrows(RpcException.class,()->loaders.acquire(oldId,"synthetic.FixtureDriver"));
        }finally{System.clearProperty(marker);}
    }
}
