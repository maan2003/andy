# Emulator Notes

This file tracks the reference implementations and prior art for the Linux-only,
container-managed Android runtime work in Andy.

# References

| Local clone | Upstream | Commit | Why it matters |
| --- | --- | --- | --- |
| `tmp/waydroid` | `https://github.com/waydroid/waydroid` | `5e3725e86b13` | Main desktop-oriented Android-in-Linux-container implementation. Strong reference for namespace/container setup, image management, host integration, and Android lifecycle control. |
| `tmp/redroid-doc` | `https://github.com/remote-android/redroid-doc` | `b80d41c6370c` | Main cloud/multi-instance Android-in-container reference. Useful for Docker/Podman/Kubernetes deployment, boot properties, overlayfs sharing, ADB exposure, and GPU mode handling. |
| `tmp/redroid-modules` | `https://github.com/remote-android/redroid-modules` | `e8a5b009ecb2` | Kernel-side reference for binder and ashmem setup, DKMS packaging, and distro-specific module installation patterns. |
| `tmp/device_redroid` | `https://github.com/remote-android/device_redroid` | `f6b5d4f4ec2f` | ReDroid AOSP device/product definitions. Useful when we need to understand how a container-first Android image is configured at the product/device level. |
| `tmp/vendor_redroid` | `https://github.com/remote-android/vendor_redroid` | `1686c1825c14` | ReDroid shared vendor modules. Relevant for gralloc, binder allocation, init scripts, and other vendor-side runtime glue. |
| `tmp/anbox` | `https://github.com/anbox/anbox` | `ddf4c57ebbe3` | Historical ancestor for this whole approach. Still valuable for architecture study, especially host/container boundaries and the original non-VM Android-on-Linux design. |
| `tmp/anbox-modules` | `https://github.com/anbox/anbox-modules` | `98f0f3b3b1ee` | Historical kernel-module packaging reference. Good comparison point for binder/ashmem installation and permissions setup. |
| `tmp/android-emulator-container-scripts` | `https://github.com/google/android-emulator-container-scripts` | `beef3bca27ad` | Official Google "emulator in Docker" baseline. Not container-native Android like Waydroid/ReDroid, but very relevant for Linux-only managed-emulator ergonomics, KVM use, ADB access, and remote-control patterns. |
| `tmp/android-cuttlefish` | `https://github.com/google/android-cuttlefish` | `aebec8284583` | Official headless/CI-focused Android virtual device stack. Important adjacent prior art for orchestration, host preparation, container packaging, and non-interactive Android boot flows. |
| `tmp/docker-android` | `https://github.com/budtmo/docker-android` | `4fe7dfdd39ac` | Community packaging around the standard Android emulator. Useful for practical CI/test workflows, remote visibility, VNC/web access, and container UX tradeoffs. |

## Quick takeaways

- `waydroid` is the best reference for a Linux-desktop-integrated Android container.
- `redroid-doc` plus the `device_redroid` and `vendor_redroid` repos are the best references for a fully managed, multi-instance container runtime.
- `android-cuttlefish` and `android-emulator-container-scripts` are useful adjacent references for orchestration and headless Linux automation, even though they are VM/emulator based rather than container-native Android.
- `anbox` is older and archived, but it is still worth keeping around as design ancestry and for host/runtime boundary ideas.

## Provisional architecture decisions

These are the current design decisions for Andy's Android runtime. They are not
final: we will validate them with small prototypes and measurements before
locking the final architecture.

### Runtime model

- Andy will use a Linux-only Android container runtime, not a VM-based emulator.
- Andy will manage the runtime directly; users should not have to know an
  emulator/container is running under the hood.
- Andy will require the necessary host kernel support rather than trying to
  hide or emulate missing kernel features.

### Isolation model

- Andy will use one shared warm Android container rather than one container per
  LLM/worktree.
- Parallelism will come from many app instances inside that shared runtime, not
  from duplicating the full Android system.
- This keeps cold boot cost amortized and preserves the density goals from the
  earlier planning notes.

### Control plane

- The existing Andy coordinator remains the main in-Android control plane.
- Andy should continue to use the coordinator for display management, input,
  screenshots, accessibility, and app lifecycle control.
- ADB should be treated as bootstrap and recovery plumbing, not as the primary
  interface for normal runtime operation.

### Rendering model

- The rendering path should stay the same: apps render into Android
  `VirtualDisplay`s backed by `ImageReader` surfaces, and the coordinator reads
  those buffers directly.
- Screenshots, input, and accessibility integration should continue to be built
  around the current Andy infrastructure rather than introducing a separate
  emulator-specific interaction path.

### Headless operation

- The target is a fully headless runtime with no dependency on a user-running
  Wayland session on the host.
- This is still an area to validate. Waydroid's existing session-management
  stack assumes a Wayland socket, so Andy should not depend on that layer.
- We still expect Android to need some internal display/HWComposer path so that
  `SurfaceFlinger` can boot; the exact mechanism remains to be proven.

### Prior-art weighting

- `waydroid` remains the strongest reference for container integration and
  graphics/display-stack details.
