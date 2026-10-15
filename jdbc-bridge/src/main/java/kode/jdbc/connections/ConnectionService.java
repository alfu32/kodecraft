package kode.jdbc.connections;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.drivers.LoaderCatalog;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import kode.jdbc.protocol.RpcServer;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.sql.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicBoolean;

/** One JDBC owner per datasource. Only abort/cancel may run outside that owner.
 * Request handlers never wait for JDBC or the output pipe on a dispatcher.
 */
public final class ConnectionService implements AutoCloseable {
    private static final int MAX_CONNECTIONS=32,MAX_QUEUE=16;
    @FunctionalInterface public interface Work { JsonNode run(Session session) throws Exception; }
    private final LoaderCatalog loaders;
    private final Map<String,Session> sources=new HashMap<>(),ids=new HashMap<>();
    private final Map<String,Session> attempts=new HashMap<>();
    // A priority close can reach its dispatcher before an earlier ordinary open.
    // Keep bounded tombstones so that abandoned opens cannot appear afterwards.
    private final Set<String> cancelledAttempts=new HashSet<>();
    private final ThreadPoolExecutor tests=pool("jdbc-test",4,16),aborts=pool("jdbc-abort",2,32);
    private final Set<Session> temporary=ConcurrentHashMap.newKeySet();
    private final AtomicBoolean stopping=new AtomicBoolean(),needsRestart=new AtomicBoolean();

