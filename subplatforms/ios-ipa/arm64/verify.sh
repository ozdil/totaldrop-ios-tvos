#!/bin/sh
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Isolated sub-platform artifact & provenance verifier for `apple-mobile-ios-ipa-arm64` (ADR 0020).

set -eu

UNIT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$UNIT_DIR/../../.." && pwd)
RECIPE="$UNIT_DIR/RECIPE.toml"
WORKFLOW="$REPO_ROOT/.github/workflows/pkg-ios-ipa-arm64.yml"

if [ "${1:-}" = "--check-recipe" ]; then
  [ -f "$RECIPE" ] || { echo "missing $RECIPE" >&2; exit 1; }
  [ -f "$WORKFLOW" ] || { echo "missing workflow $WORKFLOW" >&2; exit 1; }
  sh -n "$UNIT_DIR/build.sh"
  sh -n "$UNIT_DIR/verify.sh"
  grep -q 'apple-mobile-ios-ipa-arm64-v\*' "$WORKFLOW"
  echo "verify check ok: apple-mobile-ios-ipa-arm64"
  exit 0
fi

ART_DIR="${1:?usage: verify.sh [--check-recipe | <artifact-dir>]}"
[ -f "$ART_DIR/TotalDrop-iOS-0.3.0-beta.6-arm64.ipa" ] || { echo "missing artifact $ART_DIR/TotalDrop-iOS-0.3.0-beta.6-arm64.ipa" >&2; exit 1; }
[ -s "$ART_DIR/TotalDrop-iOS-0.3.0-beta.6-arm64.ipa" ] || { echo "empty artifact $ART_DIR/TotalDrop-iOS-0.3.0-beta.6-arm64.ipa" >&2; exit 1; }
[ -f "$ART_DIR/SHA256SUMS" ] || { echo "missing $ART_DIR/SHA256SUMS" >&2; exit 1; }
(cd "$ART_DIR" && sha256sum -c SHA256SUMS)
echo "artifact verified: $ART_DIR/TotalDrop-iOS-0.3.0-beta.6-arm64.ipa"