- `redroid-doc` plus the related ReDroid repos remain the strongest references
  for a fully managed, headless, multi-instance Android container runtime.
- The likely end state is to borrow operational/runtime ideas from ReDroid and
  graphics/container Android ideas from Waydroid.

## Prototype and experiment areas

Before finalizing the runtime architecture, we should build small focused
prototypes to answer the remaining unknowns.

## Latest Rootless Findings

These notes capture the current state of the first real headless-host
experiment on this machine.

- The host remained truly headless for this run: `WAYLAND_DISPLAY` and
  `DISPLAY` were unset throughout.
- The host kernel support is good enough to keep pushing on container-native
  Android: binderfs, Android binder IPC, memfd, user namespaces, and IPv6 are
  available, and rootless Podman works from Nix.
- The host still lacks DMA-BUF heaps and a `/dev/dri/renderD128` render node,
  so this remains a software-rendering and non-GBM-validation environment for
  now.
- Stock ReDroid still does not survive rootless Podman startup on this host.
  It exits almost immediately and never exposes ADB, so the blocker is deeper
  than "no Wayland compositor".
- Android init really does parse every file in the scanned init directories.
  The AOSP init docs confirm that `/system/etc/init`, `/vendor/etc/init`, and
  the sibling init directories are imported file-by-file, so throwaway backup
  files inside those directories can change boot behavior.
- That backup-file hypothesis was real on this machine. Besides the earlier
  `*.andy-orig*` files, stale files such as
  `/system/etc/init/hw/init.rc.andy-pre-kptr` and
  `/system/etc/init/hw/init.rc.andy-pre-kptr-valid` were also being parsed and
  were enough to reintroduce old rootless failures after the visible live
  `init.rc` had already been patched.
- A throwaway patched image plus a manual bootstrap container can keep a small
  Android core alive without Wayland: `servicemanager`, `hwservicemanager`,
  `android.hardware.graphics.composer@2.1-service`, `surfaceflinger`,
  `android.hardware.graphics.allocator@2.0-service`, and `adbd` can all stay
  running together.
- `adbd` works in that manual bootstrap path only after adding an
  APEX-specific `/apex/com.android.adbd/etc/ld.config.txt` and an explicit
  `LD_LIBRARY_PATH`. The same pattern was also needed to make
  `derive_classpath` runnable from the `com.android.sdkext` APEX.
- Binder IPC is present but still fragile in the init-less path. After fixing
  binder device permissions to `0666`, `adb shell service list` works, but it
  still shows only the base service manager (`manager`).
- A short `/init ... || true` seed is enough to populate the property area with
  useful read-only properties such as `ro.product.cpu.abilist64` and
  `ro.property_service.version`, but it does not leave behind a functioning
  property service. The property socket path exists, yet there is no live
  listener, and `setprop` attempts from `hwservicemanager` and `adbd` fail with
  `errno=111 (Connection refused)`.
- Higher-level framework services are still missing from the manual bootstrap.
  `service check SurfaceFlinger`, `activity`, `package`, `window`, and
  `display` all report not found even while the corresponding native daemons
  stay alive as processes.
- `derive_classpath` successfully reconstructs `BOOTCLASSPATH`,
  `DEX2OATBOOTCLASSPATH`, and `SYSTEMSERVERCLASSPATH`, so missing classpath
  exports are no longer the leading zygote hypothesis.
- The current rootless/manual blocker is now narrower: zygote still aborts very
  early with `Runtime library not loaded.` even after property seeding,
  linkerconfig repair, allocator startup, and derived classpath exports. That
  points to an ART/native-runtime load problem in the init-less environment,
  not just missing Java-side classpath setup.
- The immediate remaining unknown is whether that zygote failure can be solved
  with a bounded linker/runtime fix, or whether it proves that Andy needs a
  longer-lived real `init` phase to preserve property service, linkerconfig,
  sockets, and other early-boot state before handing control back to the manual
  runtime.
- On the real `/init` path, the first confirmed rootless failure is now no
  longer a mystery in the rc files. After cleaning all `*.andy-*` backups from
  the scanned init directories, a fresh privileged `/init` trace still aborts
  on `/proc/sys/kernel/kptr_restrict`, but that write is coming from Android
  init's builtin `SetKptrRestrictAction` in `init/security.cpp`, not from the
  patched `on property:security.lower_kptr_restrict=*` rc actions.
- In other words, the current source-level `/init` blocker has narrowed again:
  rootless Podman on this host cannot satisfy Android init's early builtin
  attempt to raise `kptr_restrict`, so getting past this stage will require a
  bounded init-side workaround or patch rather than more rc-file surgery.
- A direct Podman file bind on `/proc/sys/kernel/kptr_restrict` does not solve
  that blocker under the current privileged rootless probe shape. Podman will
  accept the mount, and a normal shell in the container sees it as a writable
  regular file backed by the host path, but `/init` still reaches the real
  procfs sysctl and fails with `EACCES` in `SetKptrRestrictAction`.
