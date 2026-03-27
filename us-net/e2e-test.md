# us-net Cuttlefish E2E Status

## Goal

Validate `us-net/` end to end with Cuttlefish on Linux without using OpenWRT or vfkit:

- inject a direct `vhost-user-net` device into the main Android `crosvm` VM
- back it with `us-net`
- connect `us-net` to `gvproxy -listen-qemu`

## Final Status

This works on Linux.

What is proven:

- `vfkit` is not needed on Linux.
- `gvproxy -listen-vfkit` is not usable on Linux upstream; the working transport is `gvproxy -listen-qemu`.
- OpenWRT can be bypassed for this test by disabling the AP path and injecting `--vhost-user=net,...` into the main Android VM.
- Android boots successfully with that direct net device.
- With `ro.vendor.disable_rename_eth0=true` present in the guest vendor properties, Android keeps the injected NIC as `eth0`.
- With that property set, DHCP succeeds and the guest has external connectivity without any post-boot rename.

What is still awkward:

- `assemble_cvd` is not the layer that creates this behavior; it consumes already-built guest images.
- For guest reboot testing, `gvproxy` and `us-net` currently need simple supervision because both exit when the guest disconnects.

## Architecture Used

Working path:

```text
main Android VM (crosvm)
  -> injected vhost-user-net device
  -> us-net
  -> gvproxy -listen-qemu
  -> host network
```

Linux event model:

- guest queue kicks are handled by the vhost-user worker thread
- the `gvproxy` socket is handled as a normal readiness source (`EPOLLIN`/`EPOLLOUT`)
- there is no periodic poll thread or synthetic wakeup path

Not used in this successful path:

- OpenWRT sidecar VM
- vfkit transport

## Guest Image Fix Point

The `rename_eth0` behavior is defined by the Android guest image, not by `assemble_cvd`.

What was verified:

- `/vendor/etc/init/init.cutf_cvm.rc` comes from the guest vendor image.
- `assemble_cvd` and `launch_cvd` bootconfig changes are not enough on this image. `ro.boot.disable_rename_eth0=1` and `ro.boot.vendor.disable_rename_eth0=1` do not stop the trigger because the init action checks `ro.vendor.disable_rename_eth0`.
- Patching `vendor_boot` properties alone was also not sufficient in this environment.
- The clean build-time fix is to add `ro.vendor.disable_rename_eth0=true` to the guest vendor properties so the existing init trigger does not fire.

That means the right source-side fix is in the Android product/device build that generates the vendor partition, not in `assemble_cvd`.

## Required Local Changes

### us-net

`us-net` was changed to match this Linux direct-net path:

- use the qemu stream transport to `gvproxy`
- advertise `VhostUserProtocolFeatures::MQ`
- use `virtio-queue` `Reader`/`Writer` helpers
- only run the `EVENT_IDX` drain loop for TX
- register the `gvproxy` stream fd directly with the vhost-user epoll worker
- handle TX retry via deferred frame state plus `EPOLLOUT`, not a synthetic timer
- treat guest queue kicks as "guest queue state changed" and drain TX plus deferred RX without depending on a fragile queue-event ordering assumption

Current files:

- `/home/maan2003/src/tmp/us-net/src/backend.rs`
- `/home/maan2003/src/tmp/us-net/src/main.rs`

### crosvm wrapper

The wrapper at:

- `/home/maan2003/src/tmp/tmp/cf-net-test/crosvm-log-wrapper.sh`

does all of the following:

- logs `crosvm` argv
- only rewrites the main Android `run` invocation
- injects:

```text
--vhost-user=net,socket=/tmp/cf-usnet.sock,max-queue-size=256
```

- leaves non-`run` subcommands alone so `stop_cvd` and helper commands are not corrupted

## Launch Notes

The packaged Cuttlefish build on this host requires one extra workaround:

