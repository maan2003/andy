# Device Setup

## Background services

These should be running before proceeding. Launch each as a background task using the Bash tool with `run_in_background: true`.

### Cuttlefish

```
env -C ../cuttlefish-nix2 ./result/bin/launch_cvd --enable_tap_devices=false --adb_mode=native_vsock
```

### Remote bridge

```
env -C ../fedi direnv exec . ./scripts/bridge/run-remote.sh --with-devfed
```

### Metro bundler

```
env -C ../fedi direnv exec . env -C ui/native yarn start
```

## Device connection

Connect ADB to the Cuttlefish instance:

```
adb connect vsock:3:5555
```

## Enable airplane mode

```
adb shell cmd connectivity airplane-mode enable
```

## Installing Fedi

The APK is at `../fedi/ui/native/android/app/build/outputs/apk/production/debug/app-production-debug.apk`. Install with:

```
adb install ../fedi/ui/native/android/app/build/outputs/apk/production/debug/app-production-debug.apk
```

### Setting metro host

Set the metro host globally via a system property:

```
adb shell su 0 setprop metro.host localhost
```

Then set up adb reverse so the device can reach the host's metro server:

```
adb reverse tcp:8081 tcp:8081
adb reverse tcp:26722 tcp:26722
```
