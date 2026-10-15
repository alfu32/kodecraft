package kode.jdbc;

import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import kode.jdbc.protocol.RpcServer;

public final class Main {
    public static void main(String[] args) {
        // Retain the actual stdout solely for framed protocol bytes, before any
        // JSON library or driver can initialize and print ordinary System.out.
        var protocol = new FileOutputStream(FileDescriptor.out);
        System.setOut(new PrintStream(System.err,true,StandardCharsets.UTF_8));
        int status = 2;
        try (var server = new RpcServer(System.in,protocol)) {
            status = server.serve();
        } catch (Exception failure) {
            System.err.println("JDBC helper stopped ("+failure.getClass().getSimpleName()+")");
        }
        // JDBC libraries may retain non-daemon threads. Bounded parent shutdown
        // still supervises/reaps this process and any descendants.
        System.exit(status);
    }
}
