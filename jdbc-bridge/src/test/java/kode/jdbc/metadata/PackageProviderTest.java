package kode.jdbc.metadata;

import com.fasterxml.jackson.databind.JsonNode;
import kode.jdbc.protocol.FrameCodec;
import java.io.Reader;
import java.io.StringReader;
import java.lang.reflect.*;
import java.sql.*;
import java.util.*;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.*;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.AfterAll;
import static org.junit.jupiter.api.Assertions.*;

/** Provider fixtures verify query/ownership contracts, not real Oracle execution. */
@Timeout(10)
class PackageProviderTest {
    private static final ThreadPoolExecutor CANCEL_EXECUTOR=new ThreadPoolExecutor(1,1,0,TimeUnit.MILLISECONDS,new ArrayBlockingQueue<>(8),Thread.ofPlatform().daemon(true).name("package-test-cancel").factory(),new ThreadPoolExecutor.AbortPolicy());
    private static MetadataService.Job job(){return new MetadataService.Job("fixture",CANCEL_EXECUTOR);}
    @AfterAll static void stopCancellationExecutor(){CANCEL_EXECUTOR.shutdownNow();}
    record Query(String sql,List<String> values){}
    @FunctionalInterface interface Answer {ResultSet rows(Query query) throws Exception;}
    static final class OracleFixture implements AutoCloseable {
        final Connection h2=MetadataServiceTest.h2();
        final List<Query> queries=new ArrayList<>();
        final List<AtomicBoolean> closed=new ArrayList<>();
        final DatabaseMetaData metadata;
        final Connection connection;
        OracleFixture(Answer answer) throws Exception {this(answer,"Oracle");}
        OracleFixture(Answer answer,String product) throws Exception {
            connection=(Connection)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{Connection.class},(p,m,args)->{
                if(m.getName().equals("getSchema"))return "default_owner";
                if(m.getName().equals("prepareStatement")){
                    String sql=(String)args[0];List<String> values=new ArrayList<>();AtomicBoolean ended=new AtomicBoolean();closed.add(ended);
                    return Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{PreparedStatement.class},(s,op,a)->switch(op.getName()){
                        case "setString" -> {int index=(int)a[0];while(values.size()<index)values.add(null);values.set(index-1,(String)a[1]);yield null;}
                        case "setQueryTimeout" -> {assertEquals(15,a[0]);yield null;}
                        case "executeQuery" -> {Query query=new Query(sql,List.copyOf(values));queries.add(query);yield answer.rows(query);}
                        case "close" -> {ended.set(true);yield null;}
                        case "isClosed" -> ended.get();
                        default -> throw new AssertionError("Unexpected statement method: "+op.getName());
                    });
                }
                throw new AssertionError("Unexpected connection method: "+m.getName());
            });
            metadata=MetadataServiceTest.proxy(h2.getMetaData(),(m,args)->switch(m.getName()){
                case "getDatabaseProductName" -> product;
                case "getConnection" -> connection;
                default -> MetadataServiceTest.PASS;
            });
        }
        @Override public void close() throws Exception {h2.close();}
        void assertStatementsClosed(){assertFalse(closed.isEmpty());assertTrue(closed.stream().allMatch(AtomicBoolean::get));}
    }
    static ResultSet rows(String[] labels,Object[]... values) throws Exception {return MetadataServiceTest.rows(labels,values);}
    static MetadataService.Request request(String kind,String schema,String category,String name,String type){return MetadataServiceTest.request(kind,"catalog",schema,category,name,type);}
    @Test void onlyARecognizedPackageProviderAdvertisesAndCreatesPackageCategories() throws Exception {
        try(Connection h2=MetadataServiceTest.h2();OracleFixture oracle=new OracleFixture(q->{throw new AssertionError("No package SQL on category discovery");})){
            var r=request("SCHEMA","owner",null,null,null);
            JsonNode unsupported=MetadataServiceTest.load(h2.getMetaData(),r);
            assertFalse(unsupported.path("capabilities").path("packages").asBoolean());
            assertTrue(unsupported.path("objects").findValuesAsText("name").stream().noneMatch("Packages"::equals));
            JsonNode supported=MetadataServiceTest.load(oracle.metadata,r);
            assertTrue(supported.path("capabilities").path("packages").asBoolean());
            assertTrue(supported.path("capabilities").path("packageDdl").asBoolean());
            assertEquals("PACKAGE",MetadataServiceTest.named(supported,"CATEGORY","Packages").path("attributes").path("categoryType").asText());
        }
    }
    @Test void shippedDatabaseFamiliesOnlyAdvertiseTheirExplicitPackageProviderCapabilities() throws Exception {
        JsonNode catalog=FrameCodec.JSON.readTree(java.nio.file.Files.readString(java.nio.file.Path.of("..","resources","database","drivers.json")));
        Set<String> families=new TreeSet<>();for(JsonNode driver:catalog.path("drivers"))families.add(driver.path("databaseFamily").asText());
        Set<String> supportedFamilies=Set.of("MariaDB","Oracle","Tibero","DB2");
        assertTrue(families.containsAll(supportedFamilies));
        for(String family:families){
            try(OracleFixture fixture=new OracleFixture(query->{throw new AssertionError("Family routing must not query package objects");},family)){
                DatabaseMetaData metadata=fixture.metadata;
                if(family.equals("MariaDB"))metadata=MetadataServiceTest.proxy(metadata,(method,args)->switch(method.getName()){case "getDatabaseMajorVersion"->11;case "getDatabaseMinorVersion"->8;default->MetadataServiceTest.PASS;});
                PackageProvider provider=PackageProvider.detect(metadata);boolean supported=supportedFamilies.contains(family);
                assertEquals(supported,provider.supported(),family);
                if(!supported){assertEquals("none",provider.name(),family);assertFalse(provider.ddl(),family);assertFalse(provider.membersSupported(),family);assertFalse(provider.sourceSupported(),family);}
                else if(family.equals("DB2")){assertEquals("db2-luw",provider.name());assertFalse(provider.ddl());assertFalse(provider.membersSupported());assertTrue(provider.sourceSupported());}
                else{assertTrue(provider.ddl(),family);assertTrue(provider.membersSupported(),family);assertTrue(provider.sourceSupported(),family);}
            }
        }
    }
    @Test void exactSchemaAndPackageNamesAreBoundAndOverloadedMembersHaveDistinctIdentities() throws Exception {
        String owner="A_%'\"界",name="P_%'\"界";
        try(OracleFixture oracle=new OracleFixture(q->{
            assertEquals(owner,q.values().getFirst());assertFalse(q.sql().contains(owner));
            if(q.sql().contains("FROM ALL_OBJECTS"))return rows(new String[]{"OBJECT_NAME","STATUS"},new Object[]{name,"VALID"});
            assertEquals(List.of(owner,name),q.values());assertTrue(q.sql().contains("a.POSITION = 0"));
            return rows(new String[]{"PROCEDURE_NAME","SUBPROGRAM_ID","OVERLOAD","ROUTINE_KIND"},
                new Object[]{"same_name","1","1","FUNCTION"},new Object[]{"same_name","2","2","FUNCTION"},new Object[]{"do_work","3",null,"PROCEDURE"});
        })){
            JsonNode packages=MetadataServiceTest.load(oracle.metadata,request("CATEGORY",owner,"PACKAGE",null,null));
            JsonNode pkg=MetadataServiceTest.named(packages,"PACKAGE",name);assertEquals(owner+"."+name,pkg.path("qualifiedName").asText());
            assertEquals(owner,pkg.path("schema").asText());assertEquals("catalog",pkg.path("catalog").asText());assertEquals("true",pkg.path("attributes").path("sourceAvailable").asText());
            JsonNode members=MetadataServiceTest.load(oracle.metadata,request("OBJECT",owner,null,name,"PACKAGE"));
            assertEquals(2,MetadataServiceTest.nodes(members,"FUNCTION").size());assertEquals(1,MetadataServiceTest.nodes(members,"PROCEDURE").size());
            Set<String> ids=new HashSet<>();for(JsonNode member:members.path("objects")){assertTrue(ids.add(member.path("id").asText()));assertEquals(name,member.path("attributes").path("packageName").asText());assertTrue(member.path("attributes").path("overload").isTextual());assertFalse(member.path("hasChildren").asBoolean());}
            oracle.assertStatementsClosed();
        }
    }
    @Test void tiberoPackagesMembersAndStoredSourceUseDocumentedCatalogViews() throws Exception {
        String owner="APP_界",packageName="PKG_%";
        String specification="CREATE OR REPLACE PACKAGE \"PKG_%\" AS\n  FUNCTION calc(x NUMBER) RETURN NUMBER;\nEND;\n";
        String body="CREATE OR REPLACE PACKAGE BODY \"PKG_%\" AS\n  FUNCTION calc(x NUMBER) RETURN NUMBER IS BEGIN RETURN x; END;\nEND;\n";
        try(OracleFixture tibero=new OracleFixture(query->{
            assertEquals(List.of(owner),query.values().size()==1?query.values():query.values().subList(0,1));
            if(query.sql().contains("FROM ALL_OBJECTS"))return rows(new String[]{"OBJECT_NAME","STATUS"},new Object[]{packageName,"VALID"});
            if(query.sql().contains("FROM ALL_PROCEDURES")){
                assertEquals(List.of(owner,packageName),query.values());assertTrue(query.sql().contains("MEMBER_NO"));
                return rows(new String[]{"PROCEDURE_NAME","FUNCTIONABLE","MEMBER_NO"},
                    new Object[]{"calc","YES",1},new Object[]{"calc","YES",2},new Object[]{"increment","NO",3});
            }
            assertTrue(query.sql().contains("FROM ALL_SOURCE"));assertEquals(List.of(owner,packageName),query.values());
            return rows(new String[]{"TYPE","LINE","TEXT"},
                new Object[]{"PACKAGE BODY",3,"END;\n"},
                new Object[]{"PACKAGE BODY",2,"  FUNCTION calc(x NUMBER) RETURN NUMBER IS BEGIN RETURN x; END;\n"},
                new Object[]{"PACKAGE BODY",1,"CREATE OR REPLACE PACKAGE BODY \"PKG_%\" AS\n"},
                new Object[]{"PACKAGE",3,"END;\n"},
                new Object[]{"PACKAGE",2,"  FUNCTION calc(x NUMBER) RETURN NUMBER;\n"},
                new Object[]{"PACKAGE",1,"CREATE OR REPLACE PACKAGE \"PKG_%\" AS\n"});
        },"Tibero")){
            PackageProvider provider=PackageProvider.detect(tibero.metadata);assertEquals("tibero",provider.name());assertTrue(provider.supported());
            JsonNode packages=MetadataServiceTest.load(tibero.metadata,request("CATEGORY",owner,"PACKAGE",null,null));
            assertEquals(packageName,MetadataServiceTest.named(packages,"PACKAGE",packageName).path("name").asText());
            JsonNode members=MetadataServiceTest.load(tibero.metadata,request("OBJECT",owner,null,packageName,"PACKAGE"));
            assertEquals(2,MetadataServiceTest.nodes(members,"FUNCTION").size());assertEquals(1,MetadataServiceTest.nodes(members,"PROCEDURE").size());
            Set<String> memberIds=new HashSet<>();for(JsonNode member:members.path("objects")){assertTrue(memberIds.add(member.path("id").asText()));assertEquals("tibero",member.path("attributes").path("packageProvider").asText());}
            MetadataService.Job sourceJob=job();sourceJob.connection(tibero.connection);
            MetadataService.Request sourceRequest=new MetadataService.Request("OBJECT","catalog",owner,null,packageName,"PACKAGE",null);
            JsonNode source=provider.source(sourceRequest,sourceJob);assertEquals(2,source.path("documents").size());
            assertEquals(Set.of(specification,body),new HashSet<>(source.path("documents").findValuesAsText("text")));
            tibero.assertStatementsClosed();
        }
    }
    @Test void ddlUsesActualReturnedCharactersAndIncludesTheExistingSpecificationAndBodyOnly() throws Exception {
        String specification="CREATE OR REPLACE PACKAGE \"P\" AS\n  FUNCTION calc(x NUMBER) RETURN NUMBER;\nEND;\n";
        String body="CREATE OR REPLACE PACKAGE BODY \"P\" AS\n  FUNCTION calc(x NUMBER) RETURN NUMBER IS BEGIN RETURN x; END;\nEND;\n";
        try(OracleFixture oracle=new OracleFixture(q->{
            if(q.sql().contains("FROM ALL_OBJECTS"))return rows(new String[]{"OBJECT_TYPE"},new Object[]{"PACKAGE"},new Object[]{"PACKAGE BODY"});
            assertEquals("SELECT DBMS_METADATA.GET_DDL(?, ?, ?) AS DDL FROM DUAL",q.sql());assertEquals(List.of("P","owner"),q.values().subList(1,3));
            return rows(new String[]{"DDL"},new Object[]{q.values().getFirst().equals("PACKAGE_SPEC")?specification:body});
        })){
            JsonNode result=PackageProvider.detect(oracle.metadata).source(request("OBJECT","owner",null,"P","PACKAGE"),job());
            assertEquals(2,result.path("documents").size());Set<String> actual=new HashSet<>();for(JsonNode doc:result.path("documents")){assertEquals("DDL",doc.path("kind").asText());actual.add(doc.path("text").asText());}
            assertEquals(Set.of(specification,body),actual);oracle.assertStatementsClosed();
        }
    }
    @Test void privilegeAndMissingObjectErrorsPropagateWithoutSubstituteDefinitions() throws Exception {
        try(OracleFixture denied=new OracleFixture(q->{throw new SQLException("not enough privileges","42000",1031);});
            OracleFixture absent=new OracleFixture(q->rows(new String[]{"OBJECT_TYPE"}))){
            var request=request("OBJECT","owner",null,"P","PACKAGE");
            assertEquals(1031,assertThrows(SQLException.class,()->PackageProvider.detect(denied.metadata).source(request,job())).getErrorCode());
            assertEquals("02000",assertThrows(SQLException.class,()->PackageProvider.detect(absent.metadata).source(request,job())).getSQLState());
            denied.assertStatementsClosed();absent.assertStatementsClosed();
        }
    }
    @Test void sourceLimitsAndCancellationCloseReadersWithoutReturningPartialDdl() throws Exception {
        AtomicBoolean closed=new AtomicBoolean();Reader tooLarge=new StringReader("x".repeat(2*1024*1024+1)){@Override public void close(){closed.set(true);super.close();}};
        assertEquals("54000",assertThrows(SQLException.class,()->PackageProvider.definition(tooLarge,()->false)).getSQLState());assertTrue(closed.get());
        closed.set(false);Reader cancelled=new StringReader("source"){@Override public void close(){closed.set(true);super.close();}};
        assertEquals("57014",assertThrows(SQLException.class,()->PackageProvider.definition(cancelled,()->true)).getSQLState());assertTrue(closed.get());
        closed.set(false);Reader cancelledRead=new Reader(){
            @Override public int read(char[] buffer,int offset,int length) throws java.io.IOException {throw new java.io.IOException("vendor cancelled streamed DDL read");}
            @Override public void close(){closed.set(true);}
        };
        assertEquals("57014",assertThrows(SQLException.class,()->PackageProvider.definition(cancelledRead,()->true)).getSQLState());assertTrue(closed.get());
        assertEquals("42501",assertThrows(SQLException.class,()->PackageProvider.definition(new StringReader(""),()->false)).getSQLState());
    }
    @Test void oraclePackageSourceCancelsTheActiveDefinitionQueryAndReturnsNoPartialDocuments() throws Exception {
        CountDownLatch reading=new CountDownLatch(1),cancelled=new CountDownLatch(1),closed=new CountDownLatch(1);
        AtomicBoolean ddlStatementClosed=new AtomicBoolean();
        Connection connection=(Connection)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{Connection.class},(proxy,method,args)->{
            if(method.getName().equals("getSchema"))return "default_owner";
            if(method.getName().equals("prepareStatement")){
                String sql=(String)args[0];PreparedStatement[] reference=new PreparedStatement[1];AtomicBoolean ended=new AtomicBoolean();
                reference[0]=(PreparedStatement)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{PreparedStatement.class},(prepared,operation,values)->switch(operation.getName()){
                    case "setString","setQueryTimeout" -> null;
                    case "executeQuery" -> {
                        if(sql.contains("FROM ALL_OBJECTS"))yield rows(new String[]{"OBJECT_TYPE"},new Object[]{"PACKAGE"});
                        yield Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{ResultSet.class},(result,op,values2)->switch(op.getName()){
                            case "getStatement" -> reference[0];
                            case "next" -> {reading.countDown();if(!cancelled.await(3,TimeUnit.SECONDS))throw new SQLException("Statement.cancel was not delivered");throw new SQLException("Vendor cancellation error","72000");}
                            case "close" -> {closed.countDown();yield null;}
                            default -> op.getReturnType().isPrimitive()?false:null;
                        });
                    }
                    case "cancel" -> {cancelled.countDown();yield null;}
                    case "close" -> {ended.set(true);if(sql.contains("DBMS_METADATA"))ddlStatementClosed.set(true);yield null;}
                    case "isClosed" -> ended.get();
                    default -> operation.getReturnType().isPrimitive()?false:null;
                });
                return reference[0];
            }
            throw new AssertionError("Unexpected connection method: "+method.getName());
        });
        try(Connection h2=MetadataServiceTest.h2()){
            DatabaseMetaData metadata=MetadataServiceTest.proxy(h2.getMetaData(),(method,args)->switch(method.getName()){
                case "getDatabaseProductName" -> "Oracle";
                case "getConnection" -> connection;
                default -> MetadataServiceTest.PASS;
            });
            MetadataService.Job job=job();job.connection(connection);
            ExecutorService owner=Executors.newSingleThreadExecutor();
            try{
                Future<?> work=owner.submit(()->{
                    try{return PackageProvider.detect(metadata).source(request("OBJECT","default_owner",null,"P","PACKAGE"),job);}
                    catch(SQLException failure){throw new CompletionException(failure);}
                });
                assertTrue(reading.await(3,TimeUnit.SECONDS));assertTrue(job.cancel());
                ExecutionException failure=assertThrows(ExecutionException.class,()->work.get(4,TimeUnit.SECONDS));
                Throwable cause=failure;while(cause!=null&&!(cause instanceof SQLException))cause=cause.getCause();
                assertNotNull(cause);assertEquals("57014",((SQLException)cause).getSQLState());
                assertTrue(cancelled.await(1,TimeUnit.SECONDS));assertTrue(closed.await(1,TimeUnit.SECONDS));assertTrue(ddlStatementClosed.get());
            }finally{owner.shutdownNow();}
        }
    }
    @Test void ownerWaitsForTheSpecificStatementCancelBeforeReleasingIt() throws Exception {
        var entered=new java.util.concurrent.CountDownLatch(1);
        var release=new java.util.concurrent.CountDownLatch(1);
        var detached=new AtomicBoolean();
        Statement statement=(Statement)Proxy.newProxyInstance(getClass().getClassLoader(),new Class<?>[]{Statement.class},(proxy,method,args)->{
            if(method.getName().equals("cancel")){entered.countDown();release.await();return null;}
            throw new AssertionError("Unexpected statement method: "+method.getName());
        });
        MetadataService.Job job=job();
        job.attach(statement);job.cancel();
        assertTrue(entered.await(2,TimeUnit.SECONDS));
        Thread owner=Thread.ofPlatform().start(()->{job.detach(statement);detached.set(true);});
        try {
            assertFalse(detached.get(),"owner must not release a statement while its cancel call is active");
        } finally {
            release.countDown();owner.join(2000);
        }
        assertTrue(detached.get());
    }

    @Test void db2KeepsBoundPackageVersionIdentityAndReturnsStoredSqlWithoutInventingDdl() throws Exception {
        String schema="APP_界",name="PACK_%",version="VERSION_1",token="0123456789ABCDEF",unversionedToken="FEDCBA9876543210";
        try(OracleFixture db2=new OracleFixture(query->{
            if(query.sql().contains("SYSCAT.PACKAGES")){
                assertEquals(List.of(schema),query.values());assertTrue(query.sql().contains("HEX(UNIQUE_ID)"));
                return rows(new String[]{"PKGNAME","PKGVERSION","PACKAGE_ID","VALID","BOUNDBY"},
                    new Object[]{name,version,token,"Y","OWNER"},new Object[]{name,"",unversionedToken,"Y","OWNER"});
            }
            assertTrue(query.sql().contains("SYSCAT.STATEMENTS"));
            String text;
            if(query.values().size()==4){assertEquals(List.of(schema,name,token,version),query.values());assertTrue(query.sql().contains("VERSION = ?"));text="SELECT 'é界😀' FROM SYSIBM.SYSDUMMY1";}
            else{assertEquals(List.of(schema,name,unversionedToken),query.values());assertTrue(query.sql().contains("VERSION IS NULL OR VERSION = ''"));text="VALUES 'unversioned-界'";}
            assertFalse(query.sql().contains(schema));assertFalse(query.sql().contains(name));
            return rows(new String[]{"STMTNO","SECTNO","TEXT"},new Object[]{"3",2,text});
        },"DB2/LINUXX8664")){
            var category=request("CATEGORY",schema,"PACKAGE",null,null);
            JsonNode listed=MetadataServiceTest.load(db2.metadata,category);
            assertTrue(listed.path("capabilities").path("packages").asBoolean());
            assertFalse(listed.path("capabilities").path("packageMembers").asBoolean());
            assertTrue(listed.path("capabilities").path("packageSource").asBoolean());
            assertFalse(listed.path("capabilities").path("packageDdl").asBoolean());
            List<JsonNode> packages=MetadataServiceTest.nodes(listed,"PACKAGE");assertEquals(2,packages.size());
            JsonNode versioned=packages.stream().filter(p->p.path("attributes").path("packageVersion").asText().equals(version)).findFirst().orElseThrow();
            JsonNode unversioned=packages.stream().filter(p->p.path("attributes").path("packageUniqueId").asText().equals(unversionedToken)).findFirst().orElseThrow();
            assertFalse(versioned.path("hasChildren").asBoolean());assertEquals(token,versioned.path("attributes").path("packageUniqueId").asText());
            MetadataService.Job job=job();job.connection(db2.connection);
            for(JsonNode pkg:List.of(versioned,unversioned)){
                MetadataService.Request sourceRequest=new MetadataService.Request("OBJECT","catalog",schema,null,name,"PACKAGE",pkg.path("id").asText());
                JsonNode source=PackageProvider.detect(db2.metadata).source(sourceRequest,job);
                assertEquals("SQL_SOURCE",source.path("documents").get(0).path("kind").asText());
                assertTrue(source.path("documents").get(0).path("text").asText().contains(pkg==versioned?"é界😀":"unversioned-界"));
                if(pkg==unversioned)assertTrue(source.path("documents").get(0).path("title").asText().contains("(no version)"));
            }
            assertTrue(db2.queries.stream().anyMatch(q->q.sql().contains("VERSION IS NULL")));
            db2.assertStatementsClosed();
        }
    }
    @Test void mariadbPackageDeclarationsIgnoreCommentsLiteralsAndNestedParameterTokens() throws Exception {
        String ddl=String.join("\n",
            "CREATE DEFINER='owner'@'host' PACKAGE \"MixedCase\" AS",
            "  -- FUNCTION hidden(x INT) RETURN INT;",
            "  FUNCTION \"Calc\"(x DECIMAL(10, 2), label VARCHAR(30)) RETURN VARCHAR(80);",
            "  PROCEDURE \"DoWork\"(x IN OUT DECIMAL(10, 2));",
            "  COMMENT 'PROCEDURE fake()';",
            "END");
        var parsed=PackageProvider.MariaDb.declarations(ddl);
        assertEquals(List.of(new PackageProvider.MariaDb.Routine("FUNCTION","Calc"),
            new PackageProvider.MariaDb.Routine("PROCEDURE","DoWork")),parsed);
    }
}
