package kode.jdbc.queries;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.connections.ConnectionService;
import kode.jdbc.drivers.LoaderCatalog;
import kode.jdbc.drivers.TransferControl;
import kode.jdbc.protocol.FrameCodec;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import java.io.StringReader;
import java.lang.reflect.*;
import java.sql.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.*;
import static org.junit.jupiter.api.Assertions.*;

@Timeout(15)
public class QueryServiceTest {
    static ObjectNode object(){return FrameCodec.JSON.createObjectNode();}
    static JsonNode await(CompletionStage<JsonNode> stage) throws Exception{return stage.toCompletableFuture().get(5,TimeUnit.SECONDS);}
    static final class Capture implements QueryService.Events {
        final List<ObjectNode> values=Collections.synchronizedList(new ArrayList<>());
        public boolean send(ObjectNode value,java.util.function.BooleanSupplier cancelled){if(cancelled.getAsBoolean())return false;values.add(value.deepCopy());return true;}
        List<JsonNode> events(String type){return values.stream().map(value->(JsonNode)value.path("event")).filter(value->value.path("type").asText().equals(type)).toList();}
        List<JsonNode> rows(){return events("rows").stream().flatMap(value->{List<JsonNode> rows=new ArrayList<>();value.path("rows").forEach(rows::add);return rows.stream();}).toList();}
        void finalOrder(JsonNode result){assertEquals(values.size(),result.path("eventCount").asInt());for(int i=0;i<values.size();i++)assertEquals(i,values.get(i).path("sequence").asInt());}
    }
    static final class Fixture implements AutoCloseable {
        final LoaderCatalog loaders=new LoaderCatalog();final ConnectionService connections=new ConnectionService(loaders);final QueryService queries=new QueryService(connections);final String connection;
        Fixture() throws Exception{this("org.h2.Driver","jdbc:h2:mem:"+UUID.randomUUID(),false);}
        Fixture(String driver,String url,boolean manual) throws Exception {
            String loader=loaders.inspect("query-fixture",List.of(),driver,new TransferControl("fixture",()->false,null)).path("loaderId").asText();ObjectNode p=object().put("dataSourceId","fixture").put("loaderId",loader).put("driverClass",driver).put("jdbcUrl",url).put("autoCommit",!manual);p.putObject("properties").put("user","sa").put("password","synthetic-query-password");connection=await(connections.open(p)).path("connectionId").asText();
        }
        ObjectNode params(String id,String sql){return object().put("connectionId",connection).put("executionId",id).put("sql",sql);}
        JsonNode execute(String sql,Capture events) throws Exception{return await(queries.execute(params(UUID.randomUUID().toString(),sql),events));}
        @Override public void close(){queries.close();connections.close();loaders.close();}
    }
    @Test void h2SelectUpdateNullEmptyBinaryDecimalTemporalBooleanAndSqlErrorsRemainTyped() throws Exception {
        try(Fixture f=new Fixture()){
            Capture create=new Capture();assertEquals("completed",f.execute("create table item(id int, name varchar(20))",create).path("outcome").asText());assertEquals(0,create.events("updateCount").getFirst().path("count").asInt());
            Capture insert=new Capture();f.execute("insert into item values(1, 'Alice'), (2, null)",insert);assertEquals(2,insert.events("updateCount").getFirst().path("count").asInt());
            Capture select=new Capture();JsonNode result=f.execute("select name, X'00FF', cast(12345678901234567890.000000000000000001 as decimal(38,18)), timestamp '2024-01-02 03:04:05.123456789', true, '' from item order by id",select);
            assertEquals("completed",result.path("outcome").asText());select.finalOrder(result);assertEquals(2,select.rows().size());JsonNode row=select.rows().getFirst();assertEquals("Alice",row.get(0).path("text").asText());assertEquals("binary",row.get(1).path("kind").asText());assertEquals("AP8=",row.get(1).path("base64").asText());assertEquals("12345678901234567890.000000000000000001",row.get(2).path("text").asText());assertEquals("2024-01-02 03:04:05.123456789",row.get(3).path("text").asText());assertEquals("true",row.get(4).path("text").asText());assertEquals("",row.get(5).path("text").asText());assertEquals("null",select.rows().get(1).get(0).path("kind").asText());
            assertEquals(6,select.events("resultSetStart").getFirst().path("columns").size());assertFalse(select.events("resultSetEnd").getFirst().path("limited").asBoolean());
            Capture error=new Capture();JsonNode failed=f.execute("select * from missing_table",error);assertEquals("failed",failed.path("outcome").asText());assertFalse(failed.path("error").path("sqlState").asText().isBlank());assertFalse(failed.toString().contains("synthetic-query-password"));error.finalOrder(failed);
        }
    }
    @Test void h2BatchesRowCapsLobPreviewsAndWideRowsRespectBoundsAndFreeLargeValues() throws Exception {
        try(Fixture f=new Fixture()){
            Capture rows=new Capture();JsonNode finalResult=await(f.queries.execute(f.params("capped","select x from system_range(1, 1000)").put("maxRows",450),rows));assertEquals(450,rows.rows().size());assertEquals(List.of(200,200,50),rows.events("rows").stream().map(value->value.path("rows").size()).toList());assertTrue(rows.events("resultSetEnd").getFirst().path("limited").asBoolean());rows.finalOrder(finalResult);
            Capture exact=new Capture();await(f.queries.execute(f.params("exact","select x from system_range(1, 450)").put("maxRows",450),exact));assertFalse(exact.events("resultSetEnd").getFirst().path("limited").asBoolean());
            Capture lob=new Capture();f.execute("select cast(repeat('😀', 40000) as clob), cast(X'00FF' as blob), ARRAY[1,2,3]",lob);JsonNode row=lob.rows().getFirst();assertTrue(row.get(0).path("truncated").asBoolean());assertEquals(65536,row.get(0).path("text").asText().getBytes(java.nio.charset.StandardCharsets.UTF_8).length);assertEquals("AP8=",row.get(1).path("base64").asText());assertTrue(row.get(2).path("text").asText().startsWith("[1, 2, 3]"));
            Capture wide=new Capture();String sql="select "+String.join(",",Collections.nCopies(32,"repeat('a', 70000)"));f.execute(sql,wide);assertEquals(32,wide.rows().getFirst().size());for(JsonNode cell:wide.rows().getFirst())assertTrue(cell.path("truncated").asBoolean());for(ObjectNode event:wide.values)assertTrue(FrameCodec.JSON.writeValueAsBytes(event).length<FrameCodec.MAX_FRAME_BYTES);
        }
    }
    @Test void unicodePreviewNeverSplitsScalarsAndDistinguishesExactLengthFromTruncation() throws Exception {
        assertEquals(new CellPreview.Text("é😀",false),CellPreview.text(new StringReader("é😀"),6));assertEquals(new CellPreview.Text("é",true),CellPreview.text(new StringReader("é😀"),5));assertEquals(new CellPreview.Text("�x�",false),CellPreview.text(new StringReader("\ud800x\udc00"),20));assertEquals(new CellPreview.Text("",false),CellPreview.text(new StringReader(""),0));
    }
    static final class Track {
        final String mode;final CountDownLatch entered=new CountDownLatch(1),firstCancel=new CountDownLatch(1),secondCancel=new CountDownLatch(1),releaseExecute=new CountDownLatch(1),releaseCancel=new CountDownLatch(1),cancelEntered=new CountDownLatch(1);
        final AtomicInteger statements=new AtomicInteger(),oldCancels=new AtomicInteger(),newCancels=new AtomicInteger();
        Track(String mode){this.mode=mode;}
    }
    public static final class FixtureDriver implements Driver {
        static final Map<String,Track> tracks=new ConcurrentHashMap<>();
        public Connection connect(String url,Properties props) throws SQLException {
            if(!acceptsURL(url))return null;Track track=tracks.get(url);Connection delegate=new org.h2.Driver().connect(url.replace("jdbc:query-fixture:","jdbc:h2:"),props);
            return (Connection)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{Connection.class},(proxy,method,args)->{
                if(method.getName().equals("createStatement")){Statement jdbc=delegate.createStatement();int ordinal=track.statements.getAndIncrement();AtomicInteger index=new AtomicInteger();
                    return Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{Statement.class},(statement,operation,arguments)->{
                        String name=operation.getName();
                        if(name.equals("cancel")){if(ordinal==0)track.oldCancels.incrementAndGet();else track.newCancels.incrementAndGet();track.cancelEntered.countDown();
                            if(track.mode.equals("unsupported"))throw new SQLFeatureNotSupportedException("Synthetic unsupported cancellation");
                            if(track.mode.equals("hanging")){while(track.releaseCancel.getCount()>0)try{track.releaseCancel.await();}catch(InterruptedException ignored){}return null;}
                            if(track.mode.equals("entry-race")){if(track.firstCancel.getCount()>0)track.firstCancel.countDown();else track.secondCancel.countDown();return null;}
                        }
                        if(name.equals("execute")){track.entered.countDown();
                            if(ordinal==0&&track.mode.equals("entry-race")){track.firstCancel.await();track.secondCancel.await();throw new SQLException("Synthetic cancelled after execute entered","57014");}
                            if(ordinal==0&&(track.mode.equals("hanging")||track.mode.equals("unsupported")))track.releaseExecute.await();
                            if(track.mode.startsWith("mixed")){index.set(0);return true;}
                        }
                        if(track.mode.startsWith("mixed")){
                            if(name.equals("getResultSet")){if(track.mode.equals("mixed-error")&&index.get()==2)throw new SQLException("Synthetic later result failure","42000",321);return jdbc.executeQuery(index.get()==0?"select 1 as first_value":"select 2 as second_value");}
                            if(name.equals("getLargeUpdateCount"))return index.get()==1?Long.MAX_VALUE:-1L;
                            if(name.equals("getMoreResults"))return index.incrementAndGet()==2;
                            if(name.equals("getWarnings")){SQLWarning first=new SQLWarning("Synthetic password=hidden warning","01000",7);first.setNextWarning(new SQLWarning("Second warning","01001",8));return first;}
                        }
                        try{return operation.invoke(jdbc,arguments);}catch(InvocationTargetException failure){throw failure.getCause();}
                    });
                }
                try{return method.invoke(delegate,args);}catch(InvocationTargetException failure){throw failure.getCause();}
            });
        }
        public boolean acceptsURL(String url){return url.startsWith("jdbc:query-fixture:");}public DriverPropertyInfo[] getPropertyInfo(String url,Properties p){return new DriverPropertyInfo[0];}public int getMajorVersion(){return 1;}public int getMinorVersion(){return 0;}public boolean jdbcCompliant(){return false;}public java.util.logging.Logger getParentLogger(){return java.util.logging.Logger.getGlobal();}
    }
    static final class Script implements AutoCloseable {
        final String url="jdbc:query-fixture:mem:"+UUID.randomUUID();final Track track;final Fixture fixture;
        Script(String mode) throws Exception{track=new Track(mode);FixtureDriver.tracks.put(url,track);fixture=new Fixture(FixtureDriver.class.getName(),url,true);}
        @Override public void close(){track.releaseExecute.countDown();track.releaseCancel.countDown();track.firstCancel.countDown();track.secondCancel.countDown();fixture.close();FixtureDriver.tracks.remove(url);}
    }
    @Test void mixedResultSetsLargeUpdateCountsAndChainedWarningsStayInOrder() throws Exception {
        try(Script script=new Script("mixed")){Capture events=new Capture();JsonNode result=script.fixture.execute("synthetic mixed statement",events);assertEquals("completed",result.path("outcome").asText());assertEquals(2,events.events("resultSetStart").size());assertEquals("1",events.rows().getFirst().get(0).path("text").asText());assertEquals("2",events.rows().get(1).get(0).path("text").asText());assertEquals(Long.MAX_VALUE,events.events("updateCount").getFirst().path("count").asLong());JsonNode warning=events.events("warning").getFirst().path("error");assertFalse(warning.path("message").asText().contains("hidden"));assertEquals(8,warning.path("chained").get(0).path("vendorCode").asInt());events.finalOrder(result);}
    }
    @Test void aLaterResultFailureKeepsEarlierRowsAndUpdatesAndMarksManualTransactionUnusable() throws Exception {
        try(Script script=new Script("mixed-error")){Capture events=new Capture();JsonNode result=script.fixture.execute("synthetic mixed failure",events);assertEquals("failed",result.path("outcome").asText());assertEquals(1,events.rows().size());assertEquals(Long.MAX_VALUE,events.events("updateCount").getFirst().path("count").asLong());assertEquals("42000",result.path("error").path("sqlState").asText());assertEquals(321,result.path("error").path("vendorCode").asInt());assertFalse(result.path("transactionUsable").asBoolean());assertTrue(result.path("previewIncomplete").asBoolean());events.finalOrder(result);}
    }
    @Test void cancellationRacingExecuteEntryRetriesOnlyItsOwnStatementAndCannotReachTheNext() throws Exception {
        try(Script script=new Script("entry-race")){Fixture f=script.fixture;Capture events=new Capture();CompletionStage<JsonNode> original=f.queries.execute(f.params("entry","select 1"),events);assertTrue(script.track.entered.await(2,TimeUnit.SECONDS));assertEquals("requested",await(f.queries.cancel("entry")).path("status").asText());JsonNode result=await(original);assertEquals("cancelled",result.path("outcome").asText());assertTrue(result.path("previewIncomplete").asBoolean());assertTrue(script.track.oldCancels.get()>=2);events.finalOrder(result);assertEquals("alreadyFinished",await(f.queries.cancel("entry")).path("status").asText());assertEquals("completed",f.execute("select 2",new Capture()).path("outcome").asText());assertEquals(0,script.track.newCancels.get());}
    }
    @Test void unsupportedCancellationIsVisibleAndAStillRunningOperationKeepsItsOriginalFinal() throws Exception {
        try(Script script=new Script("unsupported")){Fixture f=script.fixture;CompletionStage<JsonNode> original=f.queries.execute(f.params("unsupported","select 1"),new Capture());assertTrue(script.track.entered.await(2,TimeUnit.SECONDS));assertEquals("unsupported",await(f.queries.cancel("unsupported")).path("status").asText());assertFalse(original.toCompletableFuture().isDone());script.track.releaseExecute.countDown();assertEquals("cancelled",await(original).path("outcome").asText());}
    }
    @Test void hangingCancellationDoesNotHoldControlAndPreventsAnOldCancelFromTargetingANewQuery() throws Exception {
        try(Script script=new Script("hanging")){Fixture f=script.fixture;CompletionStage<JsonNode> original=f.queries.execute(f.params("hanging","select 1"),new Capture());assertTrue(script.track.entered.await(2,TimeUnit.SECONDS));long start=System.nanoTime();assertEquals("requested",await(f.queries.cancel("hanging")).path("status").asText());assertTrue(System.nanoTime()-start<TimeUnit.SECONDS.toNanos(1));assertTrue(script.track.cancelEntered.await(1,TimeUnit.SECONDS));script.track.releaseExecute.countDown();CompletionStage<JsonNode> next=f.queries.execute(f.params("next","select 2"),new Capture());assertFalse(original.toCompletableFuture().isDone());assertFalse(next.toCompletableFuture().isDone());script.track.releaseCancel.countDown();await(original);assertEquals("completed",await(next).path("outcome").asText());assertEquals(0,script.track.newCancels.get());}
    }
    @Test void aRealLongH2QueryIsCancelledOnAnIndependentWorker() throws Exception {
        try(Script script=new Script("h2-long")){Fixture f=script.fixture;Capture events=new Capture();CompletionStage<JsonNode> query=f.queries.execute(f.params("long","select count(*) from system_range(1, 1000000000) a, system_range(1, 1000000000) b"),events);assertTrue(script.track.entered.await(2,TimeUnit.SECONDS));assertEquals("requested",await(f.queries.cancel("long")).path("status").asText());JsonNode result=await(query);assertEquals("cancelled",result.path("outcome").asText());assertTrue(script.track.oldCancels.get()>0);events.finalOrder(result);}
    }
    @Test void fullOutputWaitChecksCancellationAndKeepsFinalSequenceAuthoritative() throws Exception {
        try(Fixture f=new Fixture()){CountDownLatch blocked=new CountDownLatch(1);Capture capture=new Capture();QueryService.Events events=(value,cancelled)->{if(value.path("event").path("type").asText().equals("rows")){blocked.countDown();while(!cancelled.getAsBoolean())Thread.sleep(2);return false;}return capture.send(value,cancelled);};CompletionStage<JsonNode> query=f.queries.execute(f.params("backpressure","select x from system_range(1, 100000)"),events);assertTrue(blocked.await(2,TimeUnit.SECONDS));assertEquals("requested",await(f.queries.cancel("backpressure")).path("status").asText());JsonNode result=await(query);assertEquals("cancelled",result.path("outcome").asText());assertTrue(result.path("previewIncomplete").asBoolean());capture.finalOrder(result);}
    }
    @Test void priorityCancelBeforeDispatchCreatesNoStatementAndDuplicateExecutionIdsAreRejected() throws Exception {
        try(Script script=new Script("h2-long")){Fixture f=script.fixture;assertEquals("requested",await(f.queries.cancel("early")).path("status").asText());Capture events=new Capture();JsonNode result=await(f.queries.execute(f.params("early","select 1"),events));assertEquals("cancelled",result.path("outcome").asText());assertEquals(0,script.track.statements.get());events.finalOrder(result);assertThrows(kode.jdbc.protocol.RpcException.class,()->f.queries.execute(f.params("early","select 2"),new Capture()));}
    }
}
