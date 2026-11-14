#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Assembles `TotalDropAppleMobileBridge.xcframework` combining:
# - iOS physical devices (`aarch64-apple-ios`)
# - iOS universal simulator (`aarch64-apple-ios-sim` + `x86_64-apple-ios` via `lipo`)
# - tvOS physical Apple TV 4K (`aarch64-apple-tvos`)
# - tvOS Apple Silicon simulator (`aarch64-apple-tvos-sim`)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
DIST_DIR="${ROOT_DIR}/dist"
HEADERS_DIR="${DIST_DIR}/include"
XCFRAMEWORK_OUT="${DIST_DIR}/TotalDropAppleMobileBridge.xcframework"
LIB_NAME="libtotaldrop_apple_mobile_bridge.a"

"${ROOT_DIR}/target-ios-arm64/build.sh"
"${ROOT_DIR}/target-ios-sim-arm64/build.sh"
"${ROOT_DIR}/target-ios-sim-x86_64/build.sh"
"${ROOT_DIR}/target-tvos-arm64/build.sh"

mkdir -p "${DIST_DIR}/ios-sim-universal" "${HEADERS_DIR}"

echo "==> Merging iOS simulator arm64 + x86_64 slices with lipo"
lipo -create \
  -output "${DIST_DIR}/ios-sim-universal/${LIB_NAME}" \
  "${ROOT_DIR}/target/aarch64-apple-ios-sim/release/${LIB_NAME}" \
  "${ROOT_DIR}/target/x86_64-apple-ios/release/${LIB_NAME}"

# REUSE-IgnoreStart
cat > "${HEADERS_DIR}/TotalDropAppleMobileBridge.h" <<'EOF'
// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef TOTALDROP_APPLE_MOBILE_BRIDGE_H
#define TOTALDROP_APPLE_MOBILE_BRIDGE_H
#include <stdint.h>
#endif
EOF
# REUSE-IgnoreEnd

cat > "${HEADERS_DIR}/module.modulemap" <<'EOF'
module TotalDropAppleMobileBridge {
    header "TotalDropAppleMobileBridge.h"
    export *
}
EOF

rm -rf "${XCFRAMEWORK_OUT}"
xcodebuild -create-xcframework \
  -library "${ROOT_DIR}/target/aarch64-apple-ios/release/${LIB_NAME}" \
  -headers "${HEADERS_DIR}" \
  -library "${DIST_DIR}/ios-sim-universal/${LIB_NAME}" \
  -headers "${HEADERS_DIR}" \
  -library "${ROOT_DIR}/target/aarch64-apple-tvos/release/${LIB_NAME}" \
  -headers "${HEADERS_DIR}" \
  -library "${ROOT_DIR}/target/aarch64-apple-tvos-sim/release/${LIB_NAME}" \
  -headers "${HEADERS_DIR}" \
  -output "${XCFRAMEWORK_OUT}"

echo "==> Created ${XCFRAMEWORK_OUT}"
