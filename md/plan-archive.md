# Android App Testing Platform for LLMs

-- newer plan: use roborazzi

## Goal

Let LLMs verify their own code by running it. When an LLM writes or edits React Native code, it can immediately build, deploy, and interact with the running app to check that its changes actually work — visually and functionally. Multiple LLMs work in parallel on different worktrees of the same app, each with their own running instance to test against.

## Requirements

- Parallel instances (one per LLM/worktree)
- Hypothetical overhead target (~35-40MB per instance, ~200-300MB shared base; must be measured)
- No GPU required (but can use one when available)
- Screenshots on demand (JPEG, ~14ms)
- CDP (Chrome DevTools Protocol) for JS runtime control
- Coordinator process (app_process + Rust JNI, shell user) for display/input/lifecycle management
- Hypothetical sub-second code reload target (must be measured)

---

## Architecture Overview

```
Phase 1 — Stock Android runtime + Coordinator:

Host Linux
├── Android runtime (any: emulator, Waydroid, Cuttlefish)
│   ├── Coordinator (app_process + Rust JNI, runs as shell user)
│   │   ├── VirtualDisplay + ImageReader per instance
│   │   ├── Input injection (InputManager/Instrumentation API)
│   │   ├── App lifecycle (ActivityManager API)
│   │   ├── Accessibility tree dump per display
│   │   └── Communicates via stdin/stdout (postcard binary protocol)
│   └── adbd (bootstrap only — launches coordinator)
│
├── Host-side Rust client library (tokio async)
│   └── Spawns coordinator via adb shell -T, routes CDP, exposes API to LLMs
│
├── LLM #0 ←→ Worktree #0 ←→ bundler → bundle #0 → app s00 → CDP
├── LLM #1 ←→ Worktree #1 ←→ bundler → bundle #1 → app s01 → CDP
└── LLM #N ...


Phase 2 target — Headless container (replaces stock runtime):

Host Linux
├── /dev/dri/renderD128 (optional — GPU render node)
│
├── Waydroid Android image (LineageOS) + custom hwcomposer.dummy.so
│   └── Single container (LXC, shared host kernel, binder module)
│       ├── SurfaceFlinger
│       │   ├── HWComposer.dummy (fake primary display, discards frames)
│       │   ├── EGL/GL via mesa (render node → GPU, or llvmpipe → software)
│       │   └── gralloc via mesa GBM
│       ├── Coordinator (same as Phase 1, runs as shell user)
│       └── adbd
│
├── Host-side control daemon (same as Phase 1)
│
├── LLM #0 ←→ Worktree #0 ←→ bundler → bundle #0 → app s00 → CDP
├── LLM #1 ←→ Worktree #1 ←→ bundler → bundle #1 → app s01 → CDP
└── LLM #N ...

No Wayland. No display server. No compositor on host.
Just Android in a container with optional GPU render node.
```

---

## Key Design Decisions

### 1. Container, not VM

Waydroid runs Android in a Linux container (LXC + namespaces). Shares the host kernel. No hypervisor overhead. ~200-300MB base vs ~400-500MB for a VM (Cuttlefish).

### 2. Waydroid images, not Waydroid runtime

Use Waydroid's pre-built LineageOS images because:
- Container-ready (binder, ashmem, mount points configured)
- GPU passthrough already set up (gralloc, virgl/mesa)
- Same image works with GPU or llvmpipe — no changes needed
- Regularly updated

Do NOT use Waydroid's Python scripts, session management, or desktop integration. Build custom orchestration instead.

### 3. Dummy HWComposer (no Wayland)

SurfaceFlinger requires at least one display from HWComposer to start. Fork Waydroid's HWComposer, strip all Wayland code, replace with a dummy that:
- Reports a single fake display (360x640, 1Hz)
- Returns `CLIENT_COMPOSITION` for all layers (SurfaceFlinger does all GL compositing)
- Discards presented frames (VirtualDisplays handle real output)
- Generates VSync from a timer thread

~250 lines of C++. Drop-in replacement for `hwcomposer.waydroid.so`.

