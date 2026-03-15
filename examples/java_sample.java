/**
 * Sample Java class with intentional bugs for testing Minion Engine workflows.
 * Contains security vulnerabilities, resource leaks, and null safety issues.
 */

import java.io.*;
import java.sql.*;
import java.util.*;

public class java_sample {

    // BUG: Hardcoded credentials
    private static final String DB_URL = "jdbc:mysql://prod-server:3306/users";
    private static final String DB_USER = "root";
    private static final String DB_PASS = "root123";

    /**
     * BUG: SQL injection vulnerability — uses string concatenation
     * BUG: Resource leak — Connection never closed
     */
    public static Map<String, Object> getUser(String userId) throws SQLException {
        Connection conn = DriverManager.getConnection(DB_URL, DB_USER, DB_PASS);
        Statement stmt = conn.createStatement();
        // SQL INJECTION: userId not parameterized
        ResultSet rs = stmt.executeQuery("SELECT * FROM users WHERE id = '" + userId + "'");

        Map<String, Object> user = new HashMap<>();
        if (rs.next()) {
            user.put("id", rs.getString("id"));
            user.put("name", rs.getString("name"));
            user.put("email", rs.getString("email"));
        }
        // BUG: conn, stmt, rs never closed — resource leak
        return user;
    }

    /**
     * BUG: Null pointer dereference — no null check on input
     * BUG: Catches generic Exception instead of specific ones
     */
    public static String processInput(String input) {
        try {
            // BUG: NullPointerException if input is null
            return input.trim().toLowerCase();
        } catch (Exception e) {
            // BUG: Swallows exception silently
            return "";
        }
    }

    /**
     * BUG: Insecure deserialization — arbitrary code execution
     */
    public static Object loadObject(String path) throws Exception {
        FileInputStream fis = new FileInputStream(path);
        ObjectInputStream ois = new ObjectInputStream(fis);
        Object obj = ois.readObject();
        // BUG: streams not closed in finally block
        return obj;
    }

    /**
     * BUG: Storing password in plaintext
     * BUG: No input validation on email
     */
    public static Map<String, String> createUser(String name, String email, String password) {
        Map<String, String> user = new HashMap<>();
        user.put("name", name);
        user.put("email", email);
        user.put("password", password); // Plaintext!
        return user;
    }

    /**
     * BUG: Thread safety issue — shared mutable state without synchronization
     */
    private static List<String> auditLog = new ArrayList<>();

    public static void logAction(String action) {
        // BUG: Not thread-safe — ArrayList is not synchronized
        auditLog.add(new Date() + ": " + action);
    }

    /**
     * BUG: Checked exception not properly handled
     * BUG: File path from user input without validation (path traversal)
     */
    public static String readFile(String userPath) {
        try {
            // BUG: Path traversal — user could pass "../../../etc/passwd"
            BufferedReader reader = new BufferedReader(new FileReader(userPath));
            StringBuilder content = new StringBuilder();
            String line;
            while ((line = reader.readLine()) != null) {
                content.append(line).append("\n");
            }
            // BUG: reader never closed
            return content.toString();
        } catch (IOException e) {
            return null; // BUG: Returns null instead of throwing or Optional
        }
    }
}
