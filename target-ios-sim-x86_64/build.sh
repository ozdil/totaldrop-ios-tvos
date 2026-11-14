#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Intel iOS Simulator (x86_64-apple-ios) build unit for universal simulator XCFrameworks.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUST_TARGET="x86_64-apple-ios"

export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
export IPHONEOS_DEPLOYMENT_TARGET="18.0"

echo "==> [ios-sim-x86_64] Building Rust static library (${RUST_TARGET})"
rustup target add "${RUST_TARGET}"
cargo build \
  --manifest-path "${ROOT_DIR}/Cargo.toml" \
  --package totaldrop-apple-mobile-bridge \
  --target "${RUST_TARGET}" \
  --release \
  --locked

SIM_SDK_PATH="$(xcrun --sdk iphonesimulator --show-sdk-path)"
echo "==> [ios-sim-x86_64] Compiling Swift 6 TotalDropIOS for iphonesimulator (x86_64)"
swift build \
  --package-path "${ROOT_DIR}/ui-swiftui-ios" \
  --sdk "${SIM_SDK_PATH}" \
  --triple "x86_64-apple-ios18.0-simulator" \
  --configuration release

echo "==> [ios-sim-x86_64] Ready at ${ROOT_DIR}/target/${RUST_TARGET}/release/libtotaldrop_apple_mobile_bridge.a"
