import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Pattern;

/** Synthetic Java 21 regex oracle; no access to project/user documents. */
class JavaSearchFixture {
    static String quote(String text) {
        var out = new StringBuilder("\"");
        for (int i = 0; i < text.length(); i++) {
            char ch = text.charAt(i);
            if (ch == '"' || ch == '\\') out.append('\\').append(ch);
            else if (ch < 32) out.append(String.format("\\u%04X", (int) ch));
            else out.append(ch);
        }
        return out.append('"').toString();
    }
    public static void main(String[] args) throws Exception {
        if (Runtime.version().feature() != 21) throw new IllegalStateException("Oracle requires Java 21");
        String[][] cases = {
            {"a\r\nb\rC\u0085D\u2028E\u2029F\n", "(?m)^.$"},
            {"abc\r\n", "abc$"}, {"abc\r\nnext", "abc$"},
            {"a\rb\n", "(?d)."}, {"a\r\nb", "(?s)a..b"},
            {"a\r\nb", "\\R"}, {"a\r\nb", "a\\Z"},
            {"aA bB", "(?i:a)b"}, {"aAb aAB", "(?i:aa)b"},
            {"kK\u212A", "(?i)k"}, {"kK\u212A", "(?iu)k"},
            {"aAzZ", "(?i)[a-z]+"}, {"ab中_12𝟚", "\\w+"},
            {"1𝟚", "\\d+"}, {"1𝟚", "(?U)\\d+"},
            {"aé1", "\\p{Alpha}+"}, {"aé1", "\\p{L}+"},
            {"x A.*B x", "\\QA.*B\\E"}, {"😀A", "\\x{1F600}\\u0041"},
            {"abab abac", "(ab)\\1"}, {"abab abac", "(?<word>ab)\\k<word>"},
            {"one two oneone", "(?<=one)one"}, {"foo bar foo", "foo(?= bar)"},
            {"abcde", "[a-z&&[^bc]]"}, {"a\tb c", "\\h"},
            {"é aé aéa", "\\b[a-z]+\\b"},
            {"ab", "a*"}, {"bbb", "(?:)"}, {"a\nb", "a\\nb"}
        };
        var out = new StringBuilder("[\n");
        boolean first = true;
        for (var item : cases) {
            if (!first) out.append(",\n"); first = false;
            out.append("{\"text\":").append(quote(item[0])).append(",\"query\":").append(quote(item[1])).append(",\"ranges_utf16\":[");
            var matcher = Pattern.compile(item[1]).matcher(item[0]);
            boolean firstRange = true;
            while (matcher.find()) {
                if (matcher.start() == matcher.end()) continue; // Kotlin visible search convention
                if (!firstRange) out.append(','); firstRange = false;
                out.append('[').append(matcher.start()).append(',').append(matcher.end()).append(']');
            }
            out.append("]}");
        }
        out.append("\n]\n");
        Files.writeString(Path.of(args[0]), out, StandardCharsets.UTF_8);
    }
}
