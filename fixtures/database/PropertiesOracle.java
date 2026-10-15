import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.util.Base64;
import java.util.Properties;
import java.util.TreeSet;
/** Optional source-compatibility oracle. The argument is synthetic fixture data. */
public class PropertiesOracle {
    public static void main(String[] args) throws Exception {
        Properties properties = new Properties();
        properties.load(new ByteArrayInputStream(Base64.getDecoder().decode(args[0])));
        for (String key : new TreeSet<>(properties.stringPropertyNames())) {
            System.out.println(Base64.getEncoder().encodeToString(key.getBytes(StandardCharsets.UTF_8)) + "\t" +
                Base64.getEncoder().encodeToString(properties.getProperty(key).getBytes(StandardCharsets.UTF_8)));
        }
    }
}
