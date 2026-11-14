#!/bin/sh
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Isolated sub-platform build entry point for `apple-mobile-tvos-ipa-sim-x86_64` (ADR 0020).

set -eu

UNIT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$UNIT_DIR/../../.." && pwd)
RECIPE="$UNIT_DIR/RECIPE.toml"
DELEGATE="$REPO_ROOT/target-tvos-arm64/build-xcframework.sh"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1760000000}"

if [ "${1:-}" = "--check-recipe" ]; then
  [ -f "$RECIPE" ] || { echo "missing $RECIPE" >&2; exit 1; }
  [ -e "$DELEGATE" ] || { echo "missing upstream delegate $DELEGATE" >&2; exit 1; }
  grep -q 'unit_id = "apple-mobile-tvos-ipa-sim-x86_64"' "$RECIPE"
  grep -q 'artifact_name = "TotalDrop-tvOS-Sim-0.3.0-beta.6-x86_64.tar.gz"' "$RECIPE"
  echo "recipe check ok: apple-mobile-tvos-ipa-sim-x86_64 -> TotalDrop-tvOS-Sim-0.3.0-beta.6-x86_64.tar.gz"
  exit 0
fi

if [ "${1:-}" = "--package-staged" ]; then
  STAGE_IN="${2:?missing staging input directory}"
  OUT_DIR="${3:?missing output directory}"
  mkdir -p "$OUT_DIR"
  stage_root=$(mktemp -d)
  trap 'rm -rf "$stage_root"' EXIT
  cp -R "$STAGE_IN/." "$stage_root/" 2>/dev/null || true
  printf 'unit=%s\narch=%s\nformat=%s\ntarget=%s\n' "apple-mobile-tvos-ipa-sim-x86_64" "sim-x86_64" "tvos-ipa" "x86_64-apple-tvos" >"$stage_root/BUILD_METADATA.txt"
  tar --sort=name --mtime="@${SOURCE_DATE_EPOCH:-1760000000}" --owner=0 --group=0 --numeric-owner -cf - -C "$stage_root" . | gzip -n >"$OUT_DIR/TotalDrop-tvOS-Sim-0.3.0-beta.6-x86_64.tar.gz"
  (cd "$OUT_DIR" && sha256sum "TotalDrop-tvOS-Sim-0.3.0-beta.6-x86_64.tar.gz" >SHA256SUMS)
  echo "packaged apple-mobile-tvos-ipa-sim-x86_64 -> $OUT_DIR/TotalDrop-tvOS-Sim-0.3.0-beta.6-x86_64.tar.gz"
  exit 0
fi

OUT_DIR="${1:-$REPO_ROOT/dist/tvos-ipa/sim-x86_64}"
mkdir -p "$OUT_DIR"

case "$DELEGATE" in
  *.sh)
    sh "$DELEGATE"
    ;;
  *.ps1)
    pwsh -NoProfile -File "$DELEGATE"
    ;;
  *)
    echo "Verified delegate configuration: $DELEGATE (Rust target: x86_64-apple-tvos)"
    ;;
esac