**No Wayland compositor needed on host. No display server. Nothing.**

GPU rendering is separate from HWComposer — it goes through gralloc (mesa/GBM) and EGL (mesa), both of which use the DRM render node directly:

```
With GPU:    mount /dev/dri/renderD128 into container → mesa uses GPU
Without GPU: don't mount anything → mesa falls back to llvmpipe
Same image, same container, same HALs. Auto-detected by mesa.
```

### 3b. Virtual Displays for app rendering (not Wayland windows)

Each app instance renders to an Android VirtualDisplay backed by an ImageReader, not to a Wayland window. This is better than Waydroid's multi-window mode because:
- Screenshots are direct memory reads from ImageReader + JPEG encode (~14ms, no compositor involvement)
- No dependency on Wayland for the capture/render path
- Each app gets a proper isolated display context
- Resolution/density configurable per instance
- SurfaceFlinger composites to the ImageReader Surface using GPU composition (or llvmpipe)

```java
// Coordinator creates per-instance virtual displays (runs as shell user via app_process)
DisplayManager dm = context.getSystemService(DisplayManager.class);
ImageReader reader = ImageReader.newInstance(360, 640, PixelFormat.RGBA_8888, 2);
VirtualDisplay vd = dm.createVirtualDisplay(
    "instance-07", 360, 640, 160, reader.getSurface(),
    VIRTUAL_DISPLAY_FLAG_PUBLIC | VIRTUAL_DISPLAY_FLAG_OWN_CONTENT_ONLY);

// Launch app on this display
ActivityManager am = context.getSystemService(ActivityManager.class);
// or via: Runtime.exec("am start --display <displayId> -n com.host.app.s07/.HostActivity")

// Screenshot = read ImageReader buffer (pure memory, no compositor)
Image img = reader.acquireLatestImage();
```

**Privilege requirements:** The coordinator runs via `app_process` as shell UID (2000). Shell user has the permissions needed for `VIRTUAL_DISPLAY_FLAG_PUBLIC`, `am start --display`, input injection, and accessibility tree access. No platform signing or system-app privileges required. This is the same privilege level as ADB shell commands, but running as a persistent in-process program gives access to VirtualDisplay + ImageReader APIs (fast screenshots), per-display accessibility tree dumping, and eliminates per-command process spawn overhead.

```
Coordinator (app_process as shell user — no platform signing needed):
  VirtualDisplay + ImageReader creation (no overlay display cap)
  Direct buffer reads + JPEG encode for ~14ms screenshots
  Input injection via InputManager/Instrumentation API
  App lifecycle via ActivityManager
  Per-display accessibility tree dump
  Per-display density/resolution control
```

### 4. Multiple instances via different app IDs

Install the same host APK under different package names:

```
com.host.app.s00  →  own process, own data dir, own CDP socket
com.host.app.s01  →  own process, own data dir, own CDP socket
...
```

Why not multi-user:
- Android only shows one user foreground per display
- Background users' Activities get stopped and surfaces destroyed
- Multi-display per-user is poorly supported

Why different app IDs work:
- All apps under same user — all equally foreground-eligible
- Each launches on its own VirtualDisplay (coordinator creates display, launches app on it)
- Each gets own process (Zygote fork, COW), own data dir, own CDP socket
- Full isolation with zero platform hacks

### 5. Multi-instance APKs via Gradle variants

Build multiple installable package IDs from the same source using Gradle flavors/applicationId suffixes (for example, `com.host.app.s00` ... `com.host.app.s49`), then pre-install a pool.

Use standard Gradle signing/build outputs only; no binary patching of `.apk` files.

### 6. Code delivery is bundler-agnostic

Each LLM has its own worktree with different source code. The platform doesn't prescribe a bundler — it accepts a JS bundle file and loads it. The project's existing build toolchain (Metro, Expo, etc.) produces the bundle; the platform just delivers it to the app instance.

- LLM edits source files in its worktree
- Project's own bundler builds the bundle (Metro, Expo, whatever the project uses)
- Push bundle to app instance (via adb push or coordinator file write)
- App reloads bundle (via CDP `DevSettings.reload()` or coordinator-triggered reload)

