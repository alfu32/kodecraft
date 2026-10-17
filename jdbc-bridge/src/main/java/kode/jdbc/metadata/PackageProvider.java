package kode.jdbc.metadata;

import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.protocol.FrameCodec;
import java.io.IOException;
import java.io.Reader;
import java.nio.charset.StandardCharsets;
import java.sql.*;
import java.util.*;
import java.util.function.BooleanSupplier;

/** Vendor package operations run on the same JDBC connection owner as metadata. */
interface PackageProvider {
    String name();
    boolean supported();
    default boolean ddl(){return supported();}
    default boolean membersSupported(){return supported();}
    default boolean sourceSupported(){return supported();}
    void list(MetadataService.Load load) throws SQLException;
    void members(MetadataService.Load load) throws SQLException;
    ObjectNode source(MetadataService.Request request, MetadataService.Job job) throws SQLException;

    static PackageProvider detect(DatabaseMetaData meta) throws SQLException {
        String product=meta.getDatabaseProductName();
        if(product!=null&&product.toLowerCase(Locale.ROOT).contains("mariadb"))
            return MariaDb.detect(meta);
        if(product!=null&&product.toLowerCase(Locale.ROOT).contains("tibero"))
            return new Tibero(meta);
        if(product!=null&&product.toLowerCase(Locale.ROOT).contains("db2"))
            return new Db2(meta);
        if(product!=null && (product.equalsIgnoreCase("Oracle") || product.toLowerCase(Locale.ROOT).startsWith("oracle database")))
            return new Oracle(meta);
        return new Unsupported();
    }

    final class Unsupported implements PackageProvider {
        public String name(){return "none";}
        public boolean supported(){return false;}
        public void list(MetadataService.Load load){load.unsupported("package discovery for this database");}
        public void members(MetadataService.Load load){load.unsupported("package members for this database");}
        public ObjectNode source(MetadataService.Request request,MetadataService.Job job) throws SQLException {
            throw new SQLFeatureNotSupportedException("This database has no supported package source provider","0A000");
        }
    }

    static String identity(String... parts){
        ArrayNode id=FrameCodec.JSON.createArrayNode();for(String part:parts)id.add(part);return id.toString();
    }
    static void check(BooleanSupplier closing) throws SQLException {
        if(closing.getAsBoolean()||Thread.currentThread().isInterrupted())
            throw new SQLException("Package metadata request cancelled","57014");
    }
    static boolean next(ResultSet rows,MetadataService.Job job) throws SQLException {
        try{boolean available=rows.next();job.check();return available;}
        catch(SQLException failure){
            try{job.check();}catch(SQLException cancelled){cancelled.addSuppressed(failure);throw cancelled;}
            throw failure;
        }
    }
    static ResultSet query(MetadataService.Rows operation,MetadataService.Job job) throws SQLException {
        try{return operation.get();}
        catch(SQLException failure){
            try{job.check();}catch(SQLException cancelled){cancelled.addSuppressed(failure);throw cancelled;}
            throw failure;
        }
    }
    static PreparedStatement prepare(Connection connection,String sql,String... values) throws SQLException {
        PreparedStatement statement=connection.prepareStatement(sql);
        try{
            for(int index=0;index<values.length;index++)statement.setString(index+1,values[index]);
            try{statement.setQueryTimeout(15);}catch(SQLFeatureNotSupportedException ignored){}
            return statement;
        }catch(Throwable failure){try{statement.close();}catch(Throwable close){failure.addSuppressed(close);}throw failure;}
    }
    static String owner(DatabaseMetaData meta,MetadataService.Request request) throws SQLException {
        if(request.schema()!=null)return request.schema();
        String schema;
        try{schema=meta.getConnection().getSchema();}catch(SQLFeatureNotSupportedException|AbstractMethodError unsupported){schema=null;}
        if(schema==null||schema.isEmpty())schema=meta.getUserName();
        if(schema==null||schema.isEmpty())throw new SQLException("Select a schema before loading packages","3F000");
        return schema;
    }
    /** Read the complete source or fail; never truncate a definition into plausible DDL. */
    static String definition(Reader reader,BooleanSupplier closing) throws SQLException {
        if(reader==null)throw new SQLException("Package definition is unavailable; check source privileges","42501");
        try(reader){
            StringBuilder text=new StringBuilder();char[] buffer=new char[8192];int count;
            while((count=reader.read(buffer))!=-1){check(closing);text.append(buffer,0,count);if(text.length()>2*1024*1024)throw MetadataService.limit();}
            String value=text.toString();if(value.getBytes(StandardCharsets.UTF_8).length>2*1024*1024)throw MetadataService.limit();
            if(value.isBlank())throw new SQLException("Package definition is empty; check source privileges","42501");return value;
        }catch(IOException failure){
            try{check(closing);}catch(SQLException cancelled){cancelled.addSuppressed(failure);throw cancelled;}
            throw new SQLException("Cannot read package definition",failure);
        }
    }
    static ObjectNode definitions(ArrayNode documents) throws SQLException {
        ObjectNode result=MetadataService.object();result.set("documents",documents);
        try{if(FrameCodec.JSON.writeValueAsBytes(result).length>3*1024*1024)throw MetadataService.limit();}
        catch(IOException failure){throw new SQLException("Cannot encode package definitions",failure);}
        return result;
    }

