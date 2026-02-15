package com.coordinator;

import android.os.Looper;

import java.nio.charset.StandardCharsets;

public final class Main {

    static native void nativeRun(String[] args);

    public static void main(String[] args) throws Exception {
        String libPath = System.getenv("ANDY_LIB");
        if (libPath == null || libPath.isEmpty()) {
            throw new IllegalArgumentException("ANDY_LIB env var required");
        }

        disableLockScreenAtStartup();
        Looper.prepareMainLooper();

        System.load(libPath);
        nativeRun(args);
    }

    private static void disableLockScreenAtStartup() {
        boolean disabled =
                runBestEffortCommand("/system/bin/locksettings", "set-disabled", "true")
                        || runBestEffortCommand("/system/bin/cmd", "lock_settings", "set-disabled", "true");

        runBestEffortCommand("/system/bin/wm", "dismiss-keyguard");

        if (!disabled) {
            System.err.println("Warning: Could not disable lockscreen at startup.");
        }
    }

    private static boolean runBestEffortCommand(String... command) {
        try {
            ProcessBuilder pb = new ProcessBuilder(command);
            pb.redirectErrorStream(true);
            Process proc = pb.start();
            byte[] output = proc.getInputStream().readAllBytes();
            int exitCode = proc.waitFor();
            String out = new String(output, StandardCharsets.UTF_8).trim();
            if (exitCode != 0 && indicatesSuccess(command, out)) {
                return true;
            }
            if (exitCode != 0) {
                if (out.isEmpty()) {
                    out = "(no output)";
                }
                System.err.println(
                        "Startup command failed (" + String.join(" ", command) + "): " + out);
                return false;
            }
            return true;
        } catch (Exception e) {
            System.err.println(
                    "Startup command error (" + String.join(" ", command) + "): " + e.getMessage());
            return false;
        }
    }

    private static boolean indicatesSuccess(String[] command, String output) {
        if (output == null || output.isEmpty()) {
            return false;
        }
        String joined = String.join(" ", command);
        if (joined.contains("locksettings") || joined.contains("lock_settings")) {
            return output.contains("Lock screen disabled set to");
        }
        return false;
    }
}