This keeps the platform project-agnostic. Any React Native project that can produce a `.jsbundle` file works.

### 7. Three control channels

```
CDP (WebSocket per instance):
  - Inspect React component tree, props, state
  - Evaluate arbitrary JS (Runtime.evaluate)
  - Call functions, trigger navigation, modify state
  - Read structured UI description (describeScreen helper)
  - Initial constraint: tightly coupled to a specific RN/Hermes version
  - No CDP fallback path in v1

Coordinator (app_process + Rust JNI, shell user, stdin/stdout binary protocol):
  - VirtualDisplay create/destroy
  - Screenshot via ImageReader + JPEG encode (~14ms)
  - Input injection: tap, text, swipe (InputManager API)
  - App lifecycle: start, stop, force-stop (ActivityManager)
  - Accessibility tree dump per display
  - Display configuration (resolution, density)

Filesystem → Bundler:
  - Write/edit .tsx files in worktree
  - Project's bundler builds bundle
  - Push to app, trigger reload via CDP
```

### 8. Structured UI for LLM consumption

Screenshots alone are poor LLM input. Provide structured data:

**Accessibility tree (via coordinator, per-display):**
The coordinator runs as shell user and can access `AccessibilityNodeInfo` APIs or `UiAutomation` to dump the accessibility tree for a specific display. The stock `uiautomator dump --display` command is buggy (silently ignores the display argument — see FINDINGS.md), but the underlying framework APIs support per-display queries. The coordinator calls these APIs directly, bypassing uiautomator's bug.

```
Coordinator: send AccessibilityTree request over binary protocol
Returns: XML with element types, text, bounds, clickable state — scoped to that instance's display
```

**React component tree (via CDP, richer):**
Include a `describeScreen()` helper in the shell bundle that walks the React fiber tree and returns component names, props, layout coordinates. The LLM gets structured JSON and knows exactly what's tappable and where.

Most LLM iterations don't need a screenshot — the structured tree is faster and more informative. Screenshots become a fallback for visual verification.

---

## Memory Budget

All numbers in this section are hypotheses to validate with measurements on target hardware.

### Per-instance (marginal)

| Component | Memory |
|-----------|--------|
| App process (Zygote fork, COW) | ~30-35MB |
| VirtualDisplay + ImageReader (360x640) | ~2-3MB |
| **Total per instance** | **~35-40MB** |

Bundler memory is external to the platform. Metro costs ~200-500MB per worktree if kept running; on-demand bundlers use ~30MB transiently. This is project-specific overhead, not platform overhead.

### Shared base (fixed, once)

| Component | Memory |
|-----------|--------|
| Android container (Zygote + SystemServer + SF) | ~200-300MB |
| **Total shared** | **~200-300MB** |

### Example: 10 parallel LLMs

```
Shared base:              ~300MB
10 app instances:         10 × 38MB = ~380MB
Total:                    ~680MB
Per-LLM marginal:         ~38MB
```

---

## Startup Performance

All numbers in this section are hypotheses to validate with measurements on target hardware.

| Operation | Time |
|-----------|------|
| Android container boot (cold) | ~15-20s |
| New app instance (Zygote fork + launch) | ~100-200ms |
| Bundle build (project-specific, not platform) | varies |
| Bundle push + app reload | ~100-200ms |
| Screenshot (ImageReader + JPEG encode, on demand) | ~14ms |
| CDP Runtime.evaluate | ~5ms |
| Full edit→see cycle | bundle build + ~200-400ms (push + reload + render) |

Boot the container once. After that, launching new app instances is ~100-200ms (Zygote fork, already installed).

---

## Rendering & Screenshots

### Rendering path

All apps render to VirtualDisplays backed by ImageReader surfaces. SurfaceFlinger composites each virtual display independently using GPU composition (or llvmpipe in software mode). The Wayland primary display exists only as a dummy for SurfaceFlinger startup — no app content goes through Wayland.

