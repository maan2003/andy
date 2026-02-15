# Agent Runtime Notes

## General

Clone any reference repos into `tmp/` when needed to understand APIs or implementations.

## Project structure

- `device/` — Device-side Java + Rust JNI (runs on Android via `app_process`)
  - `device/java/` — Java sources (`Main.java`, `Wrappers.java`)
  - `device/src/` — Rust cdylib (JNI entry point + in-device axum HTTP API on Unix socket)
  - `device/build.sh` — Builds Java jar + copies native `.so` into `device/build/`
- `andy-cli/` — Host-side Rust CLI client for the forwarded Unix socket API (binary: `andy`)
  - `andy start` subcommand pushes artifacts, configures `adb forward`, and starts coordinator

## Building

### Full build (Java + Rust native)

```bash
./device/build.sh
```

This produces `device/build/` containing both `coordinator-server.jar` and `libcoordinator.so`.

## Running the coordinator

```bash
cargo run -p andy-cli -- start
```

Artifacts (`.jar` + `.so` files) are embedded in the binary at compile time. For local dev, run `./device/build.sh` first so `device/build/` has the artifacts. For nix, the flake passes artifact paths via env vars.

Socket: `~/.local/state/andy.sock`

## Running andy-cli

After starting the coordinator with `andy start`:

```bash
cargo run -p andy-cli -- <command>
```

Examples:

```bash
# Take a screenshot
cargo run -p andy-cli -- screenshot /tmp/screen.png

# Print accessibility tree
cargo run -p andy-cli -- a11y

# Tap by coordinates or accessibility text
cargo run -p andy-cli -- tap 540,960
cargo run -p andy-cli -- tap "Login"

# Launch a package
ANDY_PACKAGE=com.fedi cargo run -p andy-cli -- launch
```

Socket is always `~/.local/state/andy.sock`.

## Device setup

When asked to set up the device, follow the instructions in [device_setup.md](device_setup.md).
