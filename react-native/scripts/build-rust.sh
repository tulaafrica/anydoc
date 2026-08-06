#!/bin/bash
# Builds the anydoc Rust core for the Android device ABIs and drops the
# static libraries where the CMake build expects them.
#
# Requirements: rustup targets aarch64-linux-android + armv7-linux-androideabi,
# and ANDROID_NDK_HOME (or the default SDK location below).
set -euo pipefail
cd "$(dirname "$0")/.."

NDK="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/27.1.12297006}"
BIN="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"

export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$BIN/aarch64-linux-android24-clang"
export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="$BIN/armv7a-linux-androideabi24-clang"

for target in aarch64-linux-android armv7-linux-androideabi; do
  case $target in
    aarch64-linux-android) abi=arm64-v8a ;;
    armv7-linux-androideabi) abi=armeabi-v7a ;;
  esac
  echo "==> $target ($abi)"
  (cd .. && cargo build --release -p anydoc-mobile --target $target)
  mkdir -p android/libs/$abi
  cp ../target/$target/release/libanydoc_mobile.a android/libs/$abi/
done
echo "done: $(du -sh android/libs/*/libanydoc_mobile.a)"
