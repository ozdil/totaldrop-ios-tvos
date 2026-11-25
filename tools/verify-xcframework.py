#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
# SPDX-License-Identifier: GPL-3.0-or-later

"""Structural and native binary verifier for `TotalDropAppleMobileFFI.xcframework`.

Validates:
  - XCFramework `Info.plist` format (`CFBundlePackageType == XFWK`,
    `XCFrameworkFormatVersion == 1.0`)
  - Required library slices (`ios-arm64`, `ios-arm64_x86_64-simulator`, plus
    optional `tvos-arm64` / `tvos-arm64-simulator` when `--require-tvos` is set)
  - Exact byte parity of every slice's `Headers/totaldrop_apple_mobile_ffi.h`
    with `crates/totaldrop-apple-mobile-ffi/include/totaldrop_apple_mobile_ffi.h`
  - Presence of `Headers/module.modulemap` declaring `TotalDropAppleMobileFFI`
  - Optional `--check-native` (macOS): `lipo -archs` per slice and `nm -gU`
    symbol verification for core `totaldrop_mobile_*` C ABI exports
  - `--self-test` mode runnable on Linux and macOS CI runners

Usage:
  python3 tools/verify-xcframework.py --self-test
  python3 tools/verify-xcframework.py --xcframework dist/TotalDropAppleMobileFFI.xcframework --check-native
"""

from __future__ import annotations

import argparse
import plistlib
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CANONICAL_HEADER = (
    REPO_ROOT / "crates" / "totaldrop-apple-mobile-ffi" / "include" / "totaldrop_apple_mobile_ffi.h"
)
LIB_NAME = "libtotaldrop_apple_mobile_ffi.a"

REQUIRED_EXPORTED_SYMBOLS: tuple[str, ...] = (
    "totaldrop_mobile_string_free",
    "totaldrop_mobile_version_json",
    "totaldrop_mobile_ui_bundle_json",
    "totaldrop_mobile_handle_url_json",
    "totaldrop_mobile_extract_clipboard_code_json",
    "totaldrop_mobile_voiceover_code_json",
    "totaldrop_mobile_parse_share_manifest_json",
    "totaldrop_mobile_parse_checkpoint_json",
    "totaldrop_mobile_sanitize_filename_json",
    "totaldrop_mobile_sanitize_text_json",
    "totaldrop_mobile_format_progress_json",
    "totaldrop_mobile_session_start_send_files",
    "totaldrop_mobile_session_start_send_text",
    "totaldrop_mobile_session_start_receive",
    "totaldrop_mobile_session_accept_offer",
    "totaldrop_mobile_session_decline_offer",
    "totaldrop_mobile_session_stop",
    "totaldrop_mobile_session_poll_events_ndjson",
    "totaldrop_mobile_session_destroy",
)


def parse_nm_exported_symbols(nm_stdout: str) -> set[str]:
    """Extract normalized C symbol names (stripping Darwin leading `_`) from `nm -gU` output."""
    symbols: set[str] = set()
    for raw_line in nm_stdout.splitlines():
        line = raw_line.strip()
        if not line or line.endswith(":"):
            continue
        parts = line.split()
        if len(parts) < 2:
            continue
        sym = parts[-1]
        if sym.startswith("_"):
            sym = sym[1:]
        symbols.add(sym)
    return symbols


