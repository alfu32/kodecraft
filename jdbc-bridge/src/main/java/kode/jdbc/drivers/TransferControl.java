package kode.jdbc.drivers;

import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcException;
import kode.jdbc.protocol.RpcServer;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.BooleanSupplier;

/** Cancellation acknowledgement is independent from completion and cleanup. */
public final class TransferControl {
    private final AtomicInteger reason=new AtomicInteger();
    private final AtomicReference<Runnable> abort=new AtomicReference<>();
    private final BooleanSupplier stopping;
    private final String id;
    private final RpcServer.Context context;
    private volatile Thread worker;
    private long lastProgress;
    private String lastPhase="";
    public TransferControl(String id,BooleanSupplier stopping,RpcServer.Context context){this.id=id;this.stopping=stopping;this.context=context;}
    public void attach(){worker=Thread.currentThread();}
    public void detach(){worker=null;Thread.interrupted();}
    public void cancel(){cancel(1);}
    public void timeout(){cancel(2);}
    private void cancel(int cause){if(!reason.compareAndSet(0,cause))return;Thread thread=worker;if(thread!=null)thread.interrupt();Runnable action=abort.getAndSet(null);if(action!=null)Thread.startVirtualThread(action);}
    public boolean cancelled(){return reason.get()!=0||stopping.getAsBoolean()||Thread.currentThread().isInterrupted();}
    public void check() throws RpcException {if(reason.get()==2)throw new RpcException(-32010,"Driver download timed out");if(cancelled())throw new RpcException(-32800,"Driver resolution cancelled");}
    public AutoCloseable abortOnCancel(Runnable action){abort.set(action);if(reason.get()!=0){Runnable pending=abort.getAndSet(null);if(pending!=null)Thread.startVirtualThread(pending);}return ()->abort.compareAndSet(action,null);}
    public synchronized void progress(String phase,long completed,long total) throws Exception {
        check();long now=System.nanoTime();if(phase.equals(lastPhase)&&now-lastProgress<200_000_000)return;
        lastProgress=now;lastPhase=phase;
        if(context!=null)context.notify("driver.progress",FrameCodec.JSON.createObjectNode().put("resolutionId",id).put("phase",phase).put("completedBytes",completed).put("totalBytes",total),this::cancelled);
    }
}