```
App → Surface → SurfaceFlinger composites → ImageReader buffer (direct memory)
                                                     ↓
                                          Coordinator reads on demand → JPEG
```

### GPU available
- Host GPU via virgl/mesa passthrough
- SurfaceFlinger uses GPU composition for virtual displays
- Fast rendering, minimal CPU

### No GPU (software rendering)
- Set `LIBGL_ALWAYS_SOFTWARE=1` on host
- Mesa uses llvmpipe (software GL)
- Same image, same container, no changes
- CPU cost: near-zero for static screens, ~15-30% of one core during animations
- Reduce frame rate (`ro.surface_flinger.refresh_rate=1`) to minimize idle CPU

### Screenshot method
Primary: `ImageReader.acquireLatestImage()` on the virtual display surface, called by the coordinator — ~14ms (capture + JPEG encode), per-instance, no compositor involvement. Available in Phase 1 since the coordinator runs in-process with the ImageReaders. Uses `jpeg-encoder` crate (quality 85) with zero-copy JNI array access.

Fallback: `adb shell screencap` or `SurfaceControl.captureLayersAsForResult()` for debugging.

---

## The Host App (Shell APK)

A thin React Native app that accepts a bundle path and loads it:

```java
// HostActivity.java
public class HostActivity extends ReactActivity {
    @Override
    protected ReactActivityDelegate createReactActivityDelegate() {
        String bundleUrl = getIntent().getStringExtra("bundle_url");
        return new ReactActivityDelegate(this, "HostApp") {
            @Override
            protected ReactHost getReactHost() {
                return buildReactHost(bundleUrl);
            }
        };
    }
}
```

Shell JS bundle includes all common dependencies (react-native, navigation, reanimated, etc.) so that dynamically loaded code can `require()` them.

```javascript
// shell.js — fat shell, compiled to bytecode once
import 'react-native';
import '@react-navigation/native';
// ... all libraries the app might need

// Helper for LLM UI inspection
export function describeScreen() {
  // Walk React fiber tree, return structured JSON
  // with component names, props, layout coordinates
}

// Placeholder app — replaced via bundle reload or CDP eval
import { AppRegistry, View } from 'react-native';
AppRegistry.registerComponent('HostApp', () => () => <View />);
```

---

## LLM Workflow

```
1. LLM edits src/screens/Login.tsx in its worktree
2. Project bundler builds bundle                               (project-specific)
3. Coordinator: push bundle to /data/local/tmp/bundle_sNN.js   (~50ms)
4. CDP: DevSettings.reload()                                   (~100ms)
5. Coordinator: accessibility tree for instance
   or CDP: describeScreen() → structured component tree
6. LLM reads tree, decides to tap "Sign In" at (180, 300)
7. Coordinator: tap 180 300 on instance display
8. Coordinator: accessibility tree → verify navigation
9. Coordinator: screenshot via ImageReader + JPEG (optional)    (~14ms)
10. Repeat
```

---

## Components to Build

| Component | Phase | Effort | Lines (approx) |
|-----------|-------|--------|-----------------|
| Shell APK (thin RN host app) | 1 | Small | ~200 Java/Kotlin + ~100 JS |
| Gradle variant config/script for app ID pool | 1 | Small | ~50 |
| Coordinator (app_process + Rust JNI: VirtualDisplay, ImageReader, input, a11y tree) | 1 | Medium | ~100 Java + ~180 Rust |
| Host-side client library (spawns coordinator via adb, async API) | 1 | Medium | ~170 Rust (tokio) |
| Shared protocol crate (postcard binary framing) | 1 | Small | ~70 Rust |
| describeScreen() helper | 1 | Small | ~50 JS |
| Container setup script (LXC/namespace config) | 2 | Small | ~100 bash |
| hwcomposer.dummy.so (fake primary display, no Wayland) | 2 | Small | ~250 C++ |
| **Total** | | | **~2050 lines** |

---

## Implementation Phases

This is two loosely coupled projects. The coordinator + host daemon (Phase 1) doesn't care how Android runs — it just needs ADB to bootstrap the coordinator, then talks to it directly. The headless container runtime (Phase 2) is an optimization that can be swapped in later. Don't let Phase 2 block Phase 1.

