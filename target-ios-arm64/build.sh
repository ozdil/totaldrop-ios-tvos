#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Physical iPhone & iPad (aarch64-apple-ios) build unit for TotalDrop on iOS/iPadOS.
# Produces libtotaldrop_apple_mobile_bridge.a and validates Swift 6 device slice.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUST_TARGET="aarch64-apple-ios"

export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
export IPHONEOS_DEPLOYMENT_TARGET="18.0"

echo "==> [ios-arm64] Building Rust static library (${RUST_TARGET})"
rustup target add "${RUST_TARGET}"
cargo build \
  --manifest-path "${ROOT_DIR}/Cargo.toml" \
  --package totaldrop-apple-mobile-bridge \
  --target "${RUST_TARGET}" \
  --release \
  --locked

SDK_PATH="$(xcrun --sdk iphoneos --show-sdk-path)"
echo "==> [ios-arm64] Compiling Swift 6 TotalDropIOS & ShareExtension against iphoneos SDK"
swift build \
  --package-path "${ROOT_DIR}/ui-swiftui-ios" \
  --sdk "${SDK_PATH}" \
  --triple "arm64-apple-ios18.0" \
  --configuration release

echo "==> [ios-arm64] Static library ready at ${ROOT_DIR}/target/${RUST_TARGET}/release/libtotaldrop_apple_mobile_bridge.a"
