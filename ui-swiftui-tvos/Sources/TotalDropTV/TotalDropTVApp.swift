// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import Observation
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// Primary tabs in the Apple tvOS 10-foot living-room interface.
public enum TvTab: String, CaseIterable, Identifiable, Sendable {
    case receive
    case send

    public var id: String { rawValue }
}

/// Focusable cards mirrored from `TvFocusCard` in `totaldrop-apple-mobile-bridge`.
public enum TvFocusSlot: Hashable, Sendable {
    case codeBanner
    case qrCard
    case verificationCard
    case primaryAction
    case cancelAction
}

/// Lifecycle state of a 10-foot Apple TV session.
public enum TvSessionPhase: Equatable, Sendable {
    case idle
    case waitingToPair(code: String, qrPayload: String)
    case connecting(code: String)
    case offerReady(files: Int, totalBytes: UInt64, words: [String], names: [String])
    case transferring(percent: Int, summary: String, words: [String])
    case completed(summary: String)
    case failed(headline: String, detail: String)

    public var keepsScreenAwake: Bool {
        switch self {
        case .idle, .completed, .failed:
            return false
        case .waitingToPair, .connecting, .offerReady, .transferring:
            return true
        }
    }
}

/// `@Observable` living-room session coordinator for Apple tvOS.
///
/// Staging files on tvOS uses `FileManager.default.urls(for: .cachesDirectory)`
/// (`NSCachesDirectory/TotalDrop`) because tvOS provides no persistent user Downloads
/// directory and may purge idle app caches when foregrounded apps need storage.
@Observable
@MainActor
public final class TvAppModel {
    public var selectedTab: TvTab = .receive
    public var codeInput: String = ""
    public var strongerCode: Bool = false
    public var phase: TvSessionPhase = .idle {
        didSet {
            updateIdleTimer()
        }
    }
    public var layoutDirection: LayoutDirection = .leftToRight

    public init() {
        let lang = Locale.current.language.languageCode?.identifier ?? "en"
        if ["ar", "fa", "he", "ur"].contains(lang) {
            self.layoutDirection = .rightToLeft
        }
    }

    public var cachesStagingDirectory: URL {
        let root = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        return root.appendingPathComponent("TotalDrop", isDirectory: true)
    }

    public func startPairingSend() {
        let code = strongerCode ? "7-harbor-rocket-meadow" : "7-harbor-rocket"
        phase = .waitingToPair(
            code: code,
            qrPayload: "https://totaldrop.app/r#\(code)"
        )
    }

    public func connectWithCode() {
        let trimmed = codeInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        phase = .connecting(code: trimmed)
    }

    public func cancelOrReset() {
        phase = .idle
    }

    private func updateIdleTimer() {
        #if os(tvOS)
        UIApplication.shared.isIdleTimerDisabled = phase.keepsScreenAwake
        #endif
    }
}

@main
public struct TotalDropTVApp: App {
    @State private var model = TvAppModel()

    public init() {}

    public var body: some Scene {
        WindowGroup {
            TabView(selection: $model.selectedTab) {
                TvReceiveView(model: model)
                    .tabItem {
                        Label("Receive", systemImage: "arrow.down.to.line.circle")
                    }
                    .tag(TvTab.receive)

                TvSendView(model: model)
                    .tabItem {
                        Label("Send", systemImage: "qrcode")
                    }
                    .tag(TvTab.send)
            }
            .environment(\.layoutDirection, model.layoutDirection)
        }
    }
}
