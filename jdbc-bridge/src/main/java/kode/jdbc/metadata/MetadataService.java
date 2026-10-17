package kode.jdbc.metadata;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.connections.ConnectionService;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import kode.jdbc.protocol.RpcServer;
import java.sql.*;
import java.nio.charset.StandardCharsets;
import java.util.*;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicBoolean;
import java.sql.Statement;

/** Stateless JDBC introspection. Every getter and ResultSet belongs to the
 * connection owner; the native client owns epoch-tagged caching. */
public final class MetadataService {
    private final ConnectionService connections;
    private final ConcurrentMap<String,Job> sources=new ConcurrentHashMap<>();
    private final Set<String> cancelledBeforeStart=new LinkedHashSet<>();
    private final LinkedHashSet<String> finished=new LinkedHashSet<>();
    private final ThreadPoolExecutor cancels=new ThreadPoolExecutor(2,2,0,TimeUnit.MILLISECONDS,new ArrayBlockingQueue<>(64),Thread.ofPlatform().daemon(true).name("jdbc-metadata-cancel-",0).factory(),new ThreadPoolExecutor.AbortPolicy());
    public MetadataService(ConnectionService connections){this.connections=connections;}
    public void register(RpcServer server){
        server.register("metadata.list",false,(p,c)->list(p));
        server.register("metadata.source",false,(p,c)->source(p));
        server.register("metadata.cancel",true,(p,c)->cancel(p));
        server.onClose(this::close);
    }
    static final class Job {
        final String id;final AtomicBoolean cancelled=new AtomicBoolean();
        private Connection connection;
        final ThreadPoolExecutor executor;Statement statement;CompletableFuture<Void> cancellation=CompletableFuture.completedFuture(null);
        Job(String id,ThreadPoolExecutor executor){this.id=id;this.executor=executor;}
        void connection(Connection value){connection=value;}
        Connection connection() throws SQLException {if(connection==null)throw new SQLException("Metadata connection is unavailable","08003");return connection;}
        synchronized void attach(Statement value){statement=value;if(cancelled.get())schedule(value);}
        void detach(Statement value){
            CompletableFuture<Void> pending;
            synchronized(this){if(statement!=value)return;pending=cancellation;}
            pending.join();
            synchronized(this){if(statement==value)statement=null;}
        }
        private void schedule(Statement value){
            CompletableFuture<Void> pending=new CompletableFuture<>();cancellation=pending;
            try{executor.execute(()->{try{value.cancel();}catch(Throwable ignored){}finally{pending.complete(null);}});}
            catch(RejectedExecutionException ignored){pending.complete(null);}
        }
        synchronized boolean cancel(){boolean attached=statement!=null;cancelled.set(true);if(attached&&cancellation.isDone())schedule(statement);return attached;}
        void check() throws SQLException {if(cancelled.get()||Thread.currentThread().isInterrupted())throw new SQLException("Metadata request cancelled","57014");}
        boolean stopped(){return cancelled.get()||Thread.currentThread().isInterrupted();}
    }
    public CompletionStage<JsonNode> cancel(ObjectNode params) throws Exception {
        String id=ConnectionService.id(params,"metadataJobId");
        synchronized(cancelledBeforeStart){
            Job job=sources.get(id);
            if(job!=null){boolean attached=job.cancel();return RpcServer.result(FrameCodec.JSON.createObjectNode().put("status","requested").put("statementAttached",attached));}
            if(finished.contains(id))return RpcServer.result(FrameCodec.JSON.createObjectNode().put("status","alreadyFinished").put("statementAttached",false));
                if(cancelledBeforeStart.size()>=4096&&!cancelledBeforeStart.contains(id))throw new RpcException(-32003,"Early metadata cancellation capacity exhausted");
                cancelledBeforeStart.add(id);
            return RpcServer.result(FrameCodec.JSON.createObjectNode().put("status","requested").put("statementAttached",false));
        }
    }
    private void close(){for(Job job:sources.values())job.cancel();cancels.shutdown();}
    public CompletionStage<JsonNode> source(ObjectNode params) throws Exception {
        String connection=ConnectionService.id(params,"connectionId");
        String jobId=ConnectionService.id(params,"metadataJobId");
        if(!(params.get("request") instanceof ObjectNode request))throw new RpcException(-32602,"Package source request is required");
        Request parsed=Request.parse(request);
        if(!parsed.kind.equals("OBJECT")||!"PACKAGE".equals(parsed.objectType))throw new RpcException(-32602,"Package source requires a package object");
        Job job=new Job(jobId,cancels);
        synchronized(cancelledBeforeStart){
            if(cancelledBeforeStart.remove(jobId))job.cancelled.set(true);
            if(sources.size()>=64||finished.contains(jobId)||sources.putIfAbsent(jobId,job)!=null)throw new RpcException(-32003,"Metadata source capacity exhausted");
        }
        try{
            return connections.submit(connection,session->{
                job.check();PackageProvider.check(()->session.closing()||job.stopped());
                job.connection(session.connection());
                session.cancellation(job::cancel);
                try{
                    ObjectNode result=PackageProvider.detect(session.connection().getMetaData()).source(parsed,job);
                    job.check();result.put("contextRevision",session.contextRevision());return result;
                }finally{session.cancellation(null);}
            }).whenComplete((value,error)->{
                synchronized(cancelledBeforeStart){sources.remove(jobId,job);finished.add(jobId);if(finished.size()>4096)finished.remove(finished.iterator().next());}
            });
        }catch(Throwable failure){
            synchronized(cancelledBeforeStart){sources.remove(jobId,job);finished.add(jobId);if(finished.size()>4096)finished.remove(finished.iterator().next());}
            throw failure;
        }
    }
    public CompletionStage<JsonNode> list(ObjectNode params) throws Exception {
        String connection=ConnectionService.id(params,"connectionId");
        String jobId=ConnectionService.id(params,"metadataJobId");
        if(!(params.get("request") instanceof ObjectNode request))throw new RpcException(-32602,"Metadata request is required");
        Request parsed=Request.parse(request);
        Job job=new Job(jobId,cancels);
        synchronized(cancelledBeforeStart){
            if(cancelledBeforeStart.remove(jobId))job.cancelled.set(true);
            if(sources.size()>=64||finished.contains(jobId)||sources.putIfAbsent(jobId,job)!=null)throw new RpcException(-32003,"Metadata request capacity exhausted");
        }
        try{
            return connections.submit(connection,session->{
                job.check();job.connection(session.connection());session.cancellation(job::cancel);
                try{
                    Load load=new Load(session.connection().getMetaData(),parsed,session::closing,job);
                    ObjectNode result=load.run();job.check();result.put("contextRevision",session.contextRevision());return result;
                }finally{session.cancellation(null);}
            }).whenComplete((value,error)->finish(jobId,job));
        }catch(Throwable failure){finish(jobId,job);throw failure;}
    }
    private void finish(String jobId,Job job){synchronized(cancelledBeforeStart){sources.remove(jobId,job);finished.add(jobId);if(finished.size()>4096)finished.remove(finished.iterator().next());}}
    private static final Set<String> TYPES=Set.of("CATALOG","SCHEMA","CATEGORY","TABLE","VIEW","COLUMN","PROCEDURE","FUNCTION","PACKAGE","TYPE","INDEX","PRIMARY_KEY","FOREIGN_KEY");
    record Request(String kind,String catalog,String schema,String category,String objectName,String objectType,String objectId){
        static String text(ObjectNode value,String key) throws RpcException {
            JsonNode node=value.get(key);if(node==null||node.isNull())return null;
            if(!node.isTextual()||node.textValue().getBytes(StandardCharsets.UTF_8).length>4096||node.textValue().indexOf('\0')>=0)throw new RpcException(-32602,"Invalid metadata identifier");return node.textValue();
        }
        static Request parse(ObjectNode p) throws RpcException {
            Request r=new Request(text(p,"kind"),text(p,"catalog"),text(p,"schema"),text(p,"category"),text(p,"objectName"),text(p,"objectType"),text(p,"objectId"));
            if(r.kind==null||!Set.of("ROOT","CATALOG","SCHEMA","CATEGORY","OBJECT").contains(r.kind)||r.category!=null&&!TYPES.contains(r.category)||r.objectType!=null&&!TYPES.contains(r.objectType)||r.kind.equals("CATEGORY")&&r.category==null||r.kind.equals("OBJECT")&&(r.objectName==null||r.objectName.isEmpty()||r.objectType==null))throw new RpcException(-32602,"Invalid metadata request");return r;
        }
    }
    @FunctionalInterface interface Rows {ResultSet get() throws SQLException;}
    @FunctionalInterface interface MapRow {ObjectNode get(ResultSet row) throws SQLException;}
    @FunctionalInterface interface Flag {boolean get() throws SQLException;}
    static ObjectNode object(){return FrameCodec.JSON.createObjectNode();}
    static String empty(String s){return s==null?"":s;}
    static String qualified(String... parts) throws SQLException {return bounded(String.join(".",Arrays.stream(parts).filter(Objects::nonNull).toList()));}
    static SQLException limit(){return new SQLException("Metadata exceeds the bounded response; narrow the selected catalog or schema","54000");}
    static String bounded(String s) throws SQLException {if(s!=null&&s.getBytes(StandardCharsets.UTF_8).length>8192)throw limit();return s;}
    static String text(ResultSet r,String column) throws SQLException {return bounded(r.getString(column));}
    static final class Load {
        final DatabaseMetaData meta;final Request r;final java.util.function.BooleanSupplier closing;final Job job;
        final ArrayNode objects=FrameCodec.JSON.createArrayNode();final ObjectNode capabilities=object();
        final Set<String> messages=new LinkedHashSet<>(),ids=new HashSet<>();int bytes,scanned;
        PackageProvider packages;
        Load(DatabaseMetaData meta,Request r,java.util.function.BooleanSupplier closing){this(meta,r,closing,null);}
        Load(DatabaseMetaData meta,Request r,java.util.function.BooleanSupplier closing,Job job){this.meta=meta;this.r=r;this.closing=closing;this.job=job;}
        void check() throws SQLException {if(closing.getAsBoolean()||Thread.currentThread().isInterrupted())throw new SQLException("Metadata request cancelled","57014");if(job!=null)job.check();}
        boolean next(ResultSet result) throws SQLException {
            try{boolean available=result.next();check();return available;}
            catch(SQLException failure){
                try{check();}catch(SQLException cancelled){cancelled.addSuppressed(failure);throw cancelled;}
                throw failure;
            }
        }
        void unsupported(String name){messages.add("Driver does not support "+name);}
        boolean flag(String name,Flag getter) throws SQLException {try{return getter.get();}catch(SQLFeatureNotSupportedException|UnsupportedOperationException|AbstractMethodError e){unsupported(name);return false;}}
        ObjectNode node(String id,String name,String type,String catalog,String schema,boolean children) throws SQLException {
            ObjectNode node=object().put("id",bounded(id)).put("name",bounded(name)).put("qualifiedName",bounded(name)).put("objectType",type).put("hasChildren",children);
            node.put("catalog",catalog);node.put("schema",schema);node.putObject("attributes");return node;
        }
        void add(ObjectNode value) throws SQLException {
            if(value==null||!ids.add(value.path("id").asText()))return;
            if(objects.size()>=10000)throw limit();
            try{bytes+=FrameCodec.JSON.writeValueAsBytes(value).length;}catch(java.io.IOException e){throw new SQLException("Cannot encode metadata",e);}
            if(bytes>3*1024*1024)throw limit();objects.add(value);
        }
        void read(String name,Rows rows,MapRow mapper) throws SQLException {
            try(ResultSet result=rows.get()){
                if(result==null){unsupported(name);return;}
                Statement statement=job==null?null:result.getStatement();if(statement!=null)job.attach(statement);
                try{while(next(result)){if(++scanned>50000)throw limit();add(mapper.get(result));}}
                finally{if(statement!=null)job.detach(statement);}
            }catch(SQLFeatureNotSupportedException|UnsupportedOperationException|AbstractMethodError unsupported){unsupported(name);}
        }
        boolean any(String name,Rows rows) throws SQLException {
            try(ResultSet result=rows.get()){check();if(result==null){unsupported(name);return false;}Statement statement=job==null?null:result.getStatement();if(statement!=null)job.attach(statement);try{return next(result);}finally{if(statement!=null)job.detach(statement);}}
            catch(SQLFeatureNotSupportedException|UnsupportedOperationException|AbstractMethodError unsupported){unsupported(name);return false;}
        }
        String pattern(String name) throws SQLException {
            if(name==null)return null;String escape=meta.getSearchStringEscape();
            if(escape==null||escape.isEmpty()){
                if(name.contains("%")||name.contains("_"))throw new SQLFeatureNotSupportedException("Driver cannot escape metadata patterns");return name;
            }
            return name.replace(escape,escape+escape).replace("%",escape+"%").replace("_",escape+"_");
        }
        ObjectNode run() throws SQLException {
            packages=PackageProvider.detect(meta);
            check();capabilities.put("catalogs",flag("catalog capability",meta::supportsCatalogsInTableDefinitions));
            capabilities.put("schemas",flag("schema capability",meta::supportsSchemasInTableDefinitions));
            capabilities.put("transactions",flag("transaction capability",meta::supportsTransactions));
            capabilities.put("savepoints",flag("savepoint capability",meta::supportsSavepoints));
            capabilities.put("tables",true).put("views",true).put("procedures",flag("procedures",meta::supportsStoredProcedures)).put("functions",true).put("types",true);
            capabilities.put("packages",packages.supported()).put("packageMembers",packages.membersSupported()).put("packageSource",packages.sourceSupported()).put("packageDdl",packages.ddl());
            switch(r.kind){
                case "ROOT" -> root();
                case "CATALOG" -> {schemas(r.catalog);if(objects.isEmpty()&&r.catalog!=null)schemas(null);if(objects.isEmpty())categories();}
                case "SCHEMA" -> categories();
                case "CATEGORY" -> category();
                case "OBJECT" -> children();
                default -> throw new AssertionError();
            }
            if(messages.contains("Driver does not support functions"))capabilities.put("functions",false);
            if(messages.contains("Driver does not support types"))capabilities.put("types",false);
            if(messages.contains("Driver does not support procedures"))capabilities.put("procedures",false);
            if(messages.contains("Driver does not support tables"))capabilities.put("tables",false);
            if(messages.contains("Driver does not support views"))capabilities.put("views",false);
            ObjectNode result=object();result.set("objects",objects);result.set("capabilities",capabilities);
            if(!messages.isEmpty())result.put("message",String.join("; ",messages));return result;
        }
        void root() throws SQLException {
            read("catalogs",meta::getCatalogs,rs->{String name=text(rs,"TABLE_CAT");return name==null?null:node("catalog:"+name,name,"CATALOG",name,null,true);});
            if(!objects.isEmpty()){capabilities.put("catalogs",true);return;}
            schemas(null);if(objects.isEmpty())categories();
        }
        void schemas(String catalog) throws SQLException {
            int before=objects.size();read("schemas",()->meta.getSchemas(catalog,null),rs->{String name=text(rs,"TABLE_SCHEM");if(name==null)return null;String actual=text(rs,"TABLE_CATALOG");if(actual==null)actual=catalog;return node("schema:"+empty(actual)+":"+name,name,"SCHEMA",actual,name,true).put("qualifiedName",qualified(actual,name));});
            if(objects.size()>before)capabilities.put("schemas",true);
        }
        void categoryNode(String name,String type) throws SQLException {ObjectNode n=node("category:"+empty(r.catalog)+":"+empty(r.schema)+":"+type,name,"CATEGORY",r.catalog,r.schema,true);((ObjectNode)n.path("attributes")).put("categoryType",type);add(n);}
        void categories() throws SQLException {
            Set<String> types=new HashSet<>();
            try(ResultSet rows=meta.getTableTypes()){if(rows==null)unsupported("table types");else{Statement statement=job==null?null:rows.getStatement();if(statement!=null)job.attach(statement);try{while(next(rows)){String type=text(rows,"TABLE_TYPE");if(type!=null)types.add(type.toUpperCase(Locale.ROOT));if(types.size()>256)throw limit();}}finally{if(statement!=null)job.detach(statement);}}}
            catch(SQLFeatureNotSupportedException|UnsupportedOperationException|AbstractMethodError e){unsupported("table types");}
            if(types.isEmpty()||types.contains("TABLE")||types.contains("BASE TABLE"))categoryNode("Tables","TABLE");
            if(types.isEmpty()||types.contains("VIEW"))categoryNode("Views","VIEW");
            if(capabilities.path("procedures").asBoolean())categoryNode("Procedures","PROCEDURE");
            categoryNode("Functions","FUNCTION");if(packages.supported())categoryNode("Packages","PACKAGE");categoryNode("Types","TYPE");
        }
        void category() throws SQLException {
            switch(r.category){
                case "TABLE","VIEW" -> {tables(r.catalog);if(objects.isEmpty()&&r.catalog!=null)tables(null);}
                case "PROCEDURE","FUNCTION" -> routines();
                case "TYPE" -> read("types",meta::getTypeInfo,rs->{String name=text(rs,"TYPE_NAME");if(name==null)return null;ObjectNode n=node("type:"+name,name,"TYPE",r.catalog,r.schema,false);((ObjectNode)n.path("attributes")).put("dataType",Integer.toString(rs.getInt("DATA_TYPE")));return n;});
                case "PACKAGE" -> packages.list(this);
                default -> unsupported("this metadata category");
            }
        }
        void tables(String catalog) throws SQLException {
            String[] types=r.category.equals("VIEW")?new String[]{"VIEW"}:new String[]{"TABLE","BASE TABLE"};
            read(r.category.equals("VIEW")?"views":"tables",()->meta.getTables(catalog,pattern(r.schema),"%",types),rs->{
                String name=text(rs,"TABLE_NAME");if(name==null)return null;String schema=text(rs,"TABLE_SCHEM"),cat=text(rs,"TABLE_CAT");if(schema==null)schema=r.schema;if(cat==null)cat=r.catalog;
                String type="VIEW".equalsIgnoreCase(text(rs,"TABLE_TYPE"))?"VIEW":"TABLE";
                ObjectNode n=node("object:"+empty(cat)+":"+empty(schema)+":"+type+":"+name,name,type,cat,schema,true).put("qualifiedName",qualified(schema,name));
                ((ObjectNode)n.path("attributes")).put("remarks",empty(text(rs,"REMARKS")));return n;
            });
        }
        void routines() throws SQLException {
            boolean procedure=r.category.equals("PROCEDURE");String prefix=procedure?"PROCEDURE":"FUNCTION";
            read(procedure?"procedures":"functions",()->procedure?meta.getProcedures(r.catalog,pattern(r.schema),"%"):meta.getFunctions(r.catalog,pattern(r.schema),"%"),rs->{
                String name=text(rs,prefix+"_NAME");if(name==null)return null;String catalog=text(rs,prefix+"_CAT"),schema=text(rs,prefix+"_SCHEM");if(catalog==null)catalog=r.catalog;if(schema==null)schema=r.schema;
                ObjectNode n=node(prefix.toLowerCase(Locale.ROOT)+":"+empty(catalog)+":"+empty(schema)+":"+name,name,prefix,catalog,schema,true).put("qualifiedName",qualified(schema,name));return n;
            });
        }
        ObjectNode child(String prefix,String name,String type,String suffix) throws SQLException {return node(prefix+":"+empty(r.catalog)+":"+empty(r.schema)+":"+r.objectName+":"+name+suffix,name,type,r.catalog,r.schema,false);}
        void children() throws SQLException {
            if("PACKAGE".equals(r.objectType)){packages.members(this);return;}
            if(Set.of("PROCEDURE","FUNCTION").contains(r.objectType)){
                boolean procedure=r.objectType.equals("PROCEDURE");
                read(procedure?"procedure parameters":"function parameters",()->procedure?meta.getProcedureColumns(r.catalog,pattern(r.schema),pattern(r.objectName),"%"):meta.getFunctionColumns(r.catalog,pattern(r.schema),pattern(r.objectName),"%"),rs->{
                    String name=text(rs,"COLUMN_NAME");if(name==null)return null;ObjectNode n=child("parameter",name,"COLUMN","").put("qualifiedName",qualified(r.objectName,name));((ObjectNode)n.path("attributes")).put("type",empty(text(rs,"TYPE_NAME"))).put("mode",Integer.toString(rs.getInt("COLUMN_TYPE")));return n;
                });return;
            }
            if(!Set.of("TABLE","VIEW").contains(r.objectType)){unsupported("children for this object type");return;}
            read("columns",()->meta.getColumns(r.catalog,pattern(r.schema),pattern(r.objectName),"%"),rs->{
                String name=text(rs,"COLUMN_NAME");if(name==null)return null;ObjectNode n=child("column",name,"COLUMN","").put("qualifiedName",qualified(r.objectName,name));((ObjectNode)n.path("attributes")).put("type",empty(text(rs,"TYPE_NAME"))).put("size",Integer.toString(rs.getInt("COLUMN_SIZE"))).put("nullable",Integer.toString(rs.getInt("NULLABLE"))).put("ordinal",Integer.toString(rs.getInt("ORDINAL_POSITION")));return n;
            });
            read("primary keys",()->meta.getPrimaryKeys(r.catalog,r.schema,r.objectName),rs->{String name=text(rs,"PK_NAME"),column=text(rs,"COLUMN_NAME");if(name==null)name="PRIMARY KEY";ObjectNode n=child("pk",name,"PRIMARY_KEY",":"+empty(column));((ObjectNode)n.path("attributes")).put("column",empty(column)).put("sequence",Integer.toString(rs.getInt("KEY_SEQ")));return n;});
            read("indexes",()->meta.getIndexInfo(r.catalog,r.schema,r.objectName,false,false),rs->{String name=text(rs,"INDEX_NAME"),column=text(rs,"COLUMN_NAME");if(name==null)return null;ObjectNode n=child("idx",name,"INDEX",":"+empty(column));((ObjectNode)n.path("attributes")).put("column",empty(column)).put("nonUnique",Boolean.toString(rs.getBoolean("NON_UNIQUE")));return n;});
            read("foreign keys",()->meta.getImportedKeys(r.catalog,r.schema,r.objectName),rs->{String name=text(rs,"FK_NAME"),column=text(rs,"FKCOLUMN_NAME");if(name==null)name="FOREIGN KEY";ObjectNode n=child("fk",name,"FOREIGN_KEY",":"+empty(column));((ObjectNode)n.path("attributes")).put("column",empty(column)).put("referencedCatalog",empty(text(rs,"PKTABLE_CAT"))).put("referencedSchema",empty(text(rs,"PKTABLE_SCHEM"))).put("referencedTable",empty(text(rs,"PKTABLE_NAME"))).put("referencedColumn",empty(text(rs,"PKCOLUMN_NAME"))).put("sequence",Integer.toString(rs.getInt("KEY_SEQ"))).put("updateRule",Integer.toString(rs.getInt("UPDATE_RULE"))).put("deleteRule",Integer.toString(rs.getInt("DELETE_RULE")));return n;});
        }
    }
}