### Phase 1: Multi-app orchestration (the product)

Get the LLM testing loop working end-to-end against a stock Android runtime.

**Android runtime**: Use any of these — they all expose ADB and work today:
- Stock Android emulator: `emulator -no-window -gpu swiftshader_indirect`
- Waydroid as-is: `weston --backend=headless` + Waydroid multi-window mode
- Cuttlefish: designed for CI, headless out of the box

Pick whichever boots fastest on your machine. The coordinator doesn't care.

**Build in this order:**

1. **Coordinator** *(done)* — Java + Rust JNI program running via `app_process` as shell user. This is the central piece. It:
   - Creates VirtualDisplays backed by ImageReaders (no overlay display 4-cap)
   - Takes screenshots via ImageReader buffer reads + JPEG encode (~14ms)
   - Injects input (tap, text, swipe) via InputManager/Instrumentation API
   - Manages app lifecycle (start, stop, force-stop) via ActivityManager
   - Dumps per-display accessibility trees (uses `uiautomator dump --display`)
   - Communicates via stdin/stdout using postcard-serialized length-prefixed binary frames
   - Spawned by host library via `adb shell -T` with `kill_on_drop`
2. **Shell APK** — thin RN host app that loads a bundle from a file path (passed via intent extra)
3. **Gradle variant setup** — generate package ID pool (`com.host.app.s00...sNN`), pre-install pool of 50
4. **Bundle delivery** — push bundle to `/data/local/tmp/bundle_sNN.js`, trigger reload via CDP (`DevSettings.reload()`) or coordinator relaunch
5. **describeScreen() helper** — walk React fiber tree, return structured JSON (via CDP)
6. **Host-side control daemon** *(coordinator client done, CDP routing TODO)* — `host-coordinator` Rust async library spawns coordinator via `adb shell -T`, provides typed async API. Still needs CDP routing and unified LLM-facing API.

The coordinator runs as shell UID (2000), which has the permissions needed for VirtualDisplay creation with `VIRTUAL_DISPLAY_FLAG_PUBLIC`, `am start --display`, input injection, and accessibility tree access. No platform signing required.

The project's own bundler (Metro, Expo, etc.) produces the JS bundle. The platform doesn't include or prescribe a bundler.

**Key hypothesis validated:** Shell user can create VirtualDisplays with `VIRTUAL_DISPLAY_FLAG_PUBLIC` and launch apps on them via `am start --display`. Confirmed working on Cuttlefish (Android 15, x86_64). Screenshot pipeline achieves ~14ms/62fps with JPEG encoding.

**Milestone**: LLM edits code → project bundler → coordinator push → reload → accessibility tree / describeScreen() → interact → screenshot. Full loop working. Validate the concept.

### Phase 2: Headless container runtime (the optimization)

Replace the stock Android runtime with a minimal headless container. Only start this after Phase 1 is validated.

**Build in this order:**

1. **Container setup** — LXC/namespace config using Waydroid image, bind-mount binder + render node
2. **Boot Android in container** — get Waydroid image booting with Waydroid's own HWComposer + weston headless (known-working baseline)
3. **Fork HWComposer** — strip Wayland, implement dummy display, test SurfaceFlinger starts
4. **Validate mesa/EGL** — confirm `EGL_PLATFORM=surfaceless` or `EGL_PLATFORM=gbm` works in container without Wayland
5. **Validate VirtualDisplay** — confirm ImageReader capture works inside the container
6. **GPU passthrough** — bind-mount render node, verify mesa detects GPU, compare rendering performance

Each step can be tested independently. If step 3 or 4 proves harder than expected (mesa EGL platform issues, SurfaceFlinger crashes), fall back to step 2 (keep weston headless — it works, costs 5MB).

### Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Shell user can't create VirtualDisplay with PUBLIC flag | Blocks coordinator architecture | Fall back to overlay displays (4-cap) or use hidden API workarounds; worst case, platform-sign a minimal helper APK |
| Per-display accessibility tree API not accessible from shell user | Blocks per-display a11y tree | Use CDP describeScreen() as primary; investigate UiAutomation API access from app_process |
| Dummy HWComposer crashes SurfaceFlinger (Phase 2) | Blocks Phase 2 | Fall back to Waydroid HWComposer + weston headless |
| Mesa needs Wayland EGL platform in container (Phase 2) | Blocks no-Wayland goal | Use `EGL_PLATFORM=surfaceless` or keep minimal weston |
| Gralloc buffer format mismatch with ImageReader | Blank screenshots | Debug buffer formats, try different ImageReader pixel formats |

### What each phase delivers

```
Phase 1 alone:
  ✓ LLMs can test their code changes
  ✓ Parallel instances via app ID variants (no 4-display cap — real VirtualDisplays)
  ✓ CDP + coordinator control
  ✓ Screenshots via ImageReader + JPEG (~14ms)
  ✓ Per-display accessibility tree (no CDP required for structured UI)
  ✓ edit→see cycle = bundle build + push + reload (hypothesis)
  ✗ Higher base memory (~500MB+ with stock emulator)
  ✗ Needs emulator/Waydroid running
  ✗ Bundle build time depends on project's bundler (not platform-controlled)

Phase 1 + Phase 2:
  ✓ Everything above, plus:
  ✓ ~200-300MB base memory target (container, no VM; hypothesis)
  ✓ No Wayland / no display server on host
  ✓ GPU optional via render node
  ✗ Control plane hardening deferred (known initial issue)
```

### Future: esbuild as platform-integrated bundler

Separate from the core platform. Investigate if/when specific projects need it.

esbuild could replace project-specific bundlers for faster, lower-memory builds (~200ms, ~30MB peak vs. Metro's ~200-500MB persistent). This is attractive at scale (many parallel LLMs) where Metro's per-worktree memory cost dominates. However:

- esbuild doesn't support all React Native transforms out of the box (Flow stripping, `.android.js` platform extensions, Hermes bytecode compilation)
- RN's Babel preset includes transforms that require a compatibility layer
- This is highly project-specific — what works for one RN project may break another

If pursued, this would involve:
1. Spike: validate esbuild with a representative RN app, identify missing transforms
2. Build an esbuild plugin for RN-specific transforms (platform extensions, asset resolution)
3. Optionally add Hermes bytecode compilation as a post-build step
4. Offer as an optional fast-path alongside the project's own bundler

This is an optimization, not a requirement. The platform works with any bundler that produces a `.jsbundle` file.

---

## Known Initial Issues (Accepted)

- Security hardening is deferred: coordinator API and CDP `Runtime.evaluate` are trusted-local only in v1.
- CDP integration is tightly coupled to a specific RN/Hermes version in v1.
- No CDP fallback path in v1 (but accessibility tree via coordinator provides structured UI without CDP).
- Coordinator running via `app_process` as shell user is an unconventional deployment — must validate VirtualDisplay + InputManager + accessibility API access early.
- Bundle build tooling is the project's responsibility, not the platform's. Build times vary by project and bundler choice.

## Non-Negotiable Android Components

These must be running in the container regardless of what is stripped:

1. **Binder driver** (kernel module on host) — Android IPC, everything uses it
2. **init** — PID 1, starts all services
3. **servicemanager** — Binder name registry
4. **Zygote** — Forks app processes, preloads ART + framework classes
5. **ART** (Android Runtime) — Executes DEX bytecode
6. **System Server** — Minimum: ActivityManager, PackageManager, WindowManager, InputManager, DisplayManager
7. **SurfaceFlinger** — Composites app surfaces, required even headless
8. **HWComposer HAL** — Display abstraction (custom dummy, ~250 lines C++)
9. **Gralloc HAL** — Graphics buffer allocation (mesa/GBM from Waydroid image, uses render node or llvmpipe)

Everything else (Bluetooth, WiFi, Telephony, Camera, Sensors, SystemUI, Launcher, Settings, Audio) can be removed or ignored.