    final class Oracle implements PackageProvider {
        private final DatabaseMetaData meta;
        Oracle(DatabaseMetaData meta){this.meta=meta;}
        public String name(){return "oracle";}
        public boolean supported(){return true;}
        public void list(MetadataService.Load load) throws SQLException {
            String schema=owner(meta,load.r);
            try(PreparedStatement statement=prepare(meta.getConnection(),
                "SELECT OBJECT_NAME, STATUS FROM ALL_OBJECTS WHERE OWNER = ? AND OBJECT_TYPE = 'PACKAGE' ORDER BY OBJECT_NAME",schema)){
                load.read("packages",statement::executeQuery,row->{
                    String name=MetadataService.text(row,"OBJECT_NAME");if(name==null)return null;
                    ObjectNode node=load.node(identity("oracle-package",schema,name),name,"PACKAGE",load.r.catalog(),schema,true)
                        .put("qualifiedName",MetadataService.qualified(schema,name));
                    ((ObjectNode)node.path("attributes")).put("packageProvider",name()).put("sourceAvailable","true")
                        .put("ddlAvailable","true").put("status",MetadataService.text(row,"STATUS"));return node;
                });
            }
        }
        public void members(MetadataService.Load load) throws SQLException {
            String schema=owner(meta,load.r),name=load.r.objectName();
            String sql="SELECT p.PROCEDURE_NAME, p.SUBPROGRAM_ID, p.OVERLOAD, "
                +"CASE WHEN EXISTS (SELECT 1 FROM ALL_ARGUMENTS a WHERE a.OWNER = p.OWNER AND a.OBJECT_ID = p.OBJECT_ID "
                +"AND a.SUBPROGRAM_ID = p.SUBPROGRAM_ID AND a.POSITION = 0 AND a.DATA_LEVEL = 0) THEN 'FUNCTION' ELSE 'PROCEDURE' END AS ROUTINE_KIND "
                +"FROM ALL_PROCEDURES p WHERE p.OWNER = ? AND p.OBJECT_NAME = ? AND p.OBJECT_TYPE = 'PACKAGE' "
                +"AND p.PROCEDURE_NAME IS NOT NULL ORDER BY p.PROCEDURE_NAME, p.SUBPROGRAM_ID";
            try(PreparedStatement statement=prepare(meta.getConnection(),sql,schema,name)){
                load.read("package members",statement::executeQuery,row->{
                    String member=MetadataService.text(row,"PROCEDURE_NAME"),subprogram=MetadataService.text(row,"SUBPROGRAM_ID"),overload=MetadataService.text(row,"OVERLOAD");
                    if(member==null)return null;String kind=MetadataService.text(row,"ROUTINE_KIND");
                    ObjectNode node=load.node(identity("oracle-package-member",schema,name,subprogram,member),member,kind,load.r.catalog(),schema,false)
                        .put("qualifiedName",MetadataService.qualified(schema,name,member));
                    ((ObjectNode)node.path("attributes")).put("packageName",name).put("subprogramId",subprogram)
                        .put("overload",overload==null?"":overload).put("packageProvider",name());return node;
                });
            }
        }
        public ObjectNode source(MetadataService.Request request,MetadataService.Job job) throws SQLException {
            String schema=owner(meta,request),name=request.objectName();ArrayNode documents=FrameCodec.JSON.createArrayNode();
            try(PreparedStatement available=prepare(meta.getConnection(),
                "SELECT OBJECT_TYPE FROM ALL_OBJECTS WHERE OWNER = ? AND OBJECT_NAME = ? AND OBJECT_TYPE IN ('PACKAGE', 'PACKAGE BODY') ORDER BY OBJECT_TYPE",schema,name)){
                job.attach(available);
                try(ResultSet types=query(available::executeQuery,job)){
                job.check();
                while(next(types,job)){
                    String type=types.getString("OBJECT_TYPE");
                    String objectType=switch(type){case "PACKAGE"->"PACKAGE_SPEC";case "PACKAGE BODY"->"PACKAGE_BODY";default->throw new SQLException("Unexpected package object type");};
                    try(PreparedStatement statement=prepare(meta.getConnection(),"SELECT DBMS_METADATA.GET_DDL(?, ?, ?) AS DDL FROM DUAL",objectType,name,schema)){
                        job.attach(statement);
                        job.check();
                        try(ResultSet rows=query(statement::executeQuery,job)){
                            if(!next(rows,job))throw new SQLException("Package definition was not returned","02000");
                            String text=definition(rows.getCharacterStream("DDL"),job::stopped);
                            documents.addObject().put("title",MetadataService.qualified(schema,name)+" — "+type).put("kind","DDL").put("text",text);
                        }finally{job.detach(statement);}
                    }
                    if(documents.size()>2)throw new SQLException("Duplicate package object types returned");
                }}finally{job.detach(available);}
            }
            if(documents.isEmpty())throw new SQLException("Package was not found or source is not accessible; refresh the explorer","02000");
            return definitions(documents);
        }
    }