def verify_xcframework(
    xcframework_path: Path,
    *,
    require_tvos: bool = False,
    check_native: bool = False,
) -> list[str]:
    if not xcframework_path.is_dir():
        raise ValueError(f"missing XCFramework directory: {xcframework_path}")
    info_plist = xcframework_path / "Info.plist"
    if not info_plist.is_file():
        raise ValueError(f"missing Info.plist inside {xcframework_path}")
    if not CANONICAL_HEADER.is_file():
        raise ValueError(f"missing canonical C header at {CANONICAL_HEADER}")
    canonical_header_bytes = CANONICAL_HEADER.read_bytes()

    with info_plist.open("rb") as fh:
        plist = plistlib.load(fh)

    if plist.get("CFBundlePackageType") != "XFWK":
        raise ValueError(f"Info.plist CFBundlePackageType {plist.get('CFBundlePackageType')!r} != 'XFWK'")
    if str(plist.get("XCFrameworkFormatVersion", "")) != "1.0":
        raise ValueError(
            f"Info.plist XCFrameworkFormatVersion {plist.get('XCFrameworkFormatVersion')!r} != '1.0'"
        )

    available = plist.get("AvailableLibraries")
    if not isinstance(available, list) or not available:
        raise ValueError("Info.plist AvailableLibraries must be a non-empty list")

    by_id: dict[str, dict] = {}
    for entry in available:
        if not isinstance(entry, dict):
            raise ValueError("AvailableLibraries entry must be a dict")
        lib_id = str(entry.get("LibraryIdentifier", ""))
        if not lib_id:
            raise ValueError("AvailableLibraries entry missing LibraryIdentifier")
        by_id[lib_id] = entry

    expected_slices: dict[str, set[str]] = {
        "ios-arm64": {"arm64"},
        "ios-arm64_x86_64-simulator": {"arm64", "x86_64"},
    }
    if require_tvos:
        expected_slices["tvos-arm64"] = {"arm64"}
        expected_slices["tvos-arm64-simulator"] = {"arm64"}

    verified_slices: list[str] = []
    for slice_id, expected_archs in expected_slices.items():
        if slice_id not in by_id:
            raise ValueError(f"missing required XCFramework slice {slice_id!r}")
        entry = by_id[slice_id]
        declared_archs = set(entry.get("SupportedArchitectures") or [])
        if declared_archs != expected_archs:
            raise ValueError(
                f"slice {slice_id}: SupportedArchitectures {sorted(declared_archs)} != {sorted(expected_archs)}"
            )

        lib_rel = entry.get("LibraryPath")
        headers_rel = entry.get("HeadersPath")
        if not lib_rel or not headers_rel:
            raise ValueError(f"slice {slice_id}: missing LibraryPath or HeadersPath")

        slice_dir = xcframework_path / slice_id
        lib_path = slice_dir / lib_rel
        headers_dir = slice_dir / headers_rel
        if not lib_path.is_file() or lib_path.stat().st_size == 0:
            raise ValueError(f"slice {slice_id}: missing or empty static library {lib_path}")
        if lib_path.name != LIB_NAME:
            raise ValueError(f"slice {slice_id}: library name {lib_path.name!r} != {LIB_NAME!r}")

        hdr_path = headers_dir / "totaldrop_apple_mobile_ffi.h"
        if not hdr_path.is_file():
            raise ValueError(f"slice {slice_id}: missing header {hdr_path}")
        if hdr_path.read_bytes() != canonical_header_bytes:
            raise ValueError(f"slice {slice_id}: totaldrop_apple_mobile_ffi.h drifted from Rust crate")

        modulemap_path = headers_dir / "module.modulemap"
        if not modulemap_path.is_file():
            raise ValueError(f"slice {slice_id}: missing module.modulemap")
        mm_text = modulemap_path.read_text(encoding="utf-8")
        if "totaldrop_apple_mobile_ffi.h" not in mm_text:
            raise ValueError(f"slice {slice_id}: module.modulemap does not reference totaldrop_apple_mobile_ffi.h")

        if check_native:
            lipo_out = subprocess.check_output(
                ["lipo", "-archs", str(lib_path)], text=True
            ).strip()
            actual_archs = set(lipo_out.split())
            if actual_archs != expected_archs:
                raise ValueError(
                    f"slice {slice_id}: lipo -archs {sorted(actual_archs)} != {sorted(expected_archs)}"
                )

            for arch in sorted(expected_archs):
                nm_proc = subprocess.run(
                    ["nm", "-gU", "-arch", arch, str(lib_path)],
                    text=True,
                    capture_output=True,
                    check=False,
                )
                if not nm_proc.stdout.strip():
                    raise ValueError(
                        f"slice {slice_id} ({arch}): nm -gU produced no symbol output "
                        f"(rc={nm_proc.returncode}, stderr={nm_proc.stderr.strip()!r})"
                    )
                exported = parse_nm_exported_symbols(nm_proc.stdout)
                missing_syms = [s for s in REQUIRED_EXPORTED_SYMBOLS if s not in exported]
                if missing_syms:
                    raise ValueError(
                        f"slice {slice_id} ({arch}): missing exported C ABI symbols: "
                        f"{', '.join(missing_syms)}"
                    )

        verified_slices.append(f"{slice_id}({','.join(sorted(expected_archs))})")

    return verified_slices