- `HostSupportsQemuCli()` checks either `/manager.sock` or `/usr/lib/cuttlefish-common/bin/capability_query.py`
- creating `/usr/lib/...` is not possible as an unprivileged user here
- running `launch_cvd` inside `bwrap` with a synthetic `/manager.sock` makes Cuttlefish consider the host "sandboxed"

`avbtool` also shells out to `openssl`, so `openssl` must be on `PATH` inside that `bwrap` environment.

## Working Host Processes

Start `gvproxy`:

```bash
while true; do
  /nix/store/srba60lrabswnj3h9vjb2mp27j33lh64-gvproxy-0.8.7/bin/gvproxy \
    -debug \
    -listen unix:///tmp/cf-gvproxy-api.sock \
    -listen-qemu unix:///tmp/cf-gvproxy.sock
  sleep 1
done
```

Start `us-net`:

```bash
while true; do
  /home/maan2003/src/tmp/target/debug/us-net \
    --socket /tmp/cf-usnet.sock \
    --gvproxy /tmp/cf-gvproxy.sock
  sleep 1
done
```

Launch Cuttlefish:

```bash
bwrap \
  --tmpfs / \
  --dir /nix --bind /nix /nix \
  --dir /home --bind /home /home \
  --dir /tmp --bind /tmp /tmp \
  --dir /run --bind /run /run \
  --dir /dev --dev-bind /dev /dev \
  --proc /proc \
  --dir /sys --bind /sys /sys \
  --dir /etc --bind /etc /etc \
  --dir /var --bind /var /var \
  --chdir /home/maan2003/cuttlefish-images \
  -- /nix/store/5p86w1968gs5abgqkj9wv5gccxpy253c-bash-interactive-5.3p3/bin/bash -lc '
    touch /manager.sock
    export PATH=/nix/store/1znd05g5hidxwwzm7kpiykxqfrjg7wca-openssl-3.6.1-bin/bin:$PATH
    env -u LD_PRELOAD -u LD_LIBRARY_PATH \
      /nix/store/6px0ifbwkb78gbcmrw3fd4v01nwziaqc-android-cuttlefish-14818820/bin/launch_cvd \
      -daemon \
      -report_anonymous_usage_stats=n \
      --system_image_dir=/home/maan2003/cuttlefish-images \
      --enable_tap_devices=false \
      --adb_mode=native_vsock \
      --crosvm_binary=/home/maan2003/src/tmp/tmp/cf-net-test/crosvm-log-wrapper.sh \
      --enable_wifi=false \
      --ap_kernel_image="" \
      --ap_rootfs_image="" \
      --enable_modem_simulator=false
  '
```

## Verified Guest Property Fix

For proof on the live userdebug guest, the following was sufficient:

```bash
adb root
adb wait-for-device
adb remount
adb push /home/maan2003/src/tmp/tmp/vendor-a-root/build.prop /vendor/build.prop
adb reboot
```

## Verified Result

Observed after reboot with the original `start rename_eth0` line restored and only `ro.vendor.disable_rename_eth0=true` present:

- `eth0` is `UP,LOWER_UP`
- `buried_eth0` is not present
- `getprop ro.vendor.disable_rename_eth0` returns `true`
- DHCP lease: `192.168.127.3/24`
- DNS server: `192.168.127.1`
- default route via `192.168.127.1`
- `ping 192.168.127.1` succeeds
- `ping google.com` succeeds

Example values from the successful run:

```text
eth0    inet 192.168.127.3/24
route   192.168.127.0/24 dev eth0
dns     192.168.127.1
ping    192.168.127.1 ok
ping    google.com ok
```

`gvproxy` also showed the expected DHCP, DNS, ARP, and ICMP traffic on this run.

## Remaining Follow-up

The direct-main-VM transport itself is working.

The clean permanent fix is to set `ro.vendor.disable_rename_eth0=true` in the Android guest build that generates the vendor partition used by Cuttlefish. `assemble_cvd` should consume that image; it is not the right layer to synthesize this behavior after the fact.
