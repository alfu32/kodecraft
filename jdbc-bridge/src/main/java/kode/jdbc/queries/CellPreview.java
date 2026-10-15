package kode.jdbc.queries;

import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.protocol.FrameCodec;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.sql.*;
import java.util.Base64;

/** Bounded previews; large JDBC values are streamed and freed on their owner. */
public final class CellPreview {
    public static final int MAX_CELL_BYTES=64*1024,MAX_ROW_BYTES=512*1024,MAX_COLUMNS=256;
    public record Text(String text,boolean truncated) {}
    static SQLException limit(String message){return new SQLException(message,"54000");}
    public static String metadata(String value) throws SQLException {
        if(value==null)return "";if(value.getBytes(StandardCharsets.UTF_8).length>1024)throw limit("Column label or type name exceeds 1 KiB");return value;
    }
    /** Count UTF-8 bytes and never split a scalar. Invalid driver UTF-16 becomes
     * U+FFFD so it cannot poison the entire framed protocol. One scalar lookahead
     * distinguishes an exactly full value from a truncated preview. */
    public static Text text(Reader input,int limit) throws IOException {
        StringBuilder result=new StringBuilder(Math.min(limit,4096));int bytes=0,pending=-1;
        try(Reader reader=input instanceof BufferedReader?input:new BufferedReader(input,4096)){
            while(true){
                int first=pending>=0?pending:reader.read();pending=-1;if(first<0)return new Text(result.toString(),false);int point=first;
                if(Character.isHighSurrogate((char)first)){int second=reader.read();if(second>=0&&Character.isLowSurrogate((char)second))point=Character.toCodePoint((char)first,(char)second);else{point=0xfffd;pending=second;}}
                else if(Character.isLowSurrogate((char)first))point=0xfffd;
                int size=point<=0x7f?1:point<=0x7ff?2:point<=0xffff?3:4;
                if(bytes+size>limit)return new Text(result.toString(),true);result.appendCodePoint(point);bytes+=size;
            }
        }
    }
    private static ObjectNode nil(){return FrameCodec.JSON.createObjectNode().put("kind","null");}
    private static ObjectNode textCell(Text value){return FrameCodec.JSON.createObjectNode().put("kind","text").put("text",value.text()).put("truncated",value.truncated());}
    private static ObjectNode textCell(String value,int limit) throws IOException {return value==null?nil():textCell(text(new StringReader(value),limit));}
    private static ObjectNode binary(InputStream input,int limit) throws IOException {
        if(input==null)return nil();byte[] bytes;try(input){bytes=input.readNBytes(limit+1);}boolean truncated=bytes.length>limit;
        return FrameCodec.JSON.createObjectNode().put("kind","binary").put("base64",Base64.getEncoder().encodeToString(truncated?java.util.Arrays.copyOf(bytes,limit):bytes)).put("truncated",truncated);
    }
    public static ObjectNode cell(ResultSet rows,int column,int jdbcType,int limit) throws Exception {
        return switch(jdbcType){
            case Types.BINARY,Types.VARBINARY,Types.LONGVARBINARY -> binary(rows.getBinaryStream(column),limit);
            case Types.BLOB -> {Blob blob=rows.getBlob(column);if(blob==null)yield nil();try{yield binary(blob.getBinaryStream(),limit);}finally{blob.free();}}
            case Types.CLOB,Types.NCLOB -> {Clob clob=jdbcType==Types.NCLOB?rows.getNClob(column):rows.getClob(column);if(clob==null)yield nil();try{yield textCell(text(clob.getCharacterStream(),limit));}finally{clob.free();}}
            case Types.CHAR,Types.VARCHAR,Types.LONGVARCHAR,Types.NCHAR,Types.NVARCHAR,Types.LONGNVARCHAR -> {Reader reader=rows.getCharacterStream(column);yield reader==null?nil():textCell(text(reader,limit));}
            case Types.SQLXML -> {SQLXML xml=rows.getSQLXML(column);if(xml==null)yield nil();try{yield textCell(text(xml.getCharacterStream(),limit));}finally{xml.free();}}
            case Types.ARRAY -> {java.sql.Array array=rows.getArray(column);if(array==null)yield nil();try{yield array(array,limit);}finally{array.free();}}
            default -> {
                Object value=rows.getObject(column);if(value==null||rows.wasNull())yield nil();
                // Preserve source Boolean/decimal/temporal toString semantics,
                // without numeric JSON coercion or unbounded binary previews.
                if(value instanceof byte[] bytes)yield binary(new ByteArrayInputStream(bytes),limit);
                if(value instanceof Blob blob)try{yield binary(blob.getBinaryStream(),limit);}finally{blob.free();}
                if(value instanceof Clob clob)try{yield textCell(text(clob.getCharacterStream(),limit));}finally{clob.free();}
                if(value instanceof SQLXML xml)try{yield textCell(text(xml.getCharacterStream(),limit));}finally{xml.free();}
                if(value instanceof java.sql.Array array)try{yield array(array,limit);}finally{array.free();}
                yield textCell(value.toString(),limit);
            }
        };
    }
    private static ObjectNode array(java.sql.Array array,int limit) throws Exception {
        // Do not materialize getArray(). Use a bounded textual element preview;
        // nested/vendor element representations remain driver-provided strings.
        StringBuilder value=new StringBuilder("[");boolean truncated=false;int count=0;
        try(ResultSet elements=array.getResultSet()){
            while(elements.next()){
                if(count++>=32){truncated=true;break;}if(count>1)value.append(", ");
                Reader reader=elements.getCharacterStream(2);
                if(reader==null)value.append("NULL");else{Text item=text(reader,Math.min(1024,limit));value.append(item.text());truncated|=item.truncated();}
                if(value.length()>limit){truncated=true;break;}
            }
        }
        value.append(']');Text preview=text(new StringReader(value.toString()),limit);return textCell(new Text(preview.text(),truncated||preview.truncated()));
    }
    private CellPreview(){}
}
