package com.coordinator;

import android.content.AttributionSource;
import android.content.Context;
import android.content.ContextWrapper;
import android.graphics.PixelFormat;
import android.hardware.display.VirtualDisplay;
import android.media.Image;
import android.media.ImageReader;
import android.os.Process;
import android.os.SystemClock;
import android.view.InputDevice;
import android.view.InputEvent;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Surface;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.nio.ByteBuffer;

public final class VirtualScreen {

    // --- Shared singletons (static) ---

    private static Object displayManagerGlobal;
    private static Object inputManager;
    private static Method injectInputEventMethod;

    private static final ContextWrapper fakeContext = new ContextWrapper(null) {
        @Override
        public String getPackageName() {
            return "com.android.shell";
        }

        @Override
        public String getOpPackageName() {
            return getPackageName();
        }

        @Override
        public AttributionSource getAttributionSource() {
            AttributionSource.Builder builder = new AttributionSource.Builder(Process.myUid());
            builder.setPackageName(getPackageName());
            return builder.build();
        }
    };

    // --- Instance fields ---

    private int displayId;
    private final int displayWidth;
    private final int displayHeight;
    private final int displayDpi;
    private final ImageReader imageReader;
    private final VirtualDisplay virtualDisplay;
    private byte[] rgbaBuffer;

    // --- Constructor ---

    public VirtualScreen(int width, int height, int dpi) throws Exception {
        this.displayWidth = width;
        this.displayHeight = height;
        this.displayDpi = dpi;

        this.imageReader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2);
        Surface surface = imageReader.getSurface();

