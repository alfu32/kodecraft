package kode.jdbc.queries;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.connections.ConnectionService;
import kode.jdbc.connections.SqlErrors;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import kode.jdbc.protocol.RpcServer;
import java.sql.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.BooleanSupplier;

/** Bounded streaming on each connection owner; independent, execution-specific
 * JDBC cancellation. No control thread waits in Statement.cancel(). */
public final class QueryService implements AutoCloseable {
    @FunctionalInterface public interface Events {boolean send(ObjectNode params,BooleanSupplier cancelled) throws Exception;}
    private final ConnectionService connections;
    private final Map<String,Job> active=new HashMap<>();
    private final Set<String> cancelledBeforeStart=new HashSet<>();
    private final LinkedHashSet<String> finished=new LinkedHashSet<>();
    private final AtomicBoolean stopping=new AtomicBoolean();
    private final ThreadPoolExecutor cancels=new ThreadPoolExecutor(4,4,0,TimeUnit.MILLISECONDS,new ArrayBlockingQueue<>(64),Thread.ofPlatform().daemon(true).name("jdbc-statement-cancel-",0).factory(),new ThreadPoolExecutor.AbortPolicy());
    private final ScheduledThreadPoolExecutor deadlines=new ScheduledThreadPoolExecutor(1,Thread.ofPlatform().daemon(true).name("jdbc-cancel-ack").factory());
    public QueryService(ConnectionService connections){this.connections=connections;deadlines.setRemoveOnCancelPolicy(true);}
    public void register(RpcServer server){server.register("query.execute",false,(p,c)->execute(p,(event,cancel)->c.notify("query.event",event,cancel)));server.register("query.cancel",true,(p,c)->cancel(ConnectionService.id(p,"executionId")));server.onClose(this);}
    private static ObjectNode object(){return FrameCodec.JSON.createObjectNode();}
    private static RpcException rejected(int code,String text){return new RpcException(code,text,object().put("notStarted",true));}
    private static int number(ObjectNode p,String key,int fallback,int min,int max) throws RpcException {JsonNode value=p.get(key);if(value==null)return fallback;if(!value.isIntegralNumber()||!value.canConvertToInt()||value.intValue()<min||value.intValue()>max)throw rejected(-32602,"Invalid SQL preview limit");return value.intValue();}
    public synchronized CompletionStage<JsonNode> execute(ObjectNode params,Events events) throws Exception {
        if(stopping.get())throw rejected(-32021,"Query service is stopping");String id=ConnectionService.id(params,"executionId"),connection=ConnectionService.id(params,"connectionId");JsonNode sql=params.get("sql");
        if(sql==null||!sql.isTextual()||sql.textValue().isBlank()||sql.textValue().getBytes(java.nio.charset.StandardCharsets.UTF_8).length>4*1024*1024)throw rejected(-32602,"SQL must contain 1 byte to 4 MiB of text");
        if(active.containsKey(id)||finished.contains(id))throw rejected(-32602,"Execution ID was already used");if(active.size()>=64)throw rejected(-32003,"SQL execution capacity exhausted");
        Job job=new Job(id,sql.textValue(),number(params,"maxRows",500,1,100_000),number(params,"fetchSize",200,0,10_000),events,params);
        job.requested=cancelledBeforeStart.remove(id);active.put(id,job);
        try{return connections.submit(connection,job::run).whenComplete((value,error)->retire(job));}
        catch(Throwable failure){retire(job);throw failure;}
    }
    private synchronized void retire(Job job){active.remove(job.id,job);finished.add(job.id);if(finished.size()>4096)finished.remove(finished.iterator().next());job.markFinished();}
    public synchronized CompletionStage<JsonNode> cancel(String id){
        Job job=active.get(id);if(job!=null)return job.cancel();if(finished.contains(id))return RpcServer.result(object().put("status","alreadyFinished"));
        if(cancelledBeforeStart.size()>=4096)return CompletableFuture.failedFuture(rejected(-32003,"Early SQL cancellation capacity exhausted"));
        cancelledBeforeStart.add(id);return RpcServer.result(object().put("status","requested"));
    }
    private static final class Cancelled extends Exception {}
    private final class Job {
        final String id,sql;final int maxRows,fetchSize;final Events events;final ObjectNode context;
        boolean requested,finishing,done,cancelUnsupported;Statement statement;ClassLoader loader;
        CompletableFuture<Void> cancelling;CompletableFuture<JsonNode> acknowledgement;
        long sequence;boolean incomplete;ConnectionService.Session session;
        Job(String id,String sql,int rows,int fetch,Events events,ObjectNode params){this.id=id;this.sql=sql;maxRows=rows;fetchSize=fetch;this.events=events;context=object();for(String key:List.of("catalog","schema"))if(params.has(key))context.set(key,params.get(key));}
        synchronized void markFinished(){done=true;statement=null;}
        synchronized boolean cancelled(){return requested||stopping.get()||session!=null&&session.closing();}
        void check() throws Cancelled {if(cancelled())throw new Cancelled();}
        synchronized CompletionStage<JsonNode> cancel(){
            if(done||finishing)return RpcServer.result(object().put("status","alreadyFinished"));requested=true;
            if(cancelUnsupported)return RpcServer.result(object().put("status","unsupported"));
            if(statement==null)return RpcServer.result(object().put("status","requested"));
            if(acknowledgement!=null)return acknowledgement;
            Statement target=statement;ClassLoader targetLoader=loader;cancelling=new CompletableFuture<>();acknowledgement=new CompletableFuture<>();
            try{
                ScheduledFuture<?> deadline=deadlines.schedule(()->acknowledgement.complete(object().put("status","requested")),50,TimeUnit.MILLISECONDS);
                acknowledgement.whenComplete((value,error)->deadline.cancel(false));
                cancels.execute(()->{
                    Thread thread=Thread.currentThread();ClassLoader previous=thread.getContextClassLoader();thread.setContextClassLoader(targetLoader);
                    try{
                        // cancel() can race just before execute() enters a
                        // driver that resets its cancellation flag on entry.
                        // Retry only while this same execution owns the JDBC
                        // connection; never let a retry reach the next query.
                        while(true){
                            synchronized(this){if(finishing||done)break;}
                            target.cancel();acknowledgement.complete(object().put("status","requested"));
                            synchronized(this){if(finishing||done)break;wait(25);}
                        }
                        acknowledgement.complete(object().put("status","requested"));
                    }
                    catch(Throwable failure){synchronized(this){cancelUnsupported=true;}acknowledgement.complete(object().put("status","unsupported"));}
                    finally{thread.setContextClassLoader(previous);cancelling.complete(null);}
                });
            }catch(RejectedExecutionException failure){cancelUnsupported=true;acknowledgement.complete(object().put("status","unsupported"));cancelling.complete(null);}
            return acknowledgement;
        }
        void emit(ObjectNode event) throws Exception {
            ObjectNode notification=object().put("executionId",id).put("sequence",sequence);notification.set("event",event);
            if(!events.send(notification,this::cancelled)){incomplete=true;throw new Cancelled();}sequence++;
        }
        JsonNode run(ConnectionService.Session owner) throws Exception {
            session=owner;loader=Thread.currentThread().getContextClassLoader();long started=System.nanoTime();String outcome="completed";Throwable failure=null;
            try{
                emit(object().put("type","started"));check();if(!context.isEmpty())owner.setContext(context);
                Statement created=owner.connection().createStatement();synchronized(this){statement=created;}
                owner.cancellation(()->cancel());check();
                try{created.setMaxRows(maxRows+1);}catch(SQLFeatureNotSupportedException unsupported){warning(unsupported);}
                try{created.setFetchSize(fetchSize);}catch(SQLFeatureNotSupportedException unsupported){warning(unsupported);}
                check();boolean rows=created.execute(sql);int results=0;
                while(true){
                    if(++results>1024)throw CellPreview.limit("SQL result count exceeds 1,024");
                    if(rows){try(ResultSet result=created.getResultSet()){if(result==null)throw new SQLException("Driver returned no result set");result(result,results);}}
                    else{long count;try{count=created.getLargeUpdateCount();}catch(SQLFeatureNotSupportedException|UnsupportedOperationException|AbstractMethodError unsupported){count=created.getUpdateCount();}if(count==-1)break;emit(object().put("type","updateCount").put("count",count));}
                    check();try{rows=created.getMoreResults(Statement.CLOSE_CURRENT_RESULT);}catch(SQLFeatureNotSupportedException|AbstractMethodError unsupported){rows=created.getMoreResults();}
                }
                SQLWarning warnings=created.getWarnings();if(warnings!=null)warning(warnings);
            }catch(Cancelled cancelled){outcome="cancelled";incomplete=true;}
            catch(Throwable error){failure=error;owner.failed(error);outcome=cancelled()&&isCancellation(error)?"cancelled":"failed";incomplete=true;}
            finally{
                CompletableFuture<Void> pending;Statement owned;synchronized(this){finishing=true;notifyAll();pending=cancelling;owned=statement;}
                // An old driver's cancel can affect connection-wide state. Do
                // not run the next query or release its loader until it returns.
                if(pending!=null)pending.join();
                if(owned!=null)try{owned.close();}catch(Throwable close){if(failure==null)failure=close;owner.failed(close);outcome="failed";incomplete=true;}
                synchronized(this){statement=null;}
                owner.cancellation(null);
            }
            ObjectNode result=object().put("executionId",id).put("outcome",outcome).put("elapsedMs",TimeUnit.NANOSECONDS.toMillis(System.nanoTime()-started)).put("transactionUsable",owner.transactionUsable()).put("previewIncomplete",incomplete).put("eventCount",sequence);
            if(failure!=null)result.set("error",SqlErrors.wire(failure,owner.redactor));result.set("connectionState",owner.currentState());return result;
        }
        private void warning(SQLException warning) throws Exception {ObjectNode event=object().put("type","warning");event.set("error",SqlErrors.wire(warning,session.redactor));emit(event);}
        private void result(ResultSet rows,int index) throws Exception {
            ResultSetMetaData metadata=rows.getMetaData();int count=metadata.getColumnCount();if(count<1||count>CellPreview.MAX_COLUMNS)throw CellPreview.limit("SQL preview supports 1 to 256 columns");
            String resultId=id+"-"+index;if(resultId.length()>128)resultId=UUID.randomUUID().toString();ObjectNode start=object().put("type","resultSetStart").put("resultSetId",resultId);ArrayNode columns=start.putArray("columns");int[] types=new int[count];
            for(int i=0;i<count;i++){int column=i+1;String label=metadata.getColumnLabel(column);if(label==null||label.isBlank())label=metadata.getColumnName(column);types[i]=metadata.getColumnType(column);columns.addObject().put("label",CellPreview.metadata(label)).put("jdbcType",types[i]).put("typeName",CellPreview.metadata(metadata.getColumnTypeName(column))).put("nullable",metadata.isNullable(column)!=ResultSetMetaData.columnNoNulls);}
            emit(start);ArrayNode batch=FrameCodec.JSON.createArrayNode();int bytes=0,seen=0;boolean limited=false;int cellLimit=Math.min(CellPreview.MAX_CELL_BYTES,CellPreview.MAX_ROW_BYTES/count);
            while(true){
                check();if(!rows.next())break;if(seen++>=maxRows){limited=true;break;}ArrayNode row=FrameCodec.JSON.createArrayNode();
                for(int column=0;column<count;column++){check();row.add(CellPreview.cell(rows,column+1,types[column],cellLimit));}
                int size=FrameCodec.JSON.writeValueAsBytes(row).length;
                if(!batch.isEmpty()&&(batch.size()>=200||bytes+size>2*1024*1024)){flush(resultId,batch);batch=FrameCodec.JSON.createArrayNode();bytes=0;}
                batch.add(row);bytes+=size;
            }
            if(!batch.isEmpty())flush(resultId,batch);emit(object().put("type","resultSetEnd").put("resultSetId",resultId).put("limited",limited).put("incomplete",false));
        }
        private void flush(String resultId,ArrayNode batch) throws Exception {ObjectNode event=object().put("type","rows").put("resultSetId",resultId);event.set("rows",batch);emit(event);}
    }
    private static boolean isCancellation(Throwable error){SQLException sql=SqlErrors.find(error);return sql!=null&&(sql.getErrorCode()==1013||Set.of("57014","HY008","70100").contains(sql.getSQLState()==null?"":sql.getSQLState()));}
    @Override public synchronized void close(){if(!stopping.compareAndSet(false,true))return;for(Job job:active.values())job.cancel();cancels.shutdown();deadlines.shutdown();}
}
