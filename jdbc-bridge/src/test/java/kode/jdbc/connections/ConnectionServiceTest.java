package kode.jdbc.connections;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.drivers.LoaderCatalog;
import kode.jdbc.drivers.TransferControl;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import java.lang.reflect.*;
import java.sql.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.jupiter.api.Assertions.*;

@Timeout(15)
public class ConnectionServiceTest {
    static ObjectNode object(){return FrameCodec.JSON.createObjectNode();}
    static JsonNode await(CompletionStage<JsonNode> stage) throws Exception{try{return stage.toCompletableFuture().get(5,TimeUnit.SECONDS);}catch(ExecutionException failure){if(failure.getCause() instanceof RpcException rpc)throw new ExecutionException("Synthetic fixture RPC "+rpc.code+": "+rpc.data,rpc);throw failure;}}
    static RpcException error(CompletionStage<JsonNode> stage){ExecutionException error=assertThrows(ExecutionException.class,()->await(stage));assertInstanceOf(RpcException.class,error.getCause());return (RpcException)error.getCause();}
    static final class Fixture implements AutoCloseable {
        final LoaderCatalog loaders=new LoaderCatalog();final ConnectionService service=new ConnectionService(loaders);final String loader;
        Fixture() throws Exception {this("org.h2.Driver");}
        Fixture(String driver) throws Exception {loader=loaders.inspect("fixture",List.of(),driver,new TransferControl("test",()->false,null)).path("loaderId").asText();}
        ObjectNode params(String source){ObjectNode p=object().put("dataSourceId",source).put("loaderId",loader).put("driverClass","org.h2.Driver").put("jdbcUrl","jdbc:h2:mem:"+UUID.randomUUID());p.putObject("properties").put("user","sa");return p;}
        JsonNode sql(String id,String sql) throws Exception {return await(service.submit(id,s->{try(var statement=s.connection().createStatement()){boolean rows=statement.execute(sql);ObjectNode result=object().put("hasRows",rows);if(rows)try(var cursor=statement.getResultSet()){assertTrue(cursor.next());result.put("value",cursor.getString(1));}else result.put("count",statement.getUpdateCount());return result;}}));}
        @Override public void close(){service.close();loaders.close();}
    }
    @Test void temporaryTestReturnsH2MetadataAndLatencyAndNeverReplacesLiveSession() throws Exception {
        try(Fixture f=new Fixture()){
            ObjectNode p=f.params("same");JsonNode open=await(f.service.open(p));String id=open.path("connectionId").asText();
            JsonNode test=await(f.service.test(p));assertTrue(test.path("success").asBoolean());assertEquals("H2",test.path("productName").asText());assertFalse(test.path("productVersion").asText().isBlank());assertFalse(test.path("driverVersion").asText().isBlank());assertTrue(test.path("latencyMs").asLong()>=0);assertFalse(test.has("connectionId"));
            assertEquals(id,await(f.service.open(p)).path("connectionId").asText());assertEquals("1",f.sql(id,"select 1").path("value").asText());
        }
    }
    @Test void concurrentOwnersArePublishedBeforeStartAndConfirmedCloseAllowsImmediateReopen() throws Exception {
        try(Fixture f=new Fixture()){
            for(int round=0;round<2;round++){
                List<ObjectNode> params=new ArrayList<>();List<CompletionStage<JsonNode>> opening=new ArrayList<>();
                for(int i=0;i<24;i++){ObjectNode p=f.params("parallel-"+round+"-"+i);params.add(p);opening.add(f.service.open(p));}
                for(int i=0;i<24;i++){
                    String id=await(opening.get(i)).path("connectionId").asText();assertEquals("1",f.sql(id,"select 1").path("value").asText());
                    await(f.service.disconnect(object().put("connectionId",id)));String next=await(f.service.open(params.get(i))).path("connectionId").asText();assertNotEquals(id,next);await(f.service.disconnect(object().put("connectionId",next)));
                }
            }
        }
    }
    @Test void manualTransactionsAreSharedAcrossConsolesRollbackCommitAndAutoCommitAreAuthoritative() throws Exception {
        try(Fixture f=new Fixture()){
            ObjectNode p=f.params("tx").put("autoCommit",false);JsonNode opened=await(f.service.open(p));String first=opened.path("connectionId").asText(),second=await(f.service.open(p)).path("connectionId").asText();assertEquals(first,second);
            assertTrue(opened.path("capabilities").path("transactions").asBoolean());f.sql(first,"create table item(id int primary key, name varchar(40))");
            assertEquals(1,f.sql(first,"insert into item values (1, 'draft')").path("count").asInt());assertEquals(1,f.sql(second,"update item set name='changed' where id=1").path("count").asInt());assertEquals("changed",f.sql(first,"select name from item").path("value").asText());
            JsonNode rolledBack=await(f.service.submit(first,s->s.connection().getAutoCommit()?object():s.setContext(object())));assertFalse(rolledBack.path("autoCommit").asBoolean());
            // Use the registered transaction operation through the same owner API.
            await(f.service.submit(first,s->s.transaction(true)));assertEquals("0",f.sql(second,"select count(*) from item").path("value").asText());
            f.sql(first,"insert into item values (2, 'committed')");await(f.service.submit(first,s->s.transaction(false)));await(f.service.submit(first,s->s.transaction(true)));assertEquals("1",f.sql(second,"select count(*) from item").path("value").asText());
            f.sql(first,"insert into item values (3, 'auto')");JsonNode state=await(f.service.submit(first,s->s.setContext(object().put("autoCommit",true))));assertTrue(state.path("autoCommit").asBoolean());assertTrue(state.path("contextRevision").asLong()>opened.path("contextRevision").asLong());assertTrue(await(f.service.open(p)).path("autoCommit").asBoolean());
            await(f.service.disconnect(object().put("connectionId",first)));assertThrows(RpcException.class,()->f.service.submit(first,s->object()));String next=await(f.service.open(p)).path("connectionId").asText();assertNotEquals(first,next);
        }
    }
    @Test void badUrlAndAuthenticationReturnSqlStateVendorCodeAndNoCredentialValues() throws Exception {
        try(Fixture f=new Fixture()){
            ObjectNode p=f.params("auth").put("jdbcUrl","jdbc:h2:mem:"+UUID.randomUUID()+";DB_CLOSE_DELAY=-1");p.withObject("properties").put("password","synthetic-right-secret");await(f.service.open(p));
            ObjectNode bad=p.deepCopy();bad.withObject("properties").put("password","synthetic-wrong-secret");RpcException auth=error(f.service.test(bad));assertEquals("28000",auth.data.path("sqlError").path("sqlState").asText());assertNotEquals(0,auth.data.path("sqlError").path("vendorCode").asInt());assertFalse(auth.data.toString().contains("synthetic-wrong-secret"));
            RpcException url=error(f.service.test(f.params("bad-url").put("jdbcUrl","jdbc:unrecognized:test")));assertEquals("08001",url.data.path("sqlError").path("sqlState").asText());
        }
    }
    @Test void definitionChangesRequireDisconnectAndContextFailureReportsActualState() throws Exception {
        try(Fixture f=new Fixture()){
            ObjectNode p=f.params("definition");String id=await(f.service.open(p)).path("connectionId").asText();ObjectNode changed=p.deepCopy().put("autoCommit",false);
            RpcException failure=assertThrows(RpcException.class,()->f.service.open(changed));assertEquals(SqlErrors.DEFINITION_CHANGED,failure.code);assertTrue(failure.data.path("notStarted").asBoolean());
            RpcException context=error(f.service.submit(id,s->s.setContext(object().put("autoCommit",false).put("schema","missing_schema"))));assertTrue(context.data.has("sqlError"));assertTrue(context.data.path("state").path("autoCommit").asBoolean());assertEquals("PUBLIC",context.data.path("state").path("schema").asText());
            // H2 ignores read-only mode; the bridge must report unsupported rather
            // than inventing enforcement or claiming the requested mode succeeded.
            RpcException readOnly=error(f.service.submit(id,s->s.setContext(object().put("readOnly",true))));assertFalse(readOnly.data.path("state").path("readOnly").asBoolean());
            await(f.service.submit(id,s->s.setContext(object().put("schema","INFORMATION_SCHEMA"))));JsonNode reset=await(f.service.submit(id,s->s.setContext(object().putNull("schema"))));assertEquals("PUBLIC",reset.path("schema").asText());
        }
    }
    @Test void failedTransactionRequiresRollbackAndSqlErrorChainsAreBoundedAndRedacted() throws Exception {
        try(Fixture f=new Fixture()){
            String id=await(f.service.open(f.params("failed").put("autoCommit",false))).path("connectionId").asText();
            RpcException failure=error(f.service.submit(id,s->{SQLException first=new SQLException("password=hidden token:private","40001",99),next=new SQLException("secondary","XX000",12);first.setNextException(next);next.setNextException(first);throw first;}));
            assertFalse(failure.data.path("state").path("transactionUsable").asBoolean());assertEquals(1,failure.data.path("sqlError").path("chained").size());assertFalse(failure.data.toString().contains("hidden"));assertFalse(failure.data.toString().contains("private"));
            assertEquals(SqlErrors.UNSUPPORTED,error(f.service.submit(id,s->s.setContext(object().put("autoCommit",true)))).code);
            assertTrue(await(f.service.submit(id,s->s.transaction(true))).path("transactionUsable").asBoolean());
            await(f.service.submit(id,s->s.transaction(false)));
        }
    }
    @Test void priorityCloseBeforeOpenPreventsLateHiddenConnectionAndDoesNotCloseANewerAttempt() throws Exception {
        try(Fixture f=new Fixture()){
            assertTrue(await(f.service.disconnect(object().put("attemptId","abandoned"))).path("closed").asBoolean());
            ObjectNode p=f.params("cancelled").put("attemptId","abandoned");assertEquals(SqlErrors.STALE_CONNECTION,assertThrows(RpcException.class,()->f.service.open(p)).code);
            p.put("attemptId","new-attempt");String id=await(f.service.open(p)).path("connectionId").asText();await(f.service.disconnect(object().put("attemptId","abandoned")));assertEquals("1",f.sql(id,"select 1").path("value").asText());
        }
    }
    public static final class TrackingDriver implements Driver {
        static final Map<String,Track> tracks=new ConcurrentHashMap<>();
        static final class Track {final Set<Thread> owners=ConcurrentHashMap.newKeySet();final CountDownLatch aborted=new CountDownLatch(1);final AtomicInteger closeCount=new AtomicInteger();volatile boolean failClose,warnAfterClose;}
        public Connection connect(String url,Properties properties) throws SQLException {
            if(!acceptsURL(url))return null;Track track=tracks.get(url);Connection delegate=new org.h2.Driver().connect(url.replace("jdbc:tracked:","jdbc:h2:"),properties);track.owners.add(Thread.currentThread());
            return (Connection)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{Connection.class},(proxy,method,args)->{
                if(method.getName().equals("abort")){track.aborted.countDown();return null;}
                track.owners.add(Thread.currentThread());if(method.getName().equals("close")){track.closeCount.incrementAndGet();if(track.failClose)throw new SQLException("Synthetic close failure","08006");}
                try{Object value=method.invoke(delegate,args);if(method.getName().equals("close")&&track.warnAfterClose)throw new SQLException("Synthetic failure after confirmed close","08006");return value;}catch(InvocationTargetException failure){throw failure.getCause();}
            });
        }
        public boolean acceptsURL(String url){return url.startsWith("jdbc:tracked:");}
        public DriverPropertyInfo[] getPropertyInfo(String url,Properties p){return new DriverPropertyInfo[0];}
        public int getMajorVersion(){return 1;}public int getMinorVersion(){return 0;}public boolean jdbcCompliant(){return false;}public java.util.logging.Logger getParentLogger(){return java.util.logging.Logger.getGlobal();}
    }
    @Test void disconnectWhileBusyUsesIndependentAbortRejectsQueuedWorkAndClosesOnOwner() throws Exception {
        String url="jdbc:tracked:mem:"+UUID.randomUUID();TrackingDriver.Track track=new TrackingDriver.Track();TrackingDriver.tracks.put(url,track);
        try(Fixture f=new Fixture(TrackingDriver.class.getName())){
            String id=await(f.service.open(f.params("busy").put("jdbcUrl",url).put("driverClass",TrackingDriver.class.getName()))).path("connectionId").asText();CountDownLatch entered=new CountDownLatch(1);
            CompletionStage<JsonNode> active=f.service.submit(id,s->{entered.countDown();while(track.aborted.getCount()>0)try{track.aborted.await();}catch(InterruptedException ignored){}return object().put("finished",true);});assertTrue(entered.await(1,TimeUnit.SECONDS));
            CompletionStage<JsonNode> queued=f.service.submit(id,s->{fail("Queued work must not run after disconnect");return object();});
            assertTrue(await(f.service.disconnect(object().put("connectionId",id))).path("closed").asBoolean());assertTrue(await(active).path("finished").asBoolean());assertTrue(error(queued).data.path("notStarted").asBoolean());assertEquals(1,track.closeCount.get());assertEquals(1,track.owners.size());
        }finally{TrackingDriver.tracks.remove(url);}
    }
    @Test void disconnectAfterAnAcknowledgedOperationClosesNormallyWithoutConcurrentAbort() throws Exception {
        String url="jdbc:tracked:mem:"+UUID.randomUUID();TrackingDriver.Track track=new TrackingDriver.Track();TrackingDriver.tracks.put(url,track);
        try(Fixture f=new Fixture(TrackingDriver.class.getName())){
            String id=await(f.service.open(f.params("idle").put("jdbcUrl",url).put("driverClass",TrackingDriver.class.getName()))).path("connectionId").asText();
            assertTrue(await(f.service.submit(id,s->object().put("readComplete",true))).path("readComplete").asBoolean());
            assertTrue(await(f.service.disconnect(object().put("connectionId",id))).path("closed").asBoolean());
            assertEquals(1,track.closeCount.get());assertEquals(1,track.aborted.getCount());assertEquals(1,track.owners.size());
        }finally{TrackingDriver.tracks.remove(url);}
    }
    @Test void unconfirmedCloseIsAnErrorAndKeepsLoaderLeaseUntilHelperRestart() throws Exception {
        String url="jdbc:tracked:mem:"+UUID.randomUUID();TrackingDriver.Track track=new TrackingDriver.Track();track.failClose=true;TrackingDriver.tracks.put(url,track);
        try(Fixture f=new Fixture(TrackingDriver.class.getName())){
            String id=await(f.service.open(f.params("unclosed").put("jdbcUrl",url).put("driverClass",TrackingDriver.class.getName()))).path("connectionId").asText();RpcException failure=error(f.service.disconnect(object().put("connectionId",id)));assertEquals(SqlErrors.SQL_FAILURE,failure.code);
            assertEquals(SqlErrors.NEEDS_RESTART,assertThrows(RpcException.class,()->f.service.open(f.params("new"))).code);assertFalse(failure.data.path("closed").asBoolean());
        }finally{TrackingDriver.tracks.remove(url);}
    }
    @Test void confirmedCloseStillReportsDriverFailureAndAllowsAnExplicitNewConnection() throws Exception {
        String url="jdbc:tracked:mem:"+UUID.randomUUID();TrackingDriver.Track track=new TrackingDriver.Track();track.warnAfterClose=true;TrackingDriver.tracks.put(url,track);
        try(Fixture f=new Fixture(TrackingDriver.class.getName())){
            String id=await(f.service.open(f.params("warning").put("jdbcUrl",url).put("driverClass",TrackingDriver.class.getName()))).path("connectionId").asText();RpcException failure=error(f.service.disconnect(object().put("connectionId",id)));assertTrue(failure.data.path("closed").asBoolean());assertEquals("08006",failure.data.path("sqlError").path("sqlState").asText());
            assertFalse(await(f.service.open(f.params("warning"))).path("connectionId").asText().isBlank());
        }finally{TrackingDriver.tracks.remove(url);}
    }
}
