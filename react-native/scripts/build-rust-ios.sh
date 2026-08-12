#!/bin/bash
# Builds the anydoc Rust core for iOS (device + simulator) and wraps both
# slices in an XCFramework where the podspec expects it.
#
# Requirements: rustup targets aarch64-apple-ios + aarch64-apple-ios-sim,
# and Xcode command line tools (xcodebuild).
set -euo pipefail
cd "$(dirname "$0")/.."

for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  echo "==> $target"
  (cd .. && cargo build --release -p anydoc-mobile --target $target)
done

rm -rf ios/AnydocCore.xcframework
mkdir -p ios
xcodebuild -create-xcframework \
  -library ../target/aarch64-apple-ios/release/libanydoc_mobile.a \
  -library ../target/aarch64-apple-ios-sim/release/libanydoc_mobile.a \
  -output ios/AnydocCore.xcframework
echo "done: $(du -sh ios/AnydocCore.xcframework | cut -f1)"
