package kode.jdbc;
import kode.jdbc.protocol.FrameCodec;
import kode.jdbc.protocol.RpcServer;
/** Test-only peer, using the actual production stdout bootstrap. */
public class StdoutProbeMain {
    public static void main(String[] args) throws Exception {
        try(RpcServer server=Main.createServer()) {
            server.register("fake.stdout",false,(params,context)->{
                System.out.println("synthetic-driver-stdout-marker");
                return RpcServer.result(FrameCodec.JSON.createObjectNode().put("ok",true));
            });
            System.exit(server.serve());
        }
    }
}
