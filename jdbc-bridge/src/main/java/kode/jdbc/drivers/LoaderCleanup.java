package kode.jdbc.drivers;

import java.sql.Driver;
import java.sql.DriverManager;
import java.sql.SQLException;

/** Its known class bytes are defined inside the driver loader. DriverManager's
 * caller visibility check otherwise prevents the helper from deregistering
 * drivers registered by child-loader static initializers.
 */
public final class LoaderCleanup {
    public static void deregisterOwned() throws SQLException {
        ClassLoader owner=LoaderCleanup.class.getClassLoader();
        var drivers=DriverManager.getDrivers();
        while(drivers.hasMoreElements()){Driver driver=drivers.nextElement();if(driver.getClass().getClassLoader()==owner)DriverManager.deregisterDriver(driver);}
    }
}