        String name = "coordinator-" + System.currentTimeMillis();
        this.virtualDisplay = createVirtualDisplay(name, width, height, dpi, surface);
        this.displayId = virtualDisplay.getDisplay().getDisplayId();
    }

    // --- Instance accessors ---

    public int getDisplayId() {
        return displayId;
    }

    public int getWidth() {
        return displayWidth;
    }

    public int getHeight() {
        return displayHeight;
    }

    public int getDpi() {
        return displayDpi;
    }

    // --- Release ---

    public void release() {
        virtualDisplay.release();
        imageReader.close();
    }

    // --- Screenshot ---

    public byte[] takeScreenshotRGBA() {
        Image image = imageReader.acquireLatestImage();
        if (image == null) {
            return null;
        }

        try {
            Image.Plane plane = image.getPlanes()[0];
            ByteBuffer buffer = plane.getBuffer();
            int pixelStride = plane.getPixelStride();
            int rowStride = plane.getRowStride();
            int rowPadding = rowStride - pixelStride * displayWidth;

            int size = displayWidth * displayHeight * 4;
            if (rgbaBuffer == null || rgbaBuffer.length != size) {
                rgbaBuffer = new byte[size];
            }

            if (rowPadding == 0) {
                buffer.get(rgbaBuffer, 0, size);
            } else {
                for (int row = 0; row < displayHeight; row++) {
                    buffer.position(row * rowStride);
                    buffer.get(rgbaBuffer, row * displayWidth * 4, displayWidth * 4);
                }
            }
            return rgbaBuffer;
        } finally {
            image.close();
        }
    }

    // --- Input injection (instance methods, use this.displayId) ---

    public void injectTap(float x, float y) throws ReflectiveOperationException {
        long now = SystemClock.uptimeMillis();

        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, x, y, 0);
        down.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        setDisplayId(down, displayId);
        injectInputEvent(down);
        down.recycle();

        MotionEvent up = MotionEvent.obtain(now, now + 10, MotionEvent.ACTION_UP, x, y, 0);
        up.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        setDisplayId(up, displayId);
        injectInputEvent(up);
        up.recycle();
    }

    public void injectSwipe(float x1, float y1, float x2, float y2, long durationMs) throws ReflectiveOperationException {
        long now = SystemClock.uptimeMillis();
        int steps = Math.max((int) (durationMs / 10), 2);

        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, x1, y1, 0);
        down.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        setDisplayId(down, displayId);
        injectInputEvent(down);
        down.recycle();

        for (int i = 1; i < steps; i++) {
            float t = (float) i / steps;
            float x = x1 + (x2 - x1) * t;
            float y = y1 + (y2 - y1) * t;
            long eventTime = now + (durationMs * i / steps);

            MotionEvent move = MotionEvent.obtain(now, eventTime, MotionEvent.ACTION_MOVE, x, y, 0);
            move.setSource(InputDevice.SOURCE_TOUCHSCREEN);
            setDisplayId(move, displayId);
            injectInputEvent(move);
            move.recycle();

            try {
                Thread.sleep(durationMs / steps);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
        }

        long endTime = now + durationMs;
        MotionEvent up = MotionEvent.obtain(now, endTime, MotionEvent.ACTION_UP, x2, y2, 0);
        up.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        setDisplayId(up, displayId);
        injectInputEvent(up);
        up.recycle();
    }

    public void injectKey(int keyCode) throws ReflectiveOperationException {
        long now = SystemClock.uptimeMillis();

        KeyEvent down = new KeyEvent(now, now, KeyEvent.ACTION_DOWN, keyCode, 0, 0,
                KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0, InputDevice.SOURCE_KEYBOARD);
        setDisplayId(down, displayId);
        injectInputEvent(down);

        KeyEvent up = new KeyEvent(now, now + 10, KeyEvent.ACTION_UP, keyCode, 0, 0,
                KeyCharacterMap.VIRTUAL_KEYBOARD, 0, 0, InputDevice.SOURCE_KEYBOARD);
        setDisplayId(up, displayId);
        injectInputEvent(up);
    }

    public void injectText(String text) throws ReflectiveOperationException {
        KeyCharacterMap kcm = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD);
        KeyEvent[] events = kcm.getEvents(text.toCharArray());
        if (events != null) {
            for (KeyEvent event : events) {
                setDisplayId(event, displayId);
                injectInputEvent(event);
            }
        } else {
            try {
                Runtime.getRuntime().exec(new String[]{
                        "input", "-d", String.valueOf(displayId), "text", text
                }).waitFor();
            } catch (Exception e) {
                throw new RuntimeException("Failed to inject text", e);
            }
        }
    }

    // --- Static utilities ---

    private static synchronized Object getDisplayManagerGlobal() throws ReflectiveOperationException {
        if (displayManagerGlobal == null) {
            Class<?> dmgClass = Class.forName("android.hardware.display.DisplayManagerGlobal");
            Method getInstance = dmgClass.getDeclaredMethod("getInstance");
            displayManagerGlobal = getInstance.invoke(null);
        }
        return displayManagerGlobal;
    }

    private static VirtualDisplay createVirtualDisplay(String name, int width, int height, int dpi, Surface surface) throws Exception {
        Object dmg = getDisplayManagerGlobal();

        Class<?> builderClass = Class.forName("android.hardware.display.VirtualDisplayConfig$Builder");
        Class<?> configClass = Class.forName("android.hardware.display.VirtualDisplayConfig");
        Method createMethod = dmg.getClass().getMethod("createVirtualDisplay",
                Context.class,
                android.media.projection.MediaProjection.class,
                configClass,
                VirtualDisplay.Callback.class,
                java.util.concurrent.Executor.class);

        int baseFlags = getRequiredDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_PUBLIC")
                | getRequiredDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_OWN_CONTENT_ONLY")
                | getRequiredDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_SUPPORTS_TOUCH")
                | getRequiredDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_ROTATES_WITH_CONTENT")
                | getRequiredDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_DESTROY_CONTENT_ON_REMOVAL");

        int trustedFlags = baseFlags
                | getOptionalDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_TRUSTED")
                | getOptionalDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_SHOULD_SHOW_SYSTEM_DECORATIONS")
                | getOptionalDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_OWN_FOCUS")
                | getOptionalDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_OWN_DISPLAY_GROUP")
                | getOptionalDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_ALWAYS_UNLOCKED")
                | getOptionalDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_TOUCH_FEEDBACK_DISABLED")
                | getOptionalDisplayManagerFlag("VIRTUAL_DISPLAY_FLAG_DEVICE_DISPLAY_GROUP");

        try {
            Object trustedConfig = buildVirtualDisplayConfig(
                    builderClass, name, width, height, dpi, surface, trustedFlags, true);
            return (VirtualDisplay) createMethod.invoke(dmg, fakeContext, null, trustedConfig, null, null);
        } catch (InvocationTargetException e) {
            Throwable cause = e.getCause();
            if (cause instanceof SecurityException) {
                Object fallbackConfig = buildVirtualDisplayConfig(
                        builderClass, name, width, height, dpi, surface, baseFlags, false);
                return (VirtualDisplay) createMethod.invoke(dmg, fakeContext, null, fallbackConfig, null, null);
            }
            throw e;
        }
    }

    private static Object buildVirtualDisplayConfig(
            Class<?> builderClass,
            String name,
            int width,
            int height,
            int dpi,
            Surface surface,
            int flags,
            boolean enableHomeSupport) throws ReflectiveOperationException {
        Object builder = builderClass.getConstructor(String.class, int.class, int.class, int.class)
                .newInstance(name, width, height, dpi);
        builderClass.getMethod("setFlags", int.class).invoke(builder, flags);
        builderClass.getMethod("setSurface", Surface.class).invoke(builder, surface);
        if (enableHomeSupport) {
            invokeOptionalBuilderMethod(builderClass, builder, "setHomeSupported", true);
        }
        invokeOptionalBuilderMethod(builderClass, builder, "setIgnoreActivitySizeRestrictions", true);
        return builderClass.getMethod("build").invoke(builder);
    }

    private static void invokeOptionalBuilderMethod(
            Class<?> builderClass, Object builder, String methodName, boolean value)
            throws ReflectiveOperationException {
        try {
            Method method = builderClass.getMethod(methodName, boolean.class);
            method.invoke(builder, value);
        } catch (NoSuchMethodException ignored) {}
    }

    private static int getRequiredDisplayManagerFlag(String name) throws ReflectiveOperationException {
        Class<?> dmClass = Class.forName("android.hardware.display.DisplayManager");
        return dmClass.getField(name).getInt(null);
    }

    private static int getOptionalDisplayManagerFlag(String name) {
        try {
            return getRequiredDisplayManagerFlag(name);
        } catch (ReflectiveOperationException e) {
            return 0;
        }
    }

    // --- InputManager (static shared) ---

    private static synchronized Object getInputManager() throws ReflectiveOperationException {
        if (inputManager == null) {
            try {
                Class<?> imgClass = Class.forName("android.hardware.input.InputManagerGlobal");
                Method getInstance = imgClass.getDeclaredMethod("getInstance");
                inputManager = getInstance.invoke(null);
                return inputManager;
            } catch (ClassNotFoundException | NoSuchMethodException e) {
                // Fall through to legacy path
            }
            Class<?> imClass = Class.forName("android.hardware.input.InputManager");
            Method getInstance = imClass.getDeclaredMethod("getInstance");
            inputManager = getInstance.invoke(null);
        }
        return inputManager;
    }

    private static boolean injectInputEvent(InputEvent event) throws ReflectiveOperationException {
        Object im = getInputManager();
        if (injectInputEventMethod == null) {
            injectInputEventMethod = im.getClass().getMethod("injectInputEvent", InputEvent.class, int.class);
        }
        return (boolean) injectInputEventMethod.invoke(im, event, 0);
    }

    private static void setDisplayId(InputEvent event, int displayId) throws ReflectiveOperationException {
        Method setDisplayId = InputEvent.class.getMethod("setDisplayId", int.class);
        setDisplayId.invoke(event, displayId);
    }
}
