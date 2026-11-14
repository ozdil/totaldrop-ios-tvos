# TotalDrop — Native iOS, iPadOS & Apple tvOS Module (`08-ios-and-tvos`)

Native **Swift 6 strict-concurrency** SwiftUI applications for **iOS / iPadOS** and **Apple tvOS** over the shared Rust protocol engine (`totaldrop-core`, `totaldrop-proto`, `totaldrop-crypto`, and `totaldrop-l10n`), designed for account-free CI compilation and simulator testing on GitHub macOS runners (**ADR 0009**, **ADR 0019**).

## Architecture

| Component | Path | Responsibility |
| --- | --- | --- |
| Protocol, Crypto & Core | `crates/totaldrop-{proto,crypto,core,l10n}` | CPace PAKE, QUIC + Noise `XXpsk3` + ML-KEM-768 hybrid handshake, STREAM AEAD chunk framing, BLAKE3 verification, and 35-language ICU localization |
| Safe Apple Mobile Bridge | `crates/totaldrop-apple-mobile-bridge` | `#![forbid(unsafe_code)]` Rust bridge implementing Universal Links `/r#<code>` routing, iOS Share Extension security-scoped App Group manifests (`group.app.totaldrop.ios`), `BGContinuedProcessingTask` checkpoint metadata, tvOS 10-foot Siri Remote focus state, and Dynamic Type `accessibility5` strings |
| iOS & iPadOS SwiftUI | `ui-swiftui-ios/` | Swift 6 `TotalDropIOS` (`SendView`, `ReceiveView`, `BGTaskScheduler` background lifecycle, VoiceOver phonetic code cues, Dynamic Type up to `.accessibility5`) and `ShareExtension` |
| Apple tvOS SwiftUI | `ui-swiftui-tvos/` | Swift 6 `TotalDropTV` (`TvReceiveView`, `TvSendView`, SwiftUI Focus Engine `@FocusState`, Siri Remote `.buttonStyle(.card)` directional cards, 10-foot QR pairing, and `NSCachesDirectory` staging) |
| Physical iOS Target | `target-ios-arm64/` | `aarch64-apple-ios` build script, `Info.plist`, and Associated Domains / App Group `entitlements.plist` |
| ARM64 iOS Simulator | `target-ios-sim-arm64/` | `aarch64-apple-ios-sim` build unit for free GitHub `macos-15` Apple Silicon runners |
| x86_64 iOS Simulator | `target-ios-sim-x86_64/` | `x86_64-apple-ios` simulator slice for Intel Macs and universal simulator binaries |
| Apple tvOS & XCFramework | `target-tvos-arm64/` | `aarch64-apple-tvos` + `aarch64-apple-tvos-sim` build unit and `build-xcframework.sh` multi-platform bundler |

## Accessibility & Background Lifecycle

- **VoiceOver & Dynamic Type (`accessibility5`):** `AdaptiveAccessibilityStack` switches horizontal controls to full-width vertical stacks at `>= .accessibility1`, enforces `44pt`–`68pt` minimum tap targets, disables line truncation (`fixedSize(horizontal: false, vertical: true)`), and formats transfer codes (`7, harbor, rocket`) and 4-word verification phrases for clear VoiceOver speech.
- **Background Transfers:** Registers `app.totaldrop.ios.transfer` under `BGTaskSchedulerPermittedIdentifiers` and wires system expiration callbacks directly to `MobileStopToken` (`totaldrop_core::Stop`).
- **tvOS 10-Foot Experience:** Uses native tvOS card focus, `onExitCommand` Menu button handling, `isIdleTimerDisabled` while pairing or transferring, and stages received files under `NSCachesDirectory/TotalDrop`.

## Verification

Run the full Rust workspace lint and test suite:

```bash
RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets -- -D warnings
RUSTUP_TOOLCHAIN=stable cargo test --workspace
```

Assemble the multi-architecture `TotalDropAppleMobileBridge.xcframework` on macOS 15+:

```bash
./target-tvos-arm64/build-xcframework.sh
```

## License

- Application, SwiftUI packages, and mobile bridge (`crates/totaldrop-apple-mobile-bridge`, `ui-swiftui-*`, `target-*`): `GPL-3.0-or-later`
- Core protocol and localization libraries (`crates/totaldrop-{core,crypto,proto,l10n}`): `MIT OR Apache-2.0`
