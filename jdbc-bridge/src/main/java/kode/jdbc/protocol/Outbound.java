package kode.jdbc.protocol;

import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.DataOutputStream;
import java.io.IOException;
import java.util.ArrayDeque;
import java.util.function.BooleanSupplier;

/** Bounded byte/count queues, one writer, control priority, FIFO ordinary data. */
final class Outbound implements AutoCloseable {
    private record Item(byte[] bytes, boolean control) {}
    private final ArrayDeque<Item> control = new ArrayDeque<>(), ordinary = new ArrayDeque<>();
    private final DataOutputStream output;
    private final Runnable failed;
    private final Thread writer;
    private int controlBytes, ordinaryBytes;
    private boolean closing;

    Outbound(DataOutputStream output, Runnable failed) {
        this.output = output; this.failed = failed;
        writer = Thread.ofPlatform().daemon(true).name("jdbc-protocol-writer").unstarted(this::writeLoop);
    }
    void start() { writer.start(); }
    boolean send(ObjectNode value, boolean priority, BooleanSupplier cancelled) throws IOException, InterruptedException {
        byte[] bytes = FrameCodec.encode(value);
        synchronized (this) {
            while (!closing && !cancelled.getAsBoolean()) {
                var queue = priority ? control : ordinary;
                int used = priority ? controlBytes : ordinaryBytes;
                int budget = priority ? 1024 * 1024 : 32 * 1024 * 1024;
                int count = priority ? 8 : 32;
                if (bytes.length > budget) throw new FrameCodec.ProtocolException("Control frame exceeds reserved byte budget");
                if (queue.size() < count && used + bytes.length <= budget) {
                    queue.add(new Item(bytes, priority));
                    if (priority) controlBytes += bytes.length; else ordinaryBytes += bytes.length;
                    notifyAll(); return true;
                }
                // Input/control dispatch never waits behind a blocked output pipe.
                if (priority) { failed.run(); return false; }
                wait(10);
            }
        }
        return false;
    }
    private void writeLoop() {
        try {
            while (true) {
                Item item;
                synchronized (this) {
                    while (control.isEmpty() && ordinary.isEmpty() && !closing) wait();
                    if (control.isEmpty() && ordinary.isEmpty()) return;
                    item = !control.isEmpty() ? control.remove() : ordinary.remove();
                }
                try { FrameCodec.write(output, item.bytes()); }
                finally {
                    synchronized (this) {
                        if (item.control()) controlBytes -= item.bytes().length; else ordinaryBytes -= item.bytes().length;
                        notifyAll();
                    }
                }
            }
        } catch (Exception failure) { failed.run(); }
    }
    @Override public void close() {
        synchronized (this) { closing = true; notifyAll(); }
        try { writer.join(1000); }
        catch (InterruptedException failure) { Thread.currentThread().interrupt(); }
        if (writer.isAlive()) failed.run();
    }
}