- The trace explains why that external mask is ineffective: early in first
  stage, Android init mounts a fresh procfs on `/proc`
  (`mount("proc", "/proc", "proc", 0, "hidepid=2,gid=...")`) before it queues
  the builtin `SetKptrRestrictAction` path. So a container-manager bind under
  `/proc` is not a robust bypass for this failure.
- AOSP's second-stage init code also confirms that
  `SetKptrRestrictAction` is queued as an early builtin before the normal init
  event flow, immediately after `Service::OpenAndSaveStaticKallsymsFd()`. That
  makes an explicit init-side gate or opt-in non-fatal downgrade look cleaner
  than more container-level proc masking experiments.
- The current best next patch shape is still the narrow source-level one: gate
  `SetKptrRestrictAction` behind an explicit boot arg, or only downgrade
  `EACCES`/`EPERM` to non-fatal when that opt-in boot arg is present. Stock
  behavior stays unchanged when the arg is absent, and Andy gets a bounded
  rootless escape hatch for hosts where this host-global sysctl cannot be
  raised from a rootless container.
- A direct binary patch is also viable as a fast experiment tool. In this
  image, `/init` is just a symlink to `/system/bin/init`, and the stripped PIE
  binary still has a clean local branch after the helper that tries to set
  `/proc/sys/kernel/kptr_restrict`. NOPing the `je` failure branch at that site
  was enough to skip the known `kptr_restrict` fatal path without needing to
  understand the full C++ `Result<void>` return ABI.
- That binary patch should still be treated as a probe aid, not the long-term
  maintenance answer. It is useful for quickly exposing the next rootless init
  blockers and for validating that a source-level init fork would likely work,
  but it is too brittle to become Andy's durable runtime strategy.
- Fresh bind-mounted patched-init probes in `andy-redroid-sh-probe10bin`,
  `andy-redroid-init-probe10bin`, `andy-redroid-sh-probe11bin`, and
  `andy-redroid-init-probe11bin` confirm that the `kptr_restrict` guard is no
  longer the first fatal point. The shell-run patched trace advances well into
  second-stage service startup and, on the fresh `11bin` repro, exits with
  status `6` in a way that matches the trace; the earlier `10bin` shell exit
  file that showed `0` now looks more like a bad capture than a clean success.
  The real PID 1 patched run still exits almost immediately with status `129`,
  so there is at least one later real init blocker after the `kptr_restrict`
  bypass.
- The patched shell trace exposes the next confirmed init-side fatal path after
  `kptr_restrict`: main `init` later aborts immediately after
  `openat("/proc/sys/vm/mmap_rnd_bits", O_RDONLY) = -1 EACCES`, and the binary
  still contains the nearby embedded fatal string `Unable to set adequate mmap
  entropy value!`. That makes the mmap-entropy guard in Android init's
  security setup the next narrowed rootless blocker on this host.
- A normal shell in the same privileged rootless container shape confirms why
  this is a distinct proc-sysctl boundary from `kptr_restrict`: inside the
  container, `/proc/sys/kernel/kptr_restrict` is mode `0644` and readable, but
  `/proc/sys/vm/mmap_rnd_bits` and `/proc/sys/vm/mmap_rnd_compat_bits` are mode
  `0600` and unreadable even to container root under the user-namespace
  mapping (`overflowuid`). So bypassing the `kptr_restrict` write alone does
  not remove the next host-global sysctl permission failure.
- The same patched shell trace also surfaced a separate earlier service-side
  incompatibility: `ueventd` aborts when its
  `uevent_socket_rcvbuf_size 16M` setup reaches
  `setsockopt(..., SO_RCVBUFFORCE, ...) = -1 EPERM`. Init notices that
  `SIGABRT`, marks `ueventd` restarting, and still runs far enough to hit the
  later `mmap_rnd_bits` fatal guard. So the `ueventd` receive-buffer forcing is
  a real rootless problem, but it does not appear to be the first remaining
  init-wide fatal barrier once the `kptr_restrict` branch is patched out.

### Headless boot path

- Can Andy boot the chosen containerized Android image with no host Wayland
  server at all?
- If not immediately, what is the smallest temporary bootstrap display path
  that still preserves the long-term headless design?
- Can a minimal dummy HWComposer path replace any Wayland-dependent boot path
  without destabilizing `SurfaceFlinger`?

### Base image/runtime choice

- Which starting point is more practical for Andy's managed runtime work:
  a Waydroid-derived image path, a ReDroid-style image path, or a hybrid?
- How much existing runtime glue can be reused versus replaced cleanly by Andy?

### Performance and density

- Measure cold boot time for the shared runtime.
- Measure warm app-instance launch time inside the shared runtime.
- Measure memory overhead for the shared base and for each additional app
  instance.
- Measure software-rendering behavior when no GPU render node is available.

### Coordinator integration

- Re-confirm the coordinator assumptions inside the containerized runtime:
  `VirtualDisplay`, `ImageReader`, input injection, and per-display
  accessibility access.
- Confirm that the current Andy interaction model can stay unchanged from the
  user's point of view while the runtime becomes fully managed internally.
