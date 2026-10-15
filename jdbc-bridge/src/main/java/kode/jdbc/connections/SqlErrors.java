package kode.jdbc.connections;

import com.fasterxml.jackson.databind.node.ObjectNode;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import java.sql.SQLException;
import java.util.*;

/** Wire-safe errors. Never print JDBC exception/cause chains or request bodies. */
public final class SqlErrors {
    public static final int SQL_FAILURE=-32020,STALE_CONNECTION=-32021,DEFINITION_CHANGED=-32022,UNSUPPORTED=-32023,NEEDS_RESTART=-32024;
    public static final class Redactor {
        private final List<String> secrets;
        public Redactor(Collection<String> secrets){this.secrets=new ArrayList<>(secrets.stream().filter(value->value!=null&&!value.isEmpty()).distinct().sorted(Comparator.comparingInt(String::length).reversed()).toList());}
        public synchronized String text(String value){
            String text=value==null?"Database operation failed":value;
            for(String secret:secrets)text=text.replace(secret,"<redacted>");
            text=text.replaceAll("(?i)(password|pwd|token|secret|authorization|api[_-]?key)(\\s*[=:]\\s*)[^\\s;&,]+","$1$2<redacted>");
            StringBuilder clean=new StringBuilder();text.codePoints().limit(2048).forEach(point->clean.appendCodePoint(Character.isISOControl(point)?' ':point));return clean.toString();
        }
        public synchronized void clear(){secrets.clear();}
    }
    public static SQLException find(Throwable failure){
        Set<Throwable> seen=Collections.newSetFromMap(new IdentityHashMap<>());for(int depth=0;failure!=null&&depth<32&&seen.add(failure);depth++,failure=failure.getCause())if(failure instanceof SQLException sql)return sql;return null;
    }
    public static ObjectNode wire(Throwable failure,Redactor redactor){
        SQLException sql=find(failure);ObjectNode result=FrameCodec.JSON.createObjectNode().put("message",redactor.text(sql==null?failure.getMessage():sql.getMessage())).put("vendorCode",sql==null?0:sql.getErrorCode());
        if(sql==null||sql.getSQLState()==null)result.putNull("sqlState");else result.put("sqlState",redactor.text(sql.getSQLState()));
        var chained=result.putArray("chained");Set<SQLException> seen=Collections.newSetFromMap(new IdentityHashMap<>());if(sql!=null)seen.add(sql);
        SQLException next=sql==null?null:sql.getNextException();for(int count=0;next!=null&&count<16&&seen.add(next);count++,next=next.getNextException()){
            ObjectNode item=chained.addObject().put("message",redactor.text(next.getMessage())).put("vendorCode",next.getErrorCode());if(next.getSQLState()==null)item.putNull("sqlState");else item.put("sqlState",redactor.text(next.getSQLState()));item.putArray("chained");
        }
        return result;
    }
    public static RpcException error(Throwable failure,Redactor redactor,ObjectNode state,boolean notStarted){
        if(failure instanceof RpcException rpc)return rpc;
        ObjectNode data=FrameCodec.JSON.createObjectNode();data.set("sqlError",wire(failure,redactor));data.put("notStarted",notStarted);if(state!=null)data.set("state",state);
        return new RpcException(SQL_FAILURE,"Database operation failed",data);
    }
    private SqlErrors(){}
}