    /** Tibero exposes packages and their stored source through Oracle-compatible views. */
    final class Tibero implements PackageProvider {
        private final DatabaseMetaData meta;
        Tibero(DatabaseMetaData meta){this.meta=meta;}
        public String name(){return "tibero";}
        public boolean supported(){return true;}
        public void list(MetadataService.Load load) throws SQLException {
            String schema=owner(meta,load.r);
            try(PreparedStatement statement=prepare(meta.getConnection(),
                "SELECT OBJECT_NAME, STATUS FROM ALL_OBJECTS WHERE OWNER = ? AND OBJECT_TYPE = 'PACKAGE' ORDER BY OBJECT_NAME",schema)){
                load.read("Tibero packages",statement::executeQuery,row->{
                    String name=MetadataService.text(row,"OBJECT_NAME");if(name==null)return null;
                    ObjectNode node=load.node(identity("tibero-package",schema,name),name,"PACKAGE",load.r.catalog(),schema,true)
                        .put("qualifiedName",MetadataService.qualified(schema,name));
                    ((ObjectNode)node.path("attributes")).put("packageProvider",name()).put("sourceAvailable","true")
                        .put("ddlAvailable","true").put("status",MetadataService.text(row,"STATUS"));return node;
                });
            }
        }
        public void members(MetadataService.Load load) throws SQLException {
            String schema=owner(meta,load.r),packageName=load.r.objectName();
            String sql="SELECT PROCEDURE_NAME, FUNCTIONABLE, MEMBER_NO FROM ALL_PROCEDURES "
                +"WHERE OWNER = ? AND OBJECT_NAME = ? AND PROCEDURE_NAME IS NOT NULL "
                +"ORDER BY PROCEDURE_NAME, MEMBER_NO";
            try(PreparedStatement statement=prepare(meta.getConnection(),sql,schema,packageName)){
                load.read("Tibero package members",statement::executeQuery,row->{
                    String member=MetadataService.text(row,"PROCEDURE_NAME");if(member==null)return null;
                    String functionable=MetadataService.text(row,"FUNCTIONABLE"),memberNo=MetadataService.text(row,"MEMBER_NO");
                    String type="YES".equalsIgnoreCase(functionable)||"Y".equalsIgnoreCase(functionable)?"FUNCTION":"PROCEDURE";
                    ObjectNode node=load.node(identity("tibero-package-member",schema,packageName,member,type,MetadataService.empty(memberNo)),member,type,load.r.catalog(),schema,false)
                        .put("qualifiedName",MetadataService.qualified(schema,packageName,member));
                    ((ObjectNode)node.path("attributes")).put("packageName",packageName).put("packageProvider",name())
                        .put("memberNo",MetadataService.empty(memberNo));return node;
                });
            }
        }
        public ObjectNode source(MetadataService.Request request,MetadataService.Job job) throws SQLException {
            String schema=owner(meta,request),packageName=request.objectName();ArrayNode documents=FrameCodec.JSON.createArrayNode();
            String sql="SELECT TYPE, LINE, TEXT FROM ALL_SOURCE WHERE OWNER = ? AND NAME = ? "
                +"AND TYPE IN ('PACKAGE', 'PACKAGE BODY') ORDER BY TYPE, LINE";
            try(PreparedStatement statement=prepare(job.connection(),sql,schema,packageName)){
                job.attach(statement);
                try(ResultSet rows=query(statement::executeQuery,job)){
                    String currentType=null;StringBuilder text=new StringBuilder();int scanned=0,bytes=0;
                    while(next(rows,job)){
                        if(++scanned>50000)throw MetadataService.limit();
                        String type=MetadataService.text(rows,"TYPE");
                        if(currentType!=null&&!currentType.equals(type))addDocument(documents,schema,packageName,currentType,text);
                        if(!Objects.equals(currentType,type)){currentType=type;text.setLength(0);bytes=0;}
                        String line=rows.getString("TEXT");if(line!=null){text.append(line);bytes+=line.getBytes(StandardCharsets.UTF_8).length;}
                        if(text.length()>2*1024*1024||bytes>2*1024*1024)throw MetadataService.limit();
                    }
                    if(currentType!=null)addDocument(documents,schema,packageName,currentType,text);
                }finally{job.detach(statement);}
            }
            if(documents.isEmpty())throw new SQLException("Package source was not found or is not accessible; refresh the explorer","02000");
            return definitions(documents);
        }
        private void addDocument(ArrayNode documents,String schema,String packageName,String type,StringBuilder text) throws SQLException {
            String kind=switch(type){case "PACKAGE"->"PACKAGE";case "PACKAGE BODY"->"PACKAGE BODY";default->throw new SQLException("Unexpected Tibero package source type");};
            String value=text.toString();if(value.isBlank())return;
            documents.addObject().put("title",MetadataService.qualified(schema,packageName)+" — "+kind).put("kind","DDL").put("text",value);
            if(documents.size()>2)throw new SQLException("Duplicate package source types returned");
        }
    }


