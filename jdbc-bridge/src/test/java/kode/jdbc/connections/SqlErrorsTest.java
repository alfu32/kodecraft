package kode.jdbc.connections;
import java.sql.SQLException;
import java.io.IOException;
import java.util.List;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class SqlErrorsTest {
    @Test void multilineTailAndCausesRemainReadableAndSecretsStayRedacted() {
        var redactor=new SqlErrors.Redactor(List.of("synthetic-private-password"));
        var failure=new SQLException("first line\r\n"+"long diagnostic ".repeat(400)+"\nTAIL password=synthetic-private-password","42000",73);
        failure.setNextException(new SQLException("next JDBC error","HY000",84));
        failure.initCause(new IOException("TLS detail token=synthetic-private-password"));
        var wire=SqlErrors.wire(failure,redactor);
        String message=wire.path("message").asText();
        assertTrue(message.startsWith("first line\n"));assertTrue(message.contains("TAIL"));assertEquals("42000",wire.path("sqlState").asText());
        assertEquals(2,wire.path("chained").size());assertEquals(84,wire.path("chained").get(0).path("vendorCode").asInt());
        assertTrue(wire.toString().contains("TLS detail"));assertFalse(wire.toString().contains("synthetic-private-password"));
    }
    @Test void oversizedMessagesAndCausalCyclesAreBoundedAndMarked() {
        var redactor=new SqlErrors.Redactor(List.of());
        assertTrue(redactor.text("x".repeat(70000)).endsWith("[message truncated after 65536 characters]"));
        var a=new SQLException("a");var b=new SQLException("b");a.initCause(b);b.initCause(a);
        assertEquals(1,SqlErrors.wire(a,redactor).path("chained").size());
        assertFalse(redactor.text("escape\u001b[31m\nline").contains("\u001b"));
    }
}
