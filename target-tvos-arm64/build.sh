#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Apple tvOS 4K device (aarch64-apple-tvos) and Apple Silicon tvOS Simulator
# (aarch64-apple-tvos-sim) build unit.
#
# Note: Apple tvOS targets are Tier 3 in upstream rustc, so `rust-src` +
# `-Z build-std=std,panic_abort` is used when prebuilt std components are absent.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

export TVOS_DEPLOYMENT_TARGET="18.0"

build_tvos_slice() {
  local target="$1"
  echo "==> [tvos] Building Rust static library (${target})"
  if RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}" rustup target add "${target}" 2>/dev/null; then
    RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}" cargo build \
      --manifest-path "${ROOT_DIR}/Cargo.toml" \
      --package totaldrop-apple-mobile-bridge \
      --target "${target}" \
      --release \
      --locked
  else
    # Tier-3 fallback using rust-src for stdlib synthesis on tvOS.
    RUSTUP_TOOLCHAIN="${TVOS_BUILD_STD_TOOLCHAIN:-nightly}" cargo build \
      -Z build-std=std,panic_abort \
      --manifest-path "${ROOT_DIR}/Cargo.toml" \
      --package totaldrop-apple-mobile-bridge \
      --target "${target}" \
      --release \
      --locked
  fi
}

build_tvos_slice "aarch64-apple-tvos"
build_tvos_slice "aarch64-apple-tvos-sim"

TV_SDK_PATH="$(xcrun --sdk appletvos --show-sdk-path)"
TV_SIM_SDK_PATH="$(xcrun --sdk appletvsimulator --show-sdk-path)"

echo "==> [tvos] Compiling Swift 6 TotalDropTV for appletvos and appletvsimulator"
swift build \
  --package-path "${ROOT_DIR}/ui-swiftui-tvos" \
  --sdk "${TV_SDK_PATH}" \
  --triple "arm64-apple-tvos18.0" \
  --configuration release

swift build \
  --package-path "${ROOT_DIR}/ui-swiftui-tvos" \
  --sdk "${TV_SIM_SDK_PATH}" \
  --triple "arm64-apple-tvos18.0-simulator" \
  --configuration release
