package kode.jdbc.metadata;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.connections.ConnectionService;
import kode.jdbc.drivers.LoaderCatalog;
import kode.jdbc.drivers.TransferControl;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import java.lang.reflect.*;
import java.sql.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.*;
import javax.sql.rowset.*;
import static org.junit.jupiter.api.Assertions.*;

@Timeout(15)
class MetadataServiceTest {
    private static final ThreadPoolExecutor CANCEL_EXECUTOR=new ThreadPoolExecutor(1,1,0,TimeUnit.MILLISECONDS,new ArrayBlockingQueue<>(8),Thread.ofPlatform().daemon(true).name("metadata-test-cancel").factory(),new ThreadPoolExecutor.AbortPolicy());
    static MetadataService.Request request(String kind,String catalog,String schema,String category,String name,String type){return new MetadataService.Request(kind,catalog,schema,category,name,type,null);}
    static JsonNode load(DatabaseMetaData meta,MetadataService.Request request) throws Exception {return new MetadataService.Load(meta,request,()->false).run();}
    static List<JsonNode> nodes(JsonNode result,String type){List<JsonNode> out=new ArrayList<>();for(JsonNode node:result.path("objects"))if(node.path("objectType").asText().equals(type))out.add(node);return out;}
    static JsonNode named(JsonNode result,String type,String name){return nodes(result,type).stream().filter(n->n.path("name").asText().equals(name)).findFirst().orElseThrow();}
    static Connection h2() throws Exception {return new org.h2.Driver().connect("jdbc:h2:mem:"+UUID.randomUUID(),new Properties());}
    @Test void h2TreeTypesColumnsPrimaryForeignKeysAndIndexesRetainAttributesAndLeaves() throws Exception {
        try(Connection c=h2();Statement s=c.createStatement()){
            s.execute("create schema example");s.execute("create table example.parent(id int primary key)");s.execute("create table example.child(id int primary key, parent_id int, constraint child_parent foreign key(parent_id) references example.parent(id))");s.execute("create index child_parent_idx on example.child(parent_id)");s.execute("create view example.child_view as select id from example.child");
            DatabaseMetaData m=c.getMetaData();JsonNode root=load(m,request("ROOT",null,null,null,null,null));assertFalse(nodes(root,"CATALOG").isEmpty());String cat=root.path("objects").get(0).path("name").asText();
            JsonNode schemas=load(m,request("CATALOG",cat,null,null,null,null));assertTrue(named(schemas,"SCHEMA","EXAMPLE").path("hasChildren").asBoolean());
            JsonNode categories=load(m,request("SCHEMA",cat,"EXAMPLE",null,null,null));assertEquals("TABLE",named(categories,"CATEGORY","Tables").path("attributes").path("categoryType").asText());
            JsonNode tables=load(m,request("CATEGORY",cat,"EXAMPLE","TABLE",null,null));assertEquals("EXAMPLE.CHILD",named(tables,"TABLE","CHILD").path("qualifiedName").asText());assertEquals(2,nodes(tables,"TABLE").size());
            assertEquals("CHILD_VIEW",nodes(load(m,request("CATEGORY",cat,"EXAMPLE","VIEW",null,null)),"VIEW").getFirst().path("name").asText());
            JsonNode children=load(m,request("OBJECT",cat,"EXAMPLE",null,"CHILD","TABLE"));assertEquals(2,nodes(children,"COLUMN").size());assertEquals("1",named(children,"COLUMN","ID").path("attributes").path("ordinal").asText());assertFalse(named(children,"COLUMN","ID").path("hasChildren").asBoolean());assertFalse(nodes(children,"PRIMARY_KEY").isEmpty());assertFalse(nodes(children,"INDEX").isEmpty());assertEquals("PARENT",named(children,"FOREIGN_KEY","CHILD_PARENT").path("attributes").path("referencedTable").asText());
            assertFalse(nodes(load(m,request("CATEGORY",cat,"EXAMPLE","TYPE",null,null)),"TYPE").isEmpty());assertTrue(root.path("capabilities").path("transactions").asBoolean());
            JsonNode packages=load(m,request("CATEGORY",cat,"EXAMPLE","PACKAGE",null,null));assertTrue(packages.path("objects").isEmpty());assertTrue(packages.path("message").asText().contains("package discovery"));
        }
    }
    @Test void literalWildcardNamesDoNotMergeNeighboringSchemasOrTables() throws Exception {
        try(Connection c=h2();Statement s=c.createStatement()){
            s.execute("create schema \"A_B\"");s.execute("create schema \"AXB\"");s.execute("create table \"A_B\".\"x_y\"(exact int)");s.execute("create table \"A_B\".\"xzy\"(wrong int)");s.execute("create table \"AXB\".\"x_y\"(wrong int)");
            JsonNode tables=load(c.getMetaData(),request("CATEGORY",null,"A_B","TABLE",null,null));assertEquals(2,nodes(tables,"TABLE").size());
            JsonNode columns=load(c.getMetaData(),request("OBJECT",null,"A_B",null,"x_y","TABLE"));assertEquals(1,nodes(columns,"COLUMN").size());assertEquals("EXACT",nodes(columns,"COLUMN").getFirst().path("name").asText());
        }
    }
    @Test void listResultSetStatementIsCancelledBeforeTheOwnerReleasesIt() throws Exception {
        try(Connection c=h2()){
            CountDownLatch entered=new CountDownLatch(1),cancelled=new CountDownLatch(1),closed=new CountDownLatch(1);
            Statement statement=(Statement)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{Statement.class},(p,m,a)->{if(m.getName().equals("cancel")){cancelled.countDown();return null;}return m.getReturnType().isPrimitive()?false:null;});
            ResultSet result=(ResultSet)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{ResultSet.class},(p,m,a)->switch(m.getName()){
                case "getStatement"->statement;
                case "next"->{entered.countDown();if(!cancelled.await(3,TimeUnit.SECONDS))throw new SQLException("cancel was not delivered");throw new SQLException("Vendor cancellation error","72000");}
                case "close"->{closed.countDown();yield null;}
                default->m.getReturnType().isPrimitive()?false:null;
            });
            DatabaseMetaData meta=proxy(c.getMetaData(),(method,args)->method.getName().equals("getCatalogs")?result:PASS);
            MetadataService.Job job=new MetadataService.Job(UUID.randomUUID().toString(),CANCEL_EXECUTOR);
            ExecutorService owner=Executors.newSingleThreadExecutor();
            try{
                Future<?> work=owner.submit(()->{try{return new MetadataService.Load(meta,request("ROOT",null,null,null,null,null),()->false,job).run();}catch(SQLException failure){throw new CompletionException(failure);}});
                assertTrue(entered.await(3,TimeUnit.SECONDS));assertTrue(job.cancel());
                ExecutionException failure=assertThrows(ExecutionException.class,()->work.get(4,TimeUnit.SECONDS));
                assertEquals("57014",((SQLException)failure.getCause().getCause()).getSQLState());
                assertTrue(cancelled.await(1,TimeUnit.SECONDS));assertTrue(closed.await(1,TimeUnit.SECONDS));
            }finally{owner.shutdownNow();}
        }
    }
    interface Intercept {Object call(Method method,Object[] args) throws Throwable;}
    static DatabaseMetaData proxy(DatabaseMetaData delegate,Intercept intercept){return (DatabaseMetaData)Proxy.newProxyInstance(MetadataServiceTest.class.getClassLoader(),new Class<?>[]{DatabaseMetaData.class},(p,m,a)->{Object value=intercept.call(m,a);if(value!=PASS)return value;try{return m.invoke(delegate,a);}catch(InvocationTargetException failure){throw failure.getCause();}});}
    static final Object PASS=new Object();
    static CachedRowSet rows(String[] labels,Object[]... values) throws Exception {
        RowSetMetaDataImpl columns=new RowSetMetaDataImpl();columns.setColumnCount(labels.length);for(int i=0;i<labels.length;i++){columns.setColumnName(i+1,labels[i]);columns.setColumnType(i+1,Types.VARCHAR);}
        CachedRowSet result=RowSetProvider.newFactory().createCachedRowSet();result.setMetaData(columns);for(Object[] row:values){result.moveToInsertRow();for(int i=0;i<row.length;i++)result.updateObject(i+1,row[i]);result.insertRow();result.moveToCurrentRow();}result.beforeFirst();return result;
    }
    @Test void representativeCatalogFallbacksPreserveServerNamesAndDoNotInventPackages() throws Exception {
        try(Connection c=h2()){
            AtomicInteger schemaCalls=new AtomicInteger(),tableCalls=new AtomicInteger();
            DatabaseMetaData m=proxy(c.getMetaData(),(method,args)->switch(method.getName()){
                case "getSchemas" -> {schemaCalls.incrementAndGet();yield args!=null&&args[0]!=null?rows(new String[]{"TABLE_SCHEM","TABLE_CATALOG"}):rows(new String[]{"TABLE_SCHEM","TABLE_CATALOG"},new Object[]{"dbo","server-db"});}
                case "getTables" -> {tableCalls.incrementAndGet();yield args[0]!=null?rows(new String[]{"TABLE_NAME","TABLE_SCHEM","TABLE_CAT","TABLE_TYPE","REMARKS"}):rows(new String[]{"TABLE_NAME","TABLE_SCHEM","TABLE_CAT","TABLE_TYPE","REMARKS"},new Object[]{"orders","dbo","server-db","BASE TABLE","fixture"});}
                default -> PASS;
            });
            JsonNode schemas=load(m,request("CATALOG","server-db",null,null,null,null));assertEquals("server-db.dbo",nodes(schemas,"SCHEMA").getFirst().path("qualifiedName").asText());assertEquals(2,schemaCalls.get());
            JsonNode tables=load(m,request("CATEGORY","server-db","dbo","TABLE",null,null));assertEquals("dbo.orders",nodes(tables,"TABLE").getFirst().path("qualifiedName").asText());assertEquals("fixture",nodes(tables,"TABLE").getFirst().path("attributes").path("remarks").asText());assertEquals(2,tableCalls.get());
        }
    }
    @Test void unsupportedMetadataRemainsVisibleWhileSupportedChildrenSurviveAndRealErrorsPropagate() throws Exception {
        try(Connection c=h2();Statement statement=c.createStatement()){
            statement.execute("create table item(id int primary key)");
            DatabaseMetaData m=proxy(c.getMetaData(),(method,args)->{
                if(Set.of("getIndexInfo","getImportedKeys","getFunctions","getFunctionColumns").contains(method.getName()))throw new SQLFeatureNotSupportedException("synthetic private diagnostic");return PASS;
            });
            JsonNode result=load(m,request("OBJECT",null,"PUBLIC",null,"ITEM","TABLE"));assertEquals(1,nodes(result,"COLUMN").size());assertFalse(nodes(result,"PRIMARY_KEY").isEmpty());assertTrue(result.path("message").asText().contains("indexes"));assertTrue(result.path("message").asText().contains("foreign keys"));assertFalse(result.toString().contains("synthetic private diagnostic"));
            JsonNode functions=load(m,request("CATEGORY",null,null,"FUNCTION",null,null));assertFalse(functions.path("capabilities").path("functions").asBoolean());assertTrue(functions.path("message").asText().contains("functions"));
            DatabaseMetaData failed=proxy(m,(method,args)->{if(method.getName().equals("getColumns"))throw new SQLException("broken","08006");return PASS;});assertEquals("08006",assertThrows(SQLException.class,()->load(failed,request("OBJECT",null,"PUBLIC",null,"ITEM","TABLE"))).getSQLState());
        }
    }
    @Test void routineParametersAreLeavesAndDoNotAskForTableKeys() throws Exception {
        try(Connection c=h2()){
            DatabaseMetaData m=proxy(c.getMetaData(),(method,args)->switch(method.getName()){
                case "getFunctions" -> rows(new String[]{"FUNCTION_NAME","FUNCTION_CAT","FUNCTION_SCHEM"},new Object[]{"calculate",null,"public"});
                case "getFunctionColumns" -> rows(new String[]{"COLUMN_NAME","TYPE_NAME","COLUMN_TYPE"},new Object[]{"input","INTEGER","1"});
                case "getPrimaryKeys","getIndexInfo","getImportedKeys" -> throw new AssertionError("Routine incorrectly treated as a table");
                default -> PASS;
            });
            assertEquals("public.calculate",nodes(load(m,request("CATEGORY",null,"public","FUNCTION",null,null)),"FUNCTION").getFirst().path("qualifiedName").asText());
            JsonNode parameter=nodes(load(m,request("OBJECT",null,"public",null,"calculate","FUNCTION")),"COLUMN").getFirst();assertEquals("1",parameter.path("attributes").path("mode").asText());assertFalse(parameter.path("hasChildren").asBoolean());
        }
    }
    @Test void responseBoundsCancellationAndResultSetCleanupAreExplicit() throws Exception {
        try(Connection c=h2()){
            AtomicBoolean closed=new AtomicBoolean();DatabaseMetaData m=proxy(c.getMetaData(),(method,args)->{
                if(!method.getName().equals("getCatalogs"))return PASS;ResultSet rs=rows(new String[]{"TABLE_CAT"},new Object[]{"x".repeat(8193)});
                return Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{ResultSet.class},(p,op,a)->{if(op.getName().equals("close"))closed.set(true);try{return op.invoke(rs,a);}catch(InvocationTargetException failure){throw failure.getCause();}});
            });
            assertEquals("54000",assertThrows(SQLException.class,()->load(m,request("ROOT",null,null,null,null,null))).getSQLState());assertTrue(closed.get());
            assertEquals("57014",assertThrows(SQLException.class,()->new MetadataService.Load(c.getMetaData(),request("ROOT",null,null,null,null,null),()->true).run()).getSQLState());
        }
    }
    @Test void serviceUsesRealConnectionOwnerAndValidatesRequestsBeforeSubmitting() throws Exception {
        try(LoaderCatalog loaders=new LoaderCatalog();ConnectionService connections=new ConnectionService(loaders)){
            MetadataService service=new MetadataService(connections);String loader=loaders.inspect("metadata-fixture",List.of(),"org.h2.Driver",new TransferControl("fixture",()->false,null)).path("loaderId").asText();
            var open=MetadataService.object().put("dataSourceId","metadata").put("loaderId",loader).put("driverClass","org.h2.Driver").put("jdbcUrl","jdbc:h2:mem:"+UUID.randomUUID());open.putObject("properties");String id=connections.open(open).toCompletableFuture().get(3,TimeUnit.SECONDS).path("connectionId").asText();
            var params=MetadataService.object().put("connectionId",id).put("metadataJobId",UUID.randomUUID().toString());params.putObject("request").put("kind","ROOT");JsonNode root=service.list(params).toCompletableFuture().get(3,TimeUnit.SECONDS);assertFalse(root.path("objects").isEmpty());assertTrue(root.has("contextRevision"));
            params.withObject("request").put("kind","INVALID");assertThrows(kode.jdbc.protocol.RpcException.class,()->service.list(params));
        }
    }
    @Test void earlyCancelOnlyCancelsTheMatchingMetadataListBeforeDispatch() throws Exception {
        try(LoaderCatalog loaders=new LoaderCatalog();ConnectionService connections=new ConnectionService(loaders)){
            MetadataService service=new MetadataService(connections);String loader=loaders.inspect("metadata-early-cancel",List.of(),"org.h2.Driver",new TransferControl("fixture",()->false,null)).path("loaderId").asText();
            var open=MetadataService.object().put("dataSourceId","metadata").put("loaderId",loader).put("driverClass","org.h2.Driver").put("jdbcUrl","jdbc:h2:mem:"+UUID.randomUUID());open.putObject("properties");String connection=connections.open(open).toCompletableFuture().get(3,TimeUnit.SECONDS).path("connectionId").asText();
            String cancelledId=UUID.randomUUID().toString();JsonNode cancel=service.cancel(MetadataService.object().put("metadataJobId",cancelledId)).toCompletableFuture().get(3,TimeUnit.SECONDS);assertEquals("requested",cancel.path("status").asText());assertFalse(cancel.path("statementAttached").asBoolean());
            ObjectNode cancelled=MetadataService.object().put("connectionId",connection).put("metadataJobId",cancelledId);cancelled.putObject("request").put("kind","ROOT");
            ExecutionException failure=assertThrows(ExecutionException.class,()->service.list(cancelled).toCompletableFuture().get(3,TimeUnit.SECONDS));
            Throwable cause=failure;while(cause!=null&&!(cause instanceof kode.jdbc.protocol.RpcException))cause=cause.getCause();assertNotNull(cause);var rpc=(kode.jdbc.protocol.RpcException)cause;assertEquals("Metadata request cancelled",rpc.data.path("sqlError").path("message").asText());assertEquals("57014",rpc.data.path("sqlError").path("sqlState").asText());
            ObjectNode neighbor=MetadataService.object().put("connectionId",connection).put("metadataJobId",UUID.randomUUID().toString());neighbor.putObject("request").put("kind","ROOT");
            JsonNode root=service.list(neighbor).toCompletableFuture().get(3,TimeUnit.SECONDS);assertFalse(root.path("objects").isEmpty());
        }
    }
}