    /** Db2 LUW bound SQL packages contain static statements, not routine DDL. */
    final class Db2 implements PackageProvider {
        private final DatabaseMetaData meta;
        Db2(DatabaseMetaData meta){this.meta=meta;}
        public String name(){return "db2-luw";}
        public boolean supported(){return true;}
        public boolean ddl(){return false;}
        public boolean membersSupported(){return false;}
        public boolean sourceSupported(){return true;}
        public void list(MetadataService.Load load) throws SQLException {
            String schema=owner(meta,load.r);
            String sql="SELECT PKGNAME, PKGVERSION, HEX(UNIQUE_ID) AS PACKAGE_ID, VALID, BOUNDBY "
                +"FROM SYSCAT.PACKAGES WHERE PKGSCHEMA = ? ORDER BY PKGNAME, PKGVERSION, HEX(UNIQUE_ID)";
            try(PreparedStatement statement=prepare(meta.getConnection(),sql,schema)){
                load.read("Db2 packages",statement::executeQuery,row->{
                    String name=MetadataService.text(row,"PKGNAME"),version=MetadataService.text(row,"PKGVERSION"),uniqueId=MetadataService.text(row,"PACKAGE_ID");
                    if(name==null||uniqueId==null)return null;
                    String id=PackageProvider.identity("db2-package",schema,name,version,uniqueId);
                    ObjectNode node=load.node(id,name,"PACKAGE",load.r.catalog(),schema,false)
                        .put("qualifiedName",MetadataService.qualified(schema,name));
                    ((ObjectNode)node.path("attributes")).put("packageProvider",name()).put("sourceAvailable","true")
                        .put("ddlAvailable","false").put("packageVersion",version==null?"":version)
                        .put("packageUniqueId",uniqueId).put("valid",MetadataService.text(row,"VALID"))
                        .put("boundBy",MetadataService.text(row,"BOUNDBY"));return node;
                });
            }
        }
        public void members(MetadataService.Load load){load.unsupported("bound Db2 SQL package routine members");}
        public ObjectNode source(MetadataService.Request request,MetadataService.Job job) throws SQLException {
            PackageIdentity identity=identity(request);
            // SYSCAT.PACKAGES.PKGVERSION uses an empty string for an unversioned
            // package; SYSCAT.STATEMENTS.VERSION can represent it as NULL.
            boolean unversioned=identity.version()==null||identity.version().isEmpty();
            String versionPredicate=unversioned?"(VERSION IS NULL OR VERSION = '')":"VERSION = ?";
            String sql="SELECT STMTNO, SECTNO, TEXT FROM SYSCAT.STATEMENTS WHERE PKGSCHEMA = ? AND PKGNAME = ? "
                +"AND HEX(UNIQUE_ID) = ? AND "+versionPredicate+" ORDER BY STMTNO, SECTNO, SEQNO";
            ArrayNode documents=FrameCodec.JSON.createArrayNode();
            try(PreparedStatement statement=unversioned
                    ?prepare(meta.getConnection(),sql,identity.schema(),identity.name(),identity.uniqueId())
                    :prepare(meta.getConnection(),sql,identity.schema(),identity.name(),identity.uniqueId(),identity.version())){
                job.attach(statement);
                try(ResultSet rows=query(statement::executeQuery,job)){
                    while(next(rows,job)){
                        String text=definition(rows.getCharacterStream("TEXT"),job::stopped);
                        String statementNo=MetadataService.text(rows,"STMTNO"),sectionNo=MetadataService.text(rows,"SECTNO");
                        String title=MetadataService.qualified(identity.schema(),identity.name())+" — "+(unversioned?"(no version)":identity.version())+" — statement "+statementNo+" / section "+sectionNo;
                        documents.addObject().put("title",title).put("kind","SQL_SOURCE").put("text",text);
                        if(documents.size()>10000)throw MetadataService.limit();
                    }
                }finally{job.detach(statement);}
            }
            if(documents.isEmpty())throw new SQLException("Db2 package has no accessible SQL statement text","02000");
            return definitions(documents);
        }
        private static PackageIdentity identity(MetadataService.Request request) throws SQLException {
            if(request.objectId()==null)throw new SQLException("Db2 package version identity is required; refresh the explorer","22023");
            try{
                var value=FrameCodec.JSON.readTree(request.objectId());
                if(value==null||!value.isArray()||value.size()!=5||!"db2-package".equals(value.get(0).asText()))throw new SQLException("Invalid Db2 package identity","22023");
                String schema=value.get(1).asText(),name=value.get(2).asText(),version=value.get(3).isNull()?null:value.get(3).asText(),uniqueId=value.get(4).asText();
                if(!Objects.equals(request.schema(),schema)||!Objects.equals(request.objectName(),name)||!uniqueId.matches("[0-9A-Fa-f]{16}"))throw new SQLException("Db2 package identity does not match the selected object","22023");
                return new PackageIdentity(schema,name,version,uniqueId.toUpperCase(Locale.ROOT));
            }catch(IOException failure){throw new SQLException("Invalid Db2 package identity","22023",failure);}
        }
        private record PackageIdentity(String schema,String name,String version,String uniqueId){}
    }