    public ConnectionService(LoaderCatalog loaders){this.loaders=loaders;}
    private static ThreadPoolExecutor pool(String name,int threads,int queued){return new ThreadPoolExecutor(threads,threads,0,TimeUnit.MILLISECONDS,new ArrayBlockingQueue<>(queued),Thread.ofPlatform().daemon(true).name(name+"-",0).factory(),new ThreadPoolExecutor.AbortPolicy());}
    public void register(RpcServer server){
        server.register("connection.open",false,(p,c)->open(p));
        server.register("connection.test",false,(p,c)->test(p));
        server.register("connection.close",true,(p,c)->disconnect(p));
        server.register("connection.setContext",false,(p,c)->submit(id(p,"connectionId"),s->s.setContext(p)));
        server.register("connection.commit",false,(p,c)->submit(id(p,"connectionId"),s->s.transaction(false)));
        server.register("connection.rollback",false,(p,c)->submit(id(p,"connectionId"),s->s.transaction(true)));
        server.onClose(this);
    }
    static RpcException rejected(int code,String text){return new RpcException(code,text,FrameCodec.JSON.createObjectNode().put("notStarted",true));}
    public static String id(ObjectNode value,String key) throws RpcException {
        JsonNode node=value.get(key);if(node==null||!node.isTextual()||node.textValue().isBlank()||node.textValue().getBytes(StandardCharsets.UTF_8).length>128||node.textValue().codePoints().anyMatch(Character::isISOControl))throw rejected(-32602,"Missing or invalid identifier");return node.textValue();
    }
    private static String text(ObjectNode value,String key,int max,boolean optional) throws RpcException {
        JsonNode node=value.get(key);if(optional&&(node==null||node.isNull()))return null;
        if(node==null||!node.isTextual()||node.textValue().getBytes(StandardCharsets.UTF_8).length>max)throw rejected(-32602,"Invalid connection parameter");return node.textValue();
    }
    private static boolean bool(ObjectNode value,String key,boolean fallback) throws RpcException {
        JsonNode node=value.get(key);if(node==null)return fallback;if(!node.isBoolean())throw rejected(-32602,"Invalid connection flag");return node.booleanValue();
    }
    private record Opening(String source,String attempt,String loader,String driver,String url,Properties properties,SqlErrors.Redactor redactor,String fingerprint,String catalog,String schema,boolean autoCommit,boolean readOnly) {
        static Opening parse(ObjectNode p) throws Exception {
            String source=id(p,"dataSourceId"),loader=id(p,"loaderId"),driver=text(p,"driverClass",1024,false),url=text(p,"jdbcUrl",65536,false);
            if(driver.isBlank()||url.isBlank()||!(p.get("properties") instanceof ObjectNode values)||values.size()>256)throw rejected(-32602,"Invalid connection properties");
            Properties properties=new Properties();List<String> secretValues=new ArrayList<>();
            for(var entry:values.properties()){
                if(entry.getKey().isBlank()||entry.getKey().length()>1024||!entry.getValue().isTextual()||entry.getValue().textValue().length()>65536)throw rejected(-32602,"Invalid connection property");
                String value=entry.getValue().textValue();properties.setProperty(entry.getKey(),value);secretValues.add(value);
            }
            String catalog=text(p,"catalog",4096,true),schema=text(p,"schema",4096,true);boolean autoCommit=bool(p,"autoCommit",true),readOnly=bool(p,"readOnly",false);
            // Canonical ordering means equivalent property insertion orders share
            // one session. Retain a digest rather than the credential payload.
            var canonical=FrameCodec.JSON.createObjectNode().put("loader",loader).put("driver",driver).put("url",url).put("catalog",catalog).put("schema",schema).put("autoCommit",autoCommit).put("readOnly",readOnly);
            ObjectNode sorted=canonical.putObject("properties");new TreeSet<>(properties.stringPropertyNames()).forEach(key->sorted.put(key,properties.getProperty(key)));
            String fingerprint=HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(FrameCodec.JSON.writeValueAsBytes(canonical)));
            return new Opening(source,p.has("attemptId")?id(p,"attemptId"):UUID.randomUUID().toString(),loader,driver,url,properties,new SqlErrors.Redactor(secretValues),fingerprint,catalog,schema,autoCommit,readOnly);
        }
        void clear(){properties.clear();}
    }
    private void available() throws RpcException {
        if(stopping.get())throw rejected(SqlErrors.STALE_CONNECTION,"Connection service is stopping");
        if(needsRestart.get())throw rejected(SqlErrors.NEEDS_RESTART,"A connection could not be closed; restart the helper");
        if(cancelledAttempts.size()>=4096)throw rejected(SqlErrors.NEEDS_RESTART,"Connection cancellation history is full; restart the helper");
    }
    public synchronized CompletionStage<JsonNode> open(ObjectNode params) throws Exception {
        available();Opening opening=Opening.parse(params);Session existing=sources.get(opening.source());
        if(cancelledAttempts.contains(opening.attempt())){opening.clear();opening.redactor().clear();throw rejected(SqlErrors.STALE_CONNECTION,"Connection opening was cancelled before it started");}
        if(existing!=null){opening.clear();opening.redactor().clear();if(!existing.closing.get()&&existing.fingerprint.equals(opening.fingerprint()))return existing.enqueue(Session::snapshot);throw rejected(SqlErrors.DEFINITION_CHANGED,"Datasource is already open or closing; disconnect before changing its definition");}
        if(attempts.containsKey(opening.attempt())){opening.clear();opening.redactor().clear();throw rejected(-32602,"Connection attempt ID is already active");}
        if(sources.size()>=MAX_CONNECTIONS){opening.clear();opening.redactor().clear();throw rejected(-32003,"Connection capacity exhausted");}
        Session session=new Session(opening);sources.put(session.source,session);ids.put(session.id,session);attempts.put(session.attempt,session);
        session.worker=Thread.ofPlatform().daemon(true).name("jdbc-connection-"+session.id).start(()->session.loop(opening));return session.opened;
    }
    public synchronized CompletionStage<JsonNode> test(ObjectNode params) throws Exception {
        available();Opening opening=Opening.parse(params);if(cancelledAttempts.contains(opening.attempt())){opening.clear();opening.redactor().clear();throw rejected(SqlErrors.STALE_CONNECTION,"Connection test was cancelled before it started");}if(attempts.containsKey(opening.attempt())){opening.clear();opening.redactor().clear();throw rejected(-32602,"Connection attempt ID is already active");}Session session=new Session(opening);temporary.add(session);attempts.put(session.attempt,session);
        try{tests.execute(new TestJob(opening,session));}catch(RejectedExecutionException failure){temporary.remove(session);forget(session);opening.clear();session.redactor.clear();throw rejected(-32003,"Connection test queue exhausted");}
        return session.opened;
    }
    private final class TestJob implements Runnable {
        private final Opening opening;private final Session session;private final AtomicBoolean started=new AtomicBoolean();
        TestJob(Opening opening,Session session){this.opening=opening;this.session=session;}
        @Override public void run(){
            if(!started.compareAndSet(false,true))return;
            ClassLoader previous=Thread.currentThread().getContextClassLoader();
            session.worker=Thread.currentThread();long start=System.nanoTime();JsonNode result=null;Throwable failure=null;
            try{session.connect(opening);ObjectNode info=session.snapshot();info.remove("connectionId");info.put("success",true).put("latencyMs",TimeUnit.NANOSECONDS.toMillis(System.nanoTime()-start));result=info;}
            catch(Throwable error){failure=error;}
            finally{opening.clear();}
            try{session.closeOwned();}catch(Throwable error){if(failure==null)failure=error;else failure.addSuppressed(error);}
            if(failure==null)session.opened.complete(result);else session.opened.completeExceptionally(SqlErrors.error(failure,session.redactor,null,false));
            if(session.released){session.closed.complete(FrameCodec.JSON.createObjectNode().put("closed",true));session.redactor.clear();temporary.remove(session);forget(session);}else session.closed.completeExceptionally(session.restartError());
            Thread.currentThread().setContextClassLoader(previous);
        }
        void cancelQueued(){if(started.compareAndSet(false,true)){opening.clear();session.redactor.clear();temporary.remove(session);forget(session);session.opened.completeExceptionally(rejected(SqlErrors.STALE_CONNECTION,"Connection test stopped before it started"));session.closed.complete(FrameCodec.JSON.createObjectNode().put("closed",true));}}
    }
    public synchronized CompletionStage<JsonNode> disconnect(ObjectNode params) throws Exception {
        Session session;
        if(params.has("attemptId")){
            String attempt=id(params,"attemptId");if(cancelledAttempts.size()>=4096&&!cancelledAttempts.contains(attempt))throw rejected(SqlErrors.NEEDS_RESTART,"Connection cancellation history is full; restart the helper");cancelledAttempts.add(attempt);session=attempts.get(attempt);
            if(session==null)return RpcServer.result(FrameCodec.JSON.createObjectNode().put("closed",true).put("alreadyClosed",true));
        }
        else if(params.has("connectionId")){session=ids.get(id(params,"connectionId"));if(session==null)throw rejected(SqlErrors.STALE_CONNECTION,"Connection ID is stale; reconnect explicitly");}
        else {session=sources.get(id(params,"dataSourceId"));if(session==null)return RpcServer.result(FrameCodec.JSON.createObjectNode().put("closed",true).put("alreadyClosed",true));}
        session.requestClose();return session.closed;
    }
    public synchronized CompletionStage<JsonNode> submit(String connectionId,Work work) throws RpcException {
        available();Session session=ids.get(connectionId);if(session==null||session.closing.get())throw rejected(SqlErrors.STALE_CONNECTION,"Connection ID is stale or closing; reconnect explicitly");
        return session.enqueue(work);
    }
    private synchronized void forget(Session session){sources.remove(session.source,session);ids.remove(session.id,session);attempts.remove(session.attempt,session);}
    private static final class Command {
        final Work work;final CompletableFuture<JsonNode> future=new CompletableFuture<>();
        Command(Work work){this.work=work;}
    }
    public final class Session {
        public final String id=UUID.randomUUID().toString(),source,attempt;
        private final String fingerprint;
        public final SqlErrors.Redactor redactor;
        private final ArrayBlockingQueue<Command> queue=new ArrayBlockingQueue<>(MAX_QUEUE+1);
        private final Semaphore slots=new Semaphore(MAX_QUEUE);
        private final AtomicBoolean closing=new AtomicBoolean();
        private final CompletableFuture<JsonNode> opened=new CompletableFuture<>(),closed=new CompletableFuture<>();
        private final Command closeCommand=new Command(null);
        private volatile Thread worker;
        private volatile Connection connection;
        private LoaderCatalog.Lease lease;
        private boolean released,transactionUsable=true,broken,transactions;
        private long contextRevision,stateRevision;
        private String loginCatalog,loginSchema;
        private ObjectNode info;
        private volatile Runnable cancelActive;
        Session(Opening opening){source=opening.source();attempt=opening.attempt();fingerprint=opening.fingerprint();redactor=opening.redactor();}
        private void owner(){if(Thread.currentThread()!=worker)throw new IllegalStateException("JDBC operation outside its owner");}
        public Connection connection() throws SQLException {owner();if(connection==null||broken||connection.isClosed())throw new SQLException("Connection is unavailable","08003");return connection;}
        /** Cancellation hook must refer to this execution, and must itself be
         * nonblocking (schedule Statement.cancel on its independent executor). */
        public void cancellation(Runnable callback){owner();cancelActive=callback;if(callback!=null&&closing.get())callback.run();}
        public boolean closing(){return closing.get()||stopping.get();}
        public boolean transactionUsable(){owner();return transactionUsable&&!broken;}
        public void failed(Throwable failure){owner();SQLException sql=SqlErrors.find(failure);if(sql!=null&&sql.getSQLState()!=null){String state=sql.getSQLState();if(state.startsWith("08")){broken=true;transactionUsable=false;}else if(state.startsWith("25")||state.startsWith("40"))transactionUsable=false;}}
        public ObjectNode snapshot() throws Exception {
            owner();Connection conn=connection();ObjectNode state=info.deepCopy().put("connectionId",id).put("dataSourceId",source).put("contextRevision",contextRevision).put("stateRevision",++stateRevision).put("transactionUsable",transactionUsable).put("autoCommit",conn.getAutoCommit()).put("readOnly",conn.isReadOnly());
            nullable(state,"catalog",catalog(conn));nullable(state,"schema",schema(conn));return state;
        }
        private ObjectNode safeSnapshot(){try{return snapshot();}catch(Exception ignored){return FrameCodec.JSON.createObjectNode().put("connectionId",id).put("dataSourceId",source).put("contextRevision",contextRevision).put("stateRevision",++stateRevision).put("transactionUsable",false).put("contextUnknown",true);}}
        private void connect(Opening opening) throws Exception {
            owner();if(closing())throw rejected(SqlErrors.STALE_CONNECTION,"Connection opening was cancelled");
            lease=loaders.acquire(opening.loader(),opening.driver());Thread.currentThread().setContextClassLoader(lease.contextLoader());
            try{connection=lease.driver().connect(opening.url(),opening.properties());}finally{opening.clear();}
            if(connection==null)throw new SQLException("The selected JDBC driver does not accept this URL","08001");
            if(closing())throw new SQLException("Connection opening was interrupted by disconnect","08003");
            Connection conn=connection;DatabaseMetaData metadata=conn.getMetaData();transactions=metadata.supportsTransactions();
            info=FrameCodec.JSON.createObjectNode().put("productName",redactor.text(metadata.getDatabaseProductName())).put("productVersion",redactor.text(metadata.getDatabaseProductVersion())).put("driverName",redactor.text(metadata.getDriverName())).put("driverVersion",redactor.text(metadata.getDriverVersion()));
            info.putObject("capabilities").put("transactions",transactions).put("savepoints",metadata.supportsSavepoints())
                .put("catalogs",metadata.supportsCatalogsInDataManipulation()).put("schemas",metadata.supportsSchemasInDataManipulation());
            loginCatalog=catalog(conn);loginSchema=schema(conn);
            ObjectNode context=FrameCodec.JSON.createObjectNode().put("autoCommit",opening.autoCommit()).put("readOnly",opening.readOnly());
            if(opening.catalog()!=null&&!opening.catalog().isBlank())context.put("catalog",opening.catalog());if(opening.schema()!=null&&!opening.schema().isBlank())context.put("schema",opening.schema());
            setContext(context);
        }
        private void loop(Opening opening){
            try{
                try{connect(opening);opened.complete(snapshot());}
                catch(Throwable failure){
                    try{closeOwned();}catch(Throwable closeFailure){failure.addSuppressed(closeFailure);}
                    opened.completeExceptionally(SqlErrors.error(failure,redactor,null,false));
                    if(released){closed.complete(FrameCodec.JSON.createObjectNode().put("closed",true));return;}
                    closed.completeExceptionally(restartError());return;
                }finally{opening.clear();}
                while(true){
                    Command command;
                    try{command=queue.take();}catch(InterruptedException ignored){continue;}
                    if(command==closeCommand){Thread.interrupted();try{closeOwned();closed.complete(FrameCodec.JSON.createObjectNode().put("closed",true));}catch(Throwable failure){closed.completeExceptionally(SqlErrors.error(failure,redactor,safeSnapshot(),false));}return;}
                    slots.release();
                    if(closing()){command.future.completeExceptionally(rejected(SqlErrors.STALE_CONNECTION,"Connection closed before operation started"));continue;}
                    Thread.interrupted();
                    try{command.future.complete(command.work.run(this));}
                    catch(Throwable failure){failed(failure);command.future.completeExceptionally(SqlErrors.error(failure,redactor,safeSnapshot(),false));}
                    finally{cancelActive=null;}
                }
            }finally{
                Command pending;while((pending=queue.poll())!=null)if(pending!=closeCommand){slots.release();pending.future.completeExceptionally(rejected(SqlErrors.STALE_CONNECTION,"Connection ended before operation started"));}
                Thread.currentThread().setContextClassLoader(ClassLoader.getPlatformClassLoader());
                if(released){forget(this);redactor.clear();}
            }
        }
        private CompletionStage<JsonNode> enqueue(Work work) throws RpcException {
            if(!slots.tryAcquire())throw rejected(-32003,"Connection work queue exhausted");Command command=new Command(work);
            if(!queue.offer(command)){slots.release();throw rejected(-32003,"Connection work queue exhausted");}return command.future;
        }
        private void requestClose(){
            if(!closing.compareAndSet(false,true))return;
            Command queued;while((queued=queue.poll())!=null){slots.release();queued.future.completeExceptionally(rejected(SqlErrors.STALE_CONNECTION,"Connection closed before operation started"));}
            queue.add(closeCommand);Runnable cancel=cancelActive;if(cancel!=null)try{cancel.run();}catch(RuntimeException ignored){}
            Thread thread=worker;if(thread!=null)thread.interrupt();Connection owned=connection;
            if(owned!=null)try{aborts.execute(()->{try{owned.abort(Runnable::run);}catch(SQLException|RuntimeException ignored){/* Owner still attempts close; it reports an unconfirmed close. */}});}catch(RejectedExecutionException ignored){}
        }
        private RpcException restartError(){return new RpcException(SqlErrors.NEEDS_RESTART,"Connection close could not be confirmed; restart the helper",FrameCodec.JSON.createObjectNode().put("restartRequired",true));}
        private void closeOwned() throws Exception {
            owner();Thread.interrupted();Throwable warning=null;
            if(connection!=null){
                try{connection.close();if(!connection.isClosed())throw restartError();}
                catch(Throwable failure){boolean reallyClosed=false;try{reallyClosed=connection.isClosed();}catch(Throwable ignored){}if(!reallyClosed){needsRestart.set(true);throw failure;}warning=failure;}
                connection=null;
            }
            if(lease!=null){lease.close();lease=null;}released=true;forget(this);
            if(warning!=null){ObjectNode data=FrameCodec.JSON.createObjectNode().put("closed",true).put("notStarted",false);data.set("sqlError",SqlErrors.wire(warning,redactor));throw new RpcException(SqlErrors.SQL_FAILURE,"Connection closed with a driver error",data);}
        }
        public JsonNode setContext(ObjectNode params) throws Exception {
            owner();Connection conn=connection();
            String newCatalog=params.has("catalog")?(params.get("catalog").isNull()?loginCatalog:text(params,"catalog",4096,false)):null;
            String newSchema=params.has("schema")?(params.get("schema").isNull()?loginSchema:text(params,"schema",4096,false)):null;
            boolean auto=bool(params,"autoCommit",conn.getAutoCommit()),readOnly=bool(params,"readOnly",conn.isReadOnly());
            if(!auto&&!transactions)throw rejected(SqlErrors.UNSUPPORTED,"Driver does not support transactions");
            if(!transactionUsable&&auto&&!conn.getAutoCommit())throw rejected(SqlErrors.UNSUPPORTED,"Rollback the failed transaction before enabling autocommit");
            boolean changing=false;
            try{
                if(params.has("catalog")&&!Objects.equals(catalog(conn),newCatalog)){changing=true;conn.setCatalog(newCatalog);if(!Objects.equals(catalog(conn),newCatalog))throw new SQLFeatureNotSupportedException("Driver did not apply the requested catalog");}
                if(params.has("schema")&&!Objects.equals(schema(conn),newSchema)){changing=true;conn.setSchema(newSchema);if(!Objects.equals(schema(conn),newSchema))throw new SQLFeatureNotSupportedException("Driver did not apply the requested schema");}
                if(conn.isReadOnly()!=readOnly){changing=true;conn.setReadOnly(readOnly);if(conn.isReadOnly()!=readOnly)throw new SQLFeatureNotSupportedException("Driver did not apply read-only mode");}
                if(conn.getAutoCommit()!=auto){if(!transactionUsable&&auto)throw rejected(SqlErrors.UNSUPPORTED,"Rollback the failed transaction before enabling autocommit");changing=true;conn.setAutoCommit(auto);if(conn.getAutoCommit()!=auto)throw new SQLFeatureNotSupportedException("Driver did not apply autocommit");transactionUsable=true;}
            }finally{if(changing)contextRevision++;}
            return snapshot();
        }
        public JsonNode transaction(boolean rollback) throws Exception {
            owner();Connection conn=connection();if(!transactions||conn.getAutoCommit())throw rejected(SqlErrors.UNSUPPORTED,"Transaction control requires supported manual commit mode");
            if(!rollback&&!transactionUsable)throw rejected(SqlErrors.UNSUPPORTED,"Rollback the failed transaction before committing");
            if(rollback)conn.rollback();else conn.commit();transactionUsable=true;return snapshot();
        }
    }
    private static void nullable(ObjectNode node,String key,String value){if(value==null)node.putNull(key);else node.put(key,value);}
    private static String catalog(Connection conn) throws SQLException {try{return conn.getCatalog();}catch(SQLFeatureNotSupportedException ignored){return null;}}
    private static String schema(Connection conn) throws SQLException {try{return conn.getSchema();}catch(SQLFeatureNotSupportedException|AbstractMethodError ignored){return null;}}
    @Override public void close(){
        if(!stopping.compareAndSet(false,true))return;List<Session> sessions;synchronized(this){sessions=new ArrayList<>(sources.values());for(Session session:sessions)session.requestClose();}
        for(Session session:temporary){session.closing.set(true);Thread thread=session.worker;if(thread!=null)thread.interrupt();}
        for(Runnable pending:tests.shutdownNow())if(pending instanceof TestJob job)job.cancelQueued();aborts.shutdown();
        long deadline=System.nanoTime()+TimeUnit.SECONDS.toNanos(1);
        for(Session session:sessions)try{long remaining=deadline-System.nanoTime();if(remaining>0)session.closed.get(remaining,TimeUnit.NANOSECONDS);}catch(Exception ignored){}
        // A driver can ignore close/abort/interrupt. Do not release its loader or
        // claim rollback; parent supervision terminates the entire helper.
    }
}
