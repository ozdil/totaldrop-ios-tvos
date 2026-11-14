#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Apple Silicon iOS Simulator (aarch64-apple-ios-sim) build and test unit
# for account-free GitHub macOS ARM runners (`macos-15`, ADR 0009 steps 1 & 3).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUST_TARGET="aarch64-apple-ios-sim"

export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
export IPHONEOS_DEPLOYMENT_TARGET="18.0"

echo "==> [ios-sim-arm64] Building Rust static library (${RUST_TARGET})"
rustup target add "${RUST_TARGET}"
cargo build \
  --manifest-path "${ROOT_DIR}/Cargo.toml" \
  --package totaldrop-apple-mobile-bridge \
  --target "${RUST_TARGET}" \
  --release \
  --locked

SIM_SDK_PATH="$(xcrun --sdk iphonesimulator --show-sdk-path)"
echo "==> [ios-sim-arm64] Compiling Swift 6 TotalDropIOS for iphonesimulator (arm64)"
swift build \
  --package-path "${ROOT_DIR}/ui-swiftui-ios" \
  --sdk "${SIM_SDK_PATH}" \
  --triple "arm64-apple-ios18.0-simulator" \
  --configuration release

echo "==> [ios-sim-arm64] Ready at ${ROOT_DIR}/target/${RUST_TARGET}/release/libtotaldrop_apple_mobile_bridge.a"