    final class MariaDb implements PackageProvider {
        private final DatabaseMetaData meta;
        private final boolean enabled;
        private MariaDb(DatabaseMetaData meta,boolean enabled){this.meta=meta;this.enabled=enabled;}
        static MariaDb detect(DatabaseMetaData meta) throws SQLException {
            int major=meta.getDatabaseMajorVersion(),minor=meta.getDatabaseMinorVersion();
            if(major>11||major==11&&minor>=4)return new MariaDb(meta,true);
            try(Statement statement=meta.getConnection().createStatement();ResultSet rows=statement.executeQuery("SELECT @@sql_mode")){
                if(rows.next())for(String mode:rows.getString(1).split(","))if(mode.trim().equalsIgnoreCase("ORACLE"))return new MariaDb(meta,true);
            }catch(SQLFeatureNotSupportedException|AbstractMethodError unsupported){}
            return new MariaDb(meta,false);
        }
        public String name(){return "mariadb";}
        public boolean supported(){return enabled;}
        public boolean ddl(){return enabled;}
        public void list(MetadataService.Load load) throws SQLException {
            if(!enabled){load.unsupported("MariaDB packages require Oracle SQL mode before version 11.4");return;}
            String database=database(meta,load.r);
            String sql="SHOW PACKAGE STATUS";
            try(Statement statement=meta.getConnection().createStatement()){
                try{statement.setQueryTimeout(15);}catch(SQLFeatureNotSupportedException ignored){}
                try(ResultSet rows=statement.executeQuery(sql)){
                while(rows.next()){
                    load.check();if(++load.scanned>50000)throw MetadataService.limit();
                    String owner=MetadataService.text(rows,"Db"),name=MetadataService.text(rows,"Name");
                    if(!database.equals(owner)||name==null)continue;
                    ObjectNode node=load.node(identity("mariadb-package",database,name),name,"PACKAGE",database,database,true)
                        .put("qualifiedName",MetadataService.qualified(database,name));
                    ((ObjectNode)node.path("attributes")).put("packageProvider",name()).put("sourceAvailable","true")
                        .put("ddlAvailable","true").put("status",MetadataService.text(rows,"Type"));
                    load.add(node);
                }
                }
            }catch(SQLFeatureNotSupportedException|UnsupportedOperationException unsupported){load.unsupported("MariaDB package discovery");}
        }
        public void members(MetadataService.Load load) throws SQLException {
            if(!enabled){load.unsupported("MariaDB package members require Oracle SQL mode before version 11.4");return;}
            String database=database(meta,load.r),packageName=load.r.objectName();
            String ddl=show(load,"SHOW CREATE PACKAGE "+quote(database)+"."+quote(packageName),"Create Package");
            List<Routine> routines=declarations(ddl);
            if(routines.isEmpty()){load.unsupported("MariaDB package member declarations");return;}
            int ordinal=0;
            for(Routine routine:routines){
                String id=identity("mariadb-package-member",database,packageName,routine.kind(),routine.name(),Integer.toString(ordinal++));
                ObjectNode node=load.node(id,routine.name(),routine.kind(),database,database,false)
                    .put("qualifiedName",MetadataService.qualified(database,packageName,routine.name()));
                ((ObjectNode)node.path("attributes")).put("packageName",packageName).put("packageProvider",name());
                load.add(node);
            }
        }
        public ObjectNode source(MetadataService.Request request,MetadataService.Job job) throws SQLException {
            if(!enabled)throw new SQLFeatureNotSupportedException("MariaDB package source requires Oracle SQL mode before version 11.4","0A000");
            String database=database(meta,request),packageName=request.objectName();
            ArrayNode documents=FrameCodec.JSON.createArrayNode();
            String specification=show(job,"SHOW CREATE PACKAGE "+quote(database)+"."+quote(packageName),"Create Package");
            documents.addObject().put("title",MetadataService.qualified(database,packageName)+" — PACKAGE").put("kind","DDL").put("text",specification);
            if(hasBody(job,database,packageName)){
                String body=show(job,"SHOW CREATE PACKAGE BODY "+quote(database)+"."+quote(packageName),"Create Package Body");
                documents.addObject().put("title",MetadataService.qualified(database,packageName)+" — PACKAGE BODY").put("kind","DDL").put("text",body);
            }
            return definitions(documents);
        }
        private static String database(DatabaseMetaData meta,MetadataService.Request request) throws SQLException {
            if(request.schema()!=null&&!request.schema().isEmpty())return request.schema();
            if(request.catalog()!=null&&!request.catalog().isEmpty())return request.catalog();
            String current=meta.getConnection().getCatalog();
            if(current==null||current.isEmpty())throw new SQLException("Select a database before loading packages","3F000");
            return current;
        }
        private static String quote(String value){return "`"+value.replace("`","``")+"`";}
        private static boolean hasBody(MetadataService.Job job,String database,String packageName) throws SQLException {
            Statement statement=null;
            try{
                Connection connection=job.connection();
                statement=connection.createStatement();job.attach(statement);job.check();
                try{statement.setQueryTimeout(15);}catch(SQLFeatureNotSupportedException ignored){}
                Statement active=statement;
                try(ResultSet rows=query(()->active.executeQuery("SHOW PACKAGE BODY STATUS"),job)){
                    int scanned=0;while(next(rows,job)){
                        if(++scanned>50000)throw MetadataService.limit();
                        if(database.equals(rows.getString("Db"))&&packageName.equals(rows.getString("Name")))return true;
                    }
                    return false;
                }
            }finally{if(statement!=null)job.detach(statement);if(statement!=null)try{statement.close();}catch(SQLException ignored){}}
        }
        private static String show(MetadataService.Job job,String sql,String column) throws SQLException {
            Statement statement=null;
            try{
                Connection connection=job.connection();statement=connection.createStatement();
                try{statement.setQueryTimeout(15);}catch(SQLFeatureNotSupportedException ignored){}
                job.attach(statement);job.check();Statement active=statement;
                try(ResultSet rows=query(()->active.executeQuery(sql),job)){
                    if(!next(rows,job))throw new SQLException("Package definition was not returned","02000");
                    return definition(rows.getCharacterStream(column),job::stopped);
                }
            }finally{if(statement!=null)job.detach(statement);if(statement!=null)try{statement.close();}catch(SQLException ignored){}}
        }
        private static String show(MetadataService.Load load,String sql,String column) throws SQLException {
            try(Statement statement=load.meta.getConnection().createStatement();ResultSet rows=statement.executeQuery(sql)){
                load.check();if(!rows.next())throw new SQLException("Package definition was not returned","02000");
                String value=definition(rows.getCharacterStream(column),load.closing);
                if(value.isBlank())throw new SQLException("Package definition is unavailable","42501");return value;
            }
        }
        record Routine(String kind,String name){}
        private record Token(String text,boolean identifier){}
        static List<Routine> declarations(String ddl) throws SQLException {
            if(ddl==null||ddl.isBlank())throw new SQLException("Package specification is unavailable","42501");
            if(ddl.getBytes(StandardCharsets.UTF_8).length>2*1024*1024)throw MetadataService.limit();
            List<Token> tokens=lex(ddl);List<Routine> routines=new ArrayList<>();int depth=0;
            for(int index=0;index<tokens.size();index++){
                Token token=tokens.get(index);
                if(token.text().equals("(")){depth++;continue;}if(token.text().equals(")")){depth=Math.max(0,depth-1);continue;}
                if(depth==0&&(token.text().equalsIgnoreCase("FUNCTION")||token.text().equalsIgnoreCase("PROCEDURE"))&&index+1<tokens.size()){
                    Token name=tokens.get(index+1);if(!name.identifier())continue;
                    routines.add(new Routine(token.text().toUpperCase(Locale.ROOT),name.text()));
                    if(routines.size()>10000)throw MetadataService.limit();
                }
            }
            return routines;
        }
        private static List<Token> lex(String input){
            List<Token> tokens=new ArrayList<>();int index=0;
            while(index<input.length()){
                char c=input.charAt(index);
                if(Character.isWhitespace(c)){index++;continue;}
                if(c=='#'){while(index<input.length()&&input.charAt(index)!='\n')index++;continue;}
                if(c=='-'&&index+1<input.length()&&input.charAt(index+1)=='-'){index+=2;while(index<input.length()&&input.charAt(index)!='\n')index++;continue;}
                if(c=='/'&&index+1<input.length()&&input.charAt(index+1)=='*'){index+=2;while(index+1<input.length()&&!(input.charAt(index)=='*'&&input.charAt(index+1)=='/'))index++;index=Math.min(input.length(),index+2);continue;}
                if(c=='\'' ){index++;while(index<input.length()){if(input.charAt(index++)=='\'' ){if(index<input.length()&&input.charAt(index)=='\''){index++;continue;}break;}if(input.charAt(index-1)=='\\'&&index<input.length())index++;}tokens.add(new Token("STRING",false));continue;}
                if(c=='`'||c=='"'){char end=c;index++;StringBuilder value=new StringBuilder();while(index<input.length()){char next=input.charAt(index++);if(next==end){if(index<input.length()&&input.charAt(index)==end){value.append(end);index++;continue;}break;}if(next=='\\'&&index<input.length())next=input.charAt(index++);value.append(next);}tokens.add(new Token(value.toString(),true));continue;}
                if(Character.isJavaIdentifierStart(c)||c=='$'){int start=index++;while(index<input.length()&&(Character.isJavaIdentifierPart(input.charAt(index))||input.charAt(index)=='$'))index++;tokens.add(new Token(input.substring(start,index),true));continue;}
                tokens.add(new Token(Character.toString(c),false));index++;
            }
            return tokens;
        }
    }
}
