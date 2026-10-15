package kode.jdbc.drivers;

import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import java.io.*;
import java.net.*;
import java.nio.file.*;
import java.sql.Driver;
import java.util.*;

/** Isolated provider/digest loaders and connection leases. No in-use loader or
 * artifact is replaced/deleted. Helper-visible H2 preserves source fallback.
 */
public final class LoaderCatalog implements AutoCloseable {
    private static final int MAX_LOADERS=64,MAX_DRIVERS=128;
    static final class Isolated extends URLClassLoader {
        private final Class<?> cleanup;
        Isolated(URL[] urls) throws IOException {
            super(urls,ClassLoader.getPlatformClassLoader());
            try(InputStream stream=LoaderCleanup.class.getResourceAsStream("LoaderCleanup.class")) {
                if(stream==null)throw new IOException("Driver cleanup class unavailable");byte[] bytes=stream.readAllBytes();
                cleanup=defineClass(LoaderCleanup.class.getName(),bytes,0,bytes.length);
            }catch(IOException|RuntimeException failure){super.close();throw failure;}
        }
        @Override public void close() throws IOException {
            try {cleanup.getMethod("deregisterOwned").invoke(null);}
            catch(ReflectiveOperationException failure){throw new IOException("Cannot deregister child JDBC drivers",failure);}
            finally {super.close();}
        }
    }
    private static final class Loaded {
        final String id,provider,digest;final ClassLoader loader;final Isolated owned;final List<Driver> drivers;
        int leases;boolean retired;
        Loaded(String provider,String digest,ClassLoader loader,Isolated owned,List<Driver> drivers){this.id=UUID.randomUUID().toString();this.provider=provider;this.digest=digest;this.loader=loader;this.owned=owned;this.drivers=drivers;}
    }
    public final class Lease implements AutoCloseable {
        private Loaded loaded;private final Driver driver;
        private Lease(Loaded loaded,Driver driver){this.loaded=loaded;this.driver=driver;}
        public Driver driver(){if(loaded==null)throw new IllegalStateException("Driver lease closed");return driver;}
        public ClassLoader contextLoader(){if(loaded==null)throw new IllegalStateException("Driver lease closed");return loaded.loader;}
        @Override public void close(){synchronized(LoaderCatalog.this){if(loaded!=null){loaded.leases--;if(loaded.retired&&loaded.leases==0)dispose(loaded);loaded=null;}}}
    }
    private final Map<String,Loaded> byId=new HashMap<>();
    private final Map<String,Loaded> providers=new HashMap<>();
    private final Set<String> changedProviders=new HashSet<>();
    private boolean closed;
    public synchronized ObjectNode inspect(String provider,List<Path> classpath,String selected,TransferControl control) throws Exception {
        control.check();if(closed)throw new RpcException(-32010,"Driver loaders are closed");
        if(provider.isBlank()||provider.length()>128||classpath.size()>InstallStore.MAX_JARS)throw DriverSpec.bad();
        String digest;
        if(classpath.isEmpty())digest="application-classpath";
        else {
            var entries=new ArrayList<InstallStore.Jar>();long bytes=0;
            for(int i=0;i<classpath.size();i++){
                Path path=classpath.get(i).toAbsolutePath().normalize();String hash=InstallStore.hash(path,control);long size=Files.size(path);bytes+=size;if(bytes>InstallStore.MAX_INSTALL)throw new RpcException(-32010,"Driver classpath exceeds 4 GiB");
                // Ordered byte digests agree with immutable install manifests.
                entries.add(new InstallStore.Jar(path.toString(),hash,size,""));
            }
            digest=InstallStore.classpathDigest(entries);
        }
        Loaded old=providers.get(provider);
        if(old!=null&&old.digest.equals(digest))return info(old,selected,false);
        if(old!=null&&old.leases>0)throw new RpcException(-32011,"Active connections use the previous driver; disconnect them or restart the helper",FrameCodec.JSON.createObjectNode().put("restartRequired",true));
        if(old!=null){old.retired=true;dispose(old);changedProviders.add(provider);}
        if(byId.size()>=MAX_LOADERS)throw new RpcException(-32003,"Driver loader capacity exhausted");
        Isolated isolated=null;ClassLoader loader;
        try {
            if(classpath.isEmpty())loader=LoaderCatalog.class.getClassLoader();
            else {URL[] urls=new URL[classpath.size()];for(int i=0;i<urls.length;i++)urls[i]=classpath.get(i).toUri().toURL();isolated=new Isolated(urls);loader=isolated;}
            List<Driver> drivers=discover(loader,selected);
            if(drivers.isEmpty())throw new RpcException(-32010,"No JDBC Driver class was found; configure the driver class or install the required JARs");
            if(classpath.isEmpty()&&!selected.isBlank()&&drivers.stream().noneMatch(driver->driver.getClass().getName().equals(selected)))
                throw new RpcException(-32010,"The selected JDBC driver is not visible; install its JARs");
            control.check();Loaded loaded=new Loaded(provider,digest,loader,isolated,drivers);ObjectNode information=info(loaded,selected,changedProviders.contains(provider));
            byId.put(loaded.id,loaded);providers.put(provider,loaded);return information;
        }catch(Throwable failure){
            if(isolated!=null)try{isolated.close();}catch(Exception ignored){}
            if(failure instanceof RpcException rpc)throw rpc;
            if(failure instanceof UnsatisfiedLinkError||failure.getCause() instanceof UnsatisfiedLinkError)
                throw new RpcException(-32011,"Native JDBC driver could not load; restart the helper before changing native driver versions",FrameCodec.JSON.createObjectNode().put("restartRequired",true));
            if(failure instanceof VirtualMachineError fatal)throw fatal;
            throw new RpcException(-32010,"JDBC driver classes could not be loaded; check the class and complete classpath");
        }
    }
    private static List<Driver> discover(ClassLoader loader,String selected) throws Exception {
        Thread thread=Thread.currentThread();ClassLoader previous=thread.getContextClassLoader();thread.setContextClassLoader(loader);
        try {
            List<Driver> drivers=new ArrayList<>();
            try {
                for(Driver driver:ServiceLoader.load(Driver.class,loader)){if(drivers.size()>=MAX_DRIVERS)throw new RpcException(-32010,"Driver provider count exceeds 128");drivers.add(driver);}
            }catch(ServiceConfigurationError failure){
                // Preserve explicit-class fallback even for a broken service
                // descriptor; unlike the source, linkage failures remain visible
                // if neither strategy finds a usable driver.
            }
            boolean matching=drivers.stream().anyMatch(driver->driver.getClass().getName().equals(selected));
            if(!selected.isBlank()&&!matching) {
                try {Object value=Class.forName(selected,true,loader).getDeclaredConstructor().newInstance();if(value instanceof Driver driver)drivers.add(driver);}
                catch(ReflectiveOperationException|ClassCastException failure){if(drivers.isEmpty())throw failure;}
            }
            return List.copyOf(drivers);
        } finally {thread.setContextClassLoader(previous);}
    }
    private static Driver selected(Loaded loaded,String requested) {
        return loaded.drivers.stream().filter(driver->driver.getClass().getName().equals(requested)).findFirst().orElse(loaded.drivers.getFirst());
    }
    private static ObjectNode info(Loaded loaded,String requested,boolean changed) {
        Driver driver=selected(loaded,requested);
        ObjectNode value=FrameCodec.JSON.createObjectNode().put("providerId",loaded.provider).put("classpathDigest",loaded.digest).put("loaderId",loaded.id)
            .put("selectedDriverClass",driver.getClass().getName()).put("driverMajorVersion",driver.getMajorVersion()).put("driverMinorVersion",driver.getMinorVersion()).put("restartRequired",false).put("restartRecommended",changed)
            .put("source",loaded.owned==null?"application-classpath":"isolated-classpath");
        var names=value.putArray("driverClasses");loaded.drivers.forEach(item->names.add(item.getClass().getName()));return value;
    }
    public synchronized Lease acquire(String loaderId,String driverClass) throws RpcException {
        Loaded loaded=byId.get(loaderId);if(closed||loaded==null||loaded.retired)throw new RpcException(-32012,"Driver loader ID is stale; inspect the driver again");
        loaded.leases++;return new Lease(loaded,selected(loaded,driverClass));
    }
    private void dispose(Loaded loaded) {
        if(loaded.leases>0)return;
        if(loaded.owned!=null)try{loaded.owned.close();}catch(IOException failure){changedProviders.add(loaded.provider);}
        byId.remove(loaded.id);providers.remove(loaded.provider,loaded);
    }
    @Override public synchronized void close(){closed=true;for(Loaded loaded:new ArrayList<>(byId.values())){loaded.retired=true;dispose(loaded);}}
}
