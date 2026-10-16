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
            text=text.replace("\r\n","\n").replace('\r','\n');
            int[] points=text.codePoints().limit(65537).toArray();StringBuilder clean=new StringBuilder();
            for(int index=0;index<Math.min(points.length,65536);index++){int point=points[index];clean.appendCodePoint(Character.isISOControl(point)&&point!='\n'?' ':point);}
            if(points.length>65536)clean.append("\n[message truncated after 65536 characters]");return clean.toString();
        }
        public synchronized void clear(){secrets.clear();}
    }
    public static SQLException find(Throwable failure){
        Set<Throwable> seen=Collections.newSetFromMap(new IdentityHashMap<>());for(int depth=0;failure!=null&&depth<32&&seen.add(failure);depth++,failure=failure.getCause())if(failure instanceof SQLException sql)return sql;return null;
    }
    public static ObjectNode wire(Throwable failure,Redactor redactor){
        SQLException sql=find(failure);ObjectNode result=FrameCodec.JSON.createObjectNode().put("message",redactor.text(sql==null?failure.getMessage():sql.getMessage())).put("vendorCode",sql==null?0:sql.getErrorCode());
        if(sql==null||sql.getSQLState()==null)result.putNull("sqlState");else result.put("sqlState",redactor.text(sql.getSQLState()));
        var chained=result.putArray("chained");Set<Throwable> seen=Collections.newSetFromMap(new IdentityHashMap<>());
        Throwable root=sql==null?failure:sql;seen.add(root);ArrayDeque<Throwable> pending=new ArrayDeque<>();
        if(root.getCause()!=null)pending.add(root.getCause());
        if(sql!=null&&sql.getNextException()!=null)pending.addFirst(sql.getNextException());
        if(failure!=root)pending.addLast(failure);
        while(!pending.isEmpty()&&chained.size()<16){
            Throwable next=pending.removeFirst();if(!seen.add(next))continue;
            String message=next instanceof SQLException?next.getMessage():next.getClass().getSimpleName()+": "+next.getMessage();
            ObjectNode item=chained.addObject().put("message",redactor.text(message)).put("vendorCode",next instanceof SQLException value?value.getErrorCode():0);
            if(next instanceof SQLException value&&value.getSQLState()!=null)item.put("sqlState",redactor.text(value.getSQLState()));else item.putNull("sqlState");item.putArray("chained");
            if(next instanceof SQLException value&&value.getNextException()!=null)pending.addFirst(value.getNextException());
            if(next.getCause()!=null)pending.addLast(next.getCause());
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
