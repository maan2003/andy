#!/usr/bin/env bash
set -euo pipefail

ANDROID_SDK_ROOT="${ANDROID_SDK_ROOT:-}"
ANDROID_JAR="${ANDROID_SDK_ROOT}/platforms/android-32/android.jar"
D8="${ANDROID_SDK_ROOT}/build-tools/32.0.0/d8"
SRC_DIR="java"
BUILD_DIR="build"
DEX_DIR="$BUILD_DIR/dex"
OUTPUT_JAR="$BUILD_DIR/coordinator-server.jar"

cd "$(dirname "$0")"

# Build Rust native library for both Android architectures
cargo build -p coordinator --target x86_64-linux-android --release
cargo build -p coordinator --target aarch64-linux-android --release

# Resolve the cargo target directory
CARGO_TARGET=$(cargo metadata --format-version 1 --no-deps 2>/dev/null | jq -r '.target_directory' 2>/dev/null || true)
CARGO_TARGET="${CARGO_TARGET:-../target}"

resolve_so() {
  local triple="$1"
  local so="$CARGO_TARGET/pkg/coordinator/${triple}/release/libcoordinator.so"
  if [[ ! -f "$so" ]]; then
    so="$CARGO_TARGET/${triple}/release/libcoordinator.so"
  fi
  echo "$so"
}

NATIVE_SO_X86_64="$(resolve_so x86_64-linux-android)"
NATIVE_SO_AARCH64="$(resolve_so aarch64-linux-android)"

if [[ ! -f "$ANDROID_JAR" ]]; then
  if [[ -z "$ANDROID_SDK_ROOT" ]]; then
    echo "error: ANDROID_SDK_ROOT is not set" >&2
  fi
  echo "error: android.jar not found at $ANDROID_JAR" >&2
  exit 1
fi

mapfile -t JAVA_SOURCES < <(find "$SRC_DIR" -name '*.java' | sort)
if [[ ${#JAVA_SOURCES[@]} -eq 0 ]]; then
  echo "error: no java sources found under $SRC_DIR" >&2
  exit 1
fi

rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/classes" "$DEX_DIR"

# Compile Java -> .class
javac -source 11 -target 11 \
  -cp "$ANDROID_JAR" \
  -d "$BUILD_DIR/classes" \
  "${JAVA_SOURCES[@]}"

mapfile -t CLASS_FILES < <(find "$BUILD_DIR/classes" -name '*.class' | sort)

# Convert .class -> .dex
"$D8" --lib "$ANDROID_JAR" \
   --output "$DEX_DIR" \
   "${CLASS_FILES[@]}"

# Package dex as a single deployable jar for app_process CLASSPATH
jar --create --file "$OUTPUT_JAR" -C "$DEX_DIR" classes.dex

# Clean up intermediate files so build/ only contains deployable artifacts
rm -rf "$BUILD_DIR/classes" "$DEX_DIR"

# Copy native .so into build dir with arch suffix
for arch_so in "x86_64:$NATIVE_SO_X86_64" "aarch64:$NATIVE_SO_AARCH64"; do
  arch="${arch_so%%:*}"
  so="${arch_so#*:}"
  if [[ -f "$so" ]]; then
    cp "$so" "$BUILD_DIR/libcoordinator-${arch}.so"
  else
    echo "warning: .so not found for $arch at $so" >&2
  fi
done
echo "Built: $BUILD_DIR/"