def write_synthetic_xcframework(root: Path) -> Path:
    xf = root / "TotalDropAppleMobileFFI.xcframework"
    header_bytes = CANONICAL_HEADER.read_bytes()
    modulemap_text = (
        "module CTotalDropAppleMobileFFI {\n"
        '    header "totaldrop_apple_mobile_ffi.h"\n'
        "    export *\n"
        "}\n"
    )
    libraries = [
        {
            "BinaryPath": LIB_NAME,
            "HeadersPath": "Headers",
            "LibraryIdentifier": "ios-arm64",
            "LibraryPath": LIB_NAME,
            "SupportedArchitectures": ["arm64"],
            "SupportedPlatform": "ios",
        },
        {
            "BinaryPath": LIB_NAME,
            "HeadersPath": "Headers",
            "LibraryIdentifier": "ios-arm64_x86_64-simulator",
            "LibraryPath": LIB_NAME,
            "SupportedArchitectures": ["arm64", "x86_64"],
            "SupportedPlatform": "ios",
            "SupportedPlatformVariant": "simulator",
        },
    ]
    for entry in libraries:
        sdir = xf / entry["LibraryIdentifier"]
        hdir = sdir / "Headers"
        hdir.mkdir(parents=True, exist_ok=True)
        (sdir / LIB_NAME).write_bytes(b"!<arch>\nsynthetic-test-staticlib\n")
        (hdir / "totaldrop_apple_mobile_ffi.h").write_bytes(header_bytes)
        (hdir / "module.modulemap").write_text(modulemap_text, encoding="utf-8")

    with (xf / "Info.plist").open("wb") as fh:
        plistlib.dump(
            {
                "AvailableLibraries": libraries,
                "CFBundlePackageType": "XFWK",
                "XCFrameworkFormatVersion": "1.0",
            },
            fh,
        )
    return xf


def run_self_test() -> int:
    # 1. Test nm -gU symbol parser (including universal multi-arch object headers).
    sample_nm = (
        "\nlibtotaldrop_apple_mobile_ffi.a(ffi.o) (for architecture arm64):\n"
        "0000000000000100 T _totaldrop_mobile_version_json\n"
        "0000000000000200 T _totaldrop_mobile_string_free\n"
        "\nlibtotaldrop_apple_mobile_ffi.a(ffi.o) (for architecture x86_64):\n"
        "0000000000000100 T _totaldrop_mobile_session_destroy\n"
    )
    parsed = parse_nm_exported_symbols(sample_nm)
    assert "totaldrop_mobile_version_json" in parsed
    assert "totaldrop_mobile_string_free" in parsed
    assert "totaldrop_mobile_session_destroy" in parsed

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        xf = write_synthetic_xcframework(tmp_path)
        slices = verify_xcframework(xf, require_tvos=False, check_native=False)
        assert len(slices) == 2

        # 2. Tamper header -> must fail.
        bad_hdr = xf / "ios-arm64" / "Headers" / "totaldrop_apple_mobile_ffi.h"
        orig_hdr = bad_hdr.read_bytes()
        bad_hdr.write_bytes(orig_hdr + b"\n/* drifted */\n")
        try:
            verify_xcframework(xf, require_tvos=False, check_native=False)
            raise AssertionError("expected drifted header to fail")
        except ValueError as err:
            assert "drifted" in str(err)
        bad_hdr.write_bytes(orig_hdr)

        # 3. Require tvOS when only iOS slices exist -> must fail.
        try:
            verify_xcframework(xf, require_tvos=True, check_native=False)
            raise AssertionError("expected missing tvos slice to fail")
        except ValueError as err:
            assert "tvos-arm64" in str(err)

    print("verify-xcframework.py --self-test OK (structural, header parity, tvOS gate & nm parser).")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="Run built-in unit self-test")
    parser.add_argument("--xcframework", type=Path, help="Path to TotalDropAppleMobileFFI.xcframework")
    parser.add_argument(
        "--require-tvos",
        action="store_true",
        help="Also require tvos-arm64 and tvos-arm64-simulator slices",
    )
    parser.add_argument(
        "--check-native",
        action="store_true",
        help="Run macOS lipo -archs and nm -gU symbol verification on static libraries",
    )
    args = parser.parse_args()

    if args.self_test:
        return run_self_test()

    if not args.xcframework:
        parser.error("either --self-test or --xcframework is required")

    try:
        slices = verify_xcframework(
            args.xcframework,
            require_tvos=args.require_tvos,
            check_native=args.check_native,
        )
    except ValueError as err:
        print(f"error: {err}", file=sys.stderr)
        return 1

    native_note = " + native lipo/nm verified" if args.check_native else ""
    print(f"XCFramework OK ({args.xcframework}): slices={', '.join(slices)}{native_note}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
