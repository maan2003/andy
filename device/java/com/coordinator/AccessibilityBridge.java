package com.coordinator;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.content.AttributionSource;
import android.content.Context;
import android.content.ContextWrapper;
import android.graphics.Point;
import android.graphics.Rect;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.Process;
import android.os.SystemClock;
import android.util.JsonWriter;
import android.util.SparseArray;
import android.view.Display;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;

import java.io.StringWriter;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

final class AccessibilityBridge {
    private static final int MAX_NODES = 12000;
    private static final int MAX_DEPTH = 80;

    private final Object uiAutomation;
    private final Method getWindowsOnAllDisplaysMethod;
    private final Method getWindowsMethod;
    private final Method windowGetDisplayIdMethod;
    private final Method nodeGetChildPrefetchMethod;
    private final Integer nodePrefetchHybridFlag;
    private Object displayManagerGlobal;
    private Method getRealDisplayMethod;

    AccessibilityBridge() throws Exception {
        HandlerThread thread = new HandlerThread("CoordinatorA11y");
        thread.start();
        Looper looper = thread.getLooper();

        Class<?> uiAutomationClass = Class.forName("android.app.UiAutomation");
        Class<?> uiAutomationConnectionClass = Class.forName("android.app.UiAutomationConnection");
        Class<?> iUiAutomationConnectionClass = Class.forName("android.app.IUiAutomationConnection");

        Constructor<?> connCtor = uiAutomationConnectionClass.getDeclaredConstructor();
        connCtor.setAccessible(true);
        Object uiAutomationConnection = connCtor.newInstance();

        Object automation;
        ContextWrapper fakeContext = new ContextWrapper(null) {
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
        try {
            Constructor<?> looperCtor =
                    uiAutomationClass.getDeclaredConstructor(Looper.class, iUiAutomationConnectionClass);
            looperCtor.setAccessible(true);
            automation = looperCtor.newInstance(looper, uiAutomationConnection);
        } catch (NoSuchMethodException e) {
            Constructor<?> contextCtor =
                    uiAutomationClass.getDeclaredConstructor(Context.class, iUiAutomationConnectionClass);
            contextCtor.setAccessible(true);
            automation = contextCtor.newInstance(fakeContext, uiAutomationConnection);
        }

        Method connectNoArgs = null;
        try {
            connectNoArgs = uiAutomationClass.getMethod("connect");
        } catch (NoSuchMethodException ignored) {}
        if (connectNoArgs != null) {
            connectNoArgs.invoke(automation);
        } else {
            uiAutomationClass.getMethod("connect", int.class).invoke(automation, 0);
        }

        Method getServiceInfoMethod = uiAutomationClass.getMethod("getServiceInfo");
        Object serviceInfoObj = getServiceInfoMethod.invoke(automation);
        if (serviceInfoObj instanceof AccessibilityServiceInfo) {
            AccessibilityServiceInfo info = (AccessibilityServiceInfo) serviceInfoObj;
            info.flags |= AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS;
            info.flags |= AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS;
            info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
            uiAutomationClass.getMethod("setServiceInfo", AccessibilityServiceInfo.class)
                    .invoke(automation, info);
        }

        Method gwoad = null;
        try {
            gwoad = uiAutomationClass.getMethod("getWindowsOnAllDisplays");
        } catch (NoSuchMethodException ignored) {}

        Method wgdi = null;
        try {
            wgdi = AccessibilityWindowInfo.class.getMethod("getDisplayId");
        } catch (NoSuchMethodException ignored) {}

        Method ngcp = null;
        Integer npf = null;
        try {
            ngcp = AccessibilityNodeInfo.class.getMethod("getChild", int.class, int.class);
            npf = AccessibilityNodeInfo.class.getField("FLAG_PREFETCH_DESCENDANTS_HYBRID").getInt(null);
        } catch (ReflectiveOperationException ignored) {}

        this.uiAutomation = automation;
        this.getWindowsOnAllDisplaysMethod = gwoad;
        this.getWindowsMethod = uiAutomationClass.getMethod("getWindows");
        this.windowGetDisplayIdMethod = wgdi;
        this.nodeGetChildPrefetchMethod = ngcp;
        this.nodePrefetchHybridFlag = npf;
    }

    String dumpDisplayJson(int displayId) throws Exception {
        List<AccessibilityWindowInfo> windows = getWindowsForDisplay(displayId);
        Point displaySize = getDisplaySize(displayId);

        StringWriter out = new StringWriter(16 * 1024);
        JsonWriter writer = new JsonWriter(out);
        writer.beginObject();
        writer.name("display_id").value(displayId);
        writer.name("timestamp_ms").value(SystemClock.uptimeMillis());
        if (displaySize != null) {
            writer.name("display").beginObject();
            writer.name("width").value(displaySize.x);
            writer.name("height").value(displaySize.y);
            writer.endObject();
        }

        writer.name("window_count").value(windows.size());
        writer.name("windows").beginArray();

        SnapshotState state = new SnapshotState(writer);
        for (int i = 0; i < windows.size(); i++) {
            AccessibilityWindowInfo w = windows.get(i);
            if (w.getType() != AccessibilityWindowInfo.TYPE_APPLICATION) continue;
            writeWindow(w, i, state, displaySize);
        }

        writer.endArray();
        writer.name("node_count").value(state.nodeCount);
        writer.name("truncated").value(state.truncated);
        writer.endObject();
        writer.close();
        return out.toString();
    }

    @SuppressWarnings("unchecked")
    private List<AccessibilityWindowInfo> getWindowsForDisplay(int displayId) throws Exception {
        if (getWindowsOnAllDisplaysMethod != null) {
            Object allObj = getWindowsOnAllDisplaysMethod.invoke(uiAutomation);
            if (allObj instanceof SparseArray) {
                Object value = ((SparseArray<?>) allObj).get(displayId);
                if (value instanceof List) {
                    return new ArrayList<>((List<AccessibilityWindowInfo>) value);
                }
            }
        }

        Object windowsObj = getWindowsMethod.invoke(uiAutomation);
        if (!(windowsObj instanceof List)) {
            return Collections.emptyList();
        }
        List<?> windows = (List<?>) windowsObj;
        if (windows.isEmpty()) {
            return Collections.emptyList();
        }

        if (windowGetDisplayIdMethod == null) {
            if (displayId == 0) {
                return new ArrayList<>((List<AccessibilityWindowInfo>) windowsObj);
            }
            return Collections.emptyList();
        }

        ArrayList<AccessibilityWindowInfo> filtered = new ArrayList<>();
        for (Object obj : windows) {
            if (!(obj instanceof AccessibilityWindowInfo)) {
                continue;
            }
            AccessibilityWindowInfo window = (AccessibilityWindowInfo) obj;
            if (getWindowDisplayId(window) == displayId) {
                filtered.add(window);
            }
        }
        return filtered;
    }

    private void writeWindow(
            AccessibilityWindowInfo window, int index, SnapshotState state, Point displaySize)
            throws Exception {
        JsonWriter writer = state.writer;
        writer.beginObject();
        writer.name("index").value(index);
        writer.name("id").value(window.getId());
        writer.name("display_id").value(getWindowDisplayId(window));
        writer.name("type").value(window.getType());
        writer.name("layer").value(window.getLayer());
        writer.name("active").value(window.isActive());
        writer.name("focused").value(window.isFocused());
        writer.name("accessibility_focused").value(window.isAccessibilityFocused());
        writer.name("title").value(toNullableString(window.getTitle()));

        Rect bounds = new Rect();
        window.getBoundsInScreen(bounds);
        writer.name("bounds");
        writeRect(writer, bounds);

        writer.name("nodes").beginArray();
        AccessibilityNodeInfo root = window.getRoot();
        if (root != null) {
            writeNode(root, 0, 0, -1, state, window.getId(), displaySize);
        }
        writer.endArray();
        writer.endObject();
    }

    private void writeNode(
            AccessibilityNodeInfo node,
            int depth,
            int indexInParent,
            int parentId,
            SnapshotState state,
            int windowId,
            Point displaySize)
            throws Exception {
        JsonWriter writer = state.writer;
        if (depth > MAX_DEPTH || state.nodeCount >= MAX_NODES) {
            state.truncated = true;
            return;
        }

        int nodeId = ++state.nextNodeId;
        state.nodeCount++;

        writer.beginObject();
        writer.name("id").value(nodeId);
        writer.name("parent_id").value(parentId == -1 ? null : Integer.valueOf(parentId));
        writer.name("window_id").value(windowId);
        writer.name("index").value(indexInParent);
        writer.name("package").value(toNullableString(node.getPackageName()));
        writer.name("class").value(toNullableString(node.getClassName()));
        writer.name("resource_id").value(node.getViewIdResourceName());
        writer.name("text").value(toNullableString(node.getText()));
        writer.name("content_desc").value(toNullableString(node.getContentDescription()));
        writer.name("hint").value(toNullableString(node.getHintText()));
        writer.name("checkable").value(node.isCheckable());
        writer.name("checked").value(node.isChecked());
        writer.name("clickable").value(node.isClickable());
        writer.name("enabled").value(node.isEnabled());
        writer.name("focusable").value(node.isFocusable());
        writer.name("focused").value(node.isFocused());
        writer.name("scrollable").value(node.isScrollable());
        writer.name("long_clickable").value(node.isLongClickable());
        writer.name("selected").value(node.isSelected());
        writer.name("password").value(node.isPassword());
        writer.name("visible").value(node.isVisibleToUser());
        writer.name("drawing_order").value(node.getDrawingOrder());

        Rect bounds = new Rect();
        node.getBoundsInScreen(bounds);
        clipToDisplay(bounds, displaySize);
        writer.name("bounds");
        writeRect(writer, bounds);
        writer.endObject();

        int childCount = node.getChildCount();
        for (int i = 0; i < childCount; i++) {
            if (state.nodeCount >= MAX_NODES) {
                state.truncated = true;
                break;
            }
            AccessibilityNodeInfo child = getChild(node, i);
            if (child == null || !child.isVisibleToUser()) {
                continue;
            }
            writeNode(child, depth + 1, i, nodeId, state, windowId, displaySize);
        }
    }

    private AccessibilityNodeInfo getChild(AccessibilityNodeInfo node, int index) {
        try {
            if (nodeGetChildPrefetchMethod != null && nodePrefetchHybridFlag != null) {
                Object child = nodeGetChildPrefetchMethod.invoke(node, index, nodePrefetchHybridFlag);
                return child instanceof AccessibilityNodeInfo ? (AccessibilityNodeInfo) child : null;
            }
        } catch (ReflectiveOperationException ignored) {
        }
        try {
            return node.getChild(index);
        } catch (RuntimeException e) {
            return null;
        }
    }

    private static void writeRect(JsonWriter writer, Rect rect) throws Exception {
        writer.beginObject();
        writer.name("left").value(rect.left);
        writer.name("top").value(rect.top);
        writer.name("right").value(rect.right);
        writer.name("bottom").value(rect.bottom);
        writer.endObject();
    }

    private static void clipToDisplay(Rect bounds, Point displaySize) {
        if (displaySize == null) {
            return;
        }
        Rect displayRect = new Rect(0, 0, displaySize.x, displaySize.y);
        if (!bounds.intersect(displayRect)) {
            bounds.setEmpty();
        }
    }

    private int getWindowDisplayId(AccessibilityWindowInfo window) {
        if (windowGetDisplayIdMethod == null) {
            return 0;
        }
        try {
            return (int) windowGetDisplayIdMethod.invoke(window);
        } catch (ReflectiveOperationException e) {
            return 0;
        }
    }

    private Point getDisplaySize(int displayId) {
        try {
            if (displayManagerGlobal == null) {
                Class<?> dmgClass = Class.forName("android.hardware.display.DisplayManagerGlobal");
                displayManagerGlobal = dmgClass.getDeclaredMethod("getInstance").invoke(null);
            }
            if (getRealDisplayMethod == null) {
                getRealDisplayMethod = displayManagerGlobal.getClass().getMethod("getRealDisplay", int.class);
            }
            Object displayObj = getRealDisplayMethod.invoke(displayManagerGlobal, displayId);
            if (!(displayObj instanceof Display)) {
                return null;
            }
            Display display = (Display) displayObj;
            Point size = new Point();
            display.getRealSize(size);
            return size;
        } catch (ReflectiveOperationException e) {
            return null;
        }
    }

    boolean waitForIdle(long idleTimeoutMillis, long globalTimeoutMillis) throws Exception {
        // Reset mLastEventTimeMillis to "now" so waitForIdle doesn't return
        // immediately when the UI has been idle longer than idleTimeoutMillis
        // before this call.
        java.lang.reflect.Field lockField = uiAutomation.getClass().getDeclaredField("mLock");
        lockField.setAccessible(true);
        Object lock = lockField.get(uiAutomation);
        java.lang.reflect.Field lastEventField = uiAutomation.getClass().getDeclaredField("mLastEventTimeMillis");
        lastEventField.setAccessible(true);
        synchronized (lock) {
            lastEventField.setLong(uiAutomation, SystemClock.uptimeMillis());
        }
        Method m = uiAutomation.getClass().getMethod("waitForIdle", long.class, long.class);
        try {
            m.invoke(uiAutomation, idleTimeoutMillis, globalTimeoutMillis);
            return true;
        } catch (java.lang.reflect.InvocationTargetException e) {
            if (e.getCause() instanceof java.util.concurrent.TimeoutException) {
                return false;
            }
            throw e;
        }
    }

    private static String toNullableString(CharSequence cs) {
        return cs == null ? null : cs.toString();
    }

    private static final class SnapshotState {
        final JsonWriter writer;
        int nextNodeId;
        int nodeCount;
        boolean truncated;

        SnapshotState(JsonWriter writer) {
            this.writer = writer;
        }
    }
}
