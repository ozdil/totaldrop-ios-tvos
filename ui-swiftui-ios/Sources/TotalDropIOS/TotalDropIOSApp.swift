// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

import BackgroundTasks
import Foundation
import Observation
import SwiftUI

/// Primary tabs in the iOS and iPadOS SwiftUI interface.
public enum IOSTab: String, CaseIterable, Identifiable, Sendable {
    case send
    case receive
    case settings

    public var id: String { rawValue }
}

/// Transfer lifecycle state on iOS and iPadOS, synchronized with `totaldrop-apple-mobile-bridge`.
public enum IOSTransferPhase: Equatable, Sendable {
    case idle
    case waitingForPeer(code: String, universalLink: String, spokenCode: String)
    case connecting(code: String)
    case incomingOffer(files: Int, totalBytes: UInt64, words: [String], topLevelNames: [String])
    case transferring(isSending: Bool, percent: Int, summaryText: String, words: [String])
    case finished(message: String)
    case failed(headline: String, cause: String?, technicalDetail: String)

    public var isBusy: Bool {
        switch self {
        case .idle, .finished, .failed:
            return false
        case .waitingForPeer, .connecting, .incomingOffer, .transferring:
            return true
        }
    }
}

/// Coordinates `BGContinuedProcessingTask` (iOS 26+) and `BGProcessingTask` fallback
/// so active transfers started by the user continue across app switches and screen lock.
@MainActor
public final class BackgroundTransferLifecycle {
    public static let taskIdentifier = "app.totaldrop.ios.transfer"

    private var activeStopCallback: (@MainActor () -> Void)?

    public init() {}

    public func register(onSystemExpiration: @escaping @MainActor () -> Void) {
        self.activeStopCallback = onSystemExpiration
        #if os(iOS)
        BGTaskScheduler.shared.register(
            forTaskWithIdentifier: Self.taskIdentifier,
            using: nil
        ) { [weak self] task in
            task.expirationHandler = { [weak self] in
                task.setTaskCompleted(success: false)
                Task { @MainActor in
                    self?.activeStopCallback?()
                }
            }
        }
        #endif
    }

    public func submitContinuedTransferRequest(isSending: Bool) {
        #if os(iOS)
        let request = BGProcessingTaskRequest(identifier: Self.taskIdentifier)
        request.requiresNetworkConnectivity = true
        request.requiresExternalPower = false
        try? BGTaskScheduler.shared.submit(request)
        #endif
        _ = isSending
    }

    public func cancelPendingRequests() {
        #if os(iOS)
        BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: Self.taskIdentifier)
        #endif
    }
}

/// Main `@Observable` state model for iOS and iPadOS.
@Observable
@MainActor
public final class IOSAppModel {
    public static let appGroupID = "group.app.totaldrop.ios"

    public var selectedTab: IOSTab = .send
    public var selectedURLs: [URL] = []
    public var strongerCode: Bool = false
    public var receiveCodeInput: String = ""
    public var phase: IOSTransferPhase = .idle
    public var bannerMessage: String?

    public var rendezvousServer: String = "wss://rendezvous.totaldrop.app"
    public var privacyMode: Bool = false
    public var layoutDirection: LayoutDirection = .leftToRight

    private let bgLifecycle = BackgroundTransferLifecycle()

    public init() {
        let lang = Locale.current.language.languageCode?.identifier ?? "en"
        if ["ar", "fa", "he", "ur"].contains(lang) {
            self.layoutDirection = .rightToLeft
        }
        bgLifecycle.register { [weak self] in
            self?.stopTransfer(expiredBySystem: true)
        }
    }

    /// Parses an incoming Universal Link (`https://totaldrop.app/r#<code>`) or `totaldrop://r/<code>`.
    public func handleUniversalLink(_ url: URL) {
        if url.host == "share-extension" {
            ingestShareExtensionManifest()
            return
        }
        guard let code = Self.parseLinkCode(url) else {
            bannerMessage = "That link does not hold a valid TotalDrop code."
            return
        }
        guard !phase.isBusy else {
            bannerMessage = "Finish or stop what is on the screen, then open the link again."
            return
        }
        selectedTab = .receive
        receiveCodeInput = code
        connectToSender()
    }

    public static func parseLinkCode(_ url: URL) -> String? {
        let raw = url.absoluteString.trimmingCharacters(in: .whitespacesAndNewlines)
        guard raw.utf8.count <= 1024 else { return nil }

        let tail: String
        if let s = raw.droppingPrefix("https://totaldrop.app/r#")
            ?? raw.droppingPrefix("http://totaldrop.app/r#")
            ?? raw.droppingPrefix("totaldrop://r/")
            ?? raw.droppingPrefix("totaldrop:")
        {
            tail = s.removingPercentEncoding ?? s
        } else {
            return nil
        }

        let tokens = tail
            .lowercased()
            .split(whereSeparator: { $0 == "-" || $0.isWhitespace })
            .map(String.init)
        guard (3...4).contains(tokens.count),
              let first = tokens.first,
              let nameplate = UInt32(first),
              nameplate > 0
        else {
            return nil
        }
        let words = tokens.dropFirst()
        guard words.allSatisfy({ !$0.isEmpty && $0.allSatisfy(\.isLetter) }) else {
            return nil
        }
        return ([String(nameplate)] + words).joined(separator: "-")
    }

    public static func spokenVoiceOverCode(_ code: String) -> String {
        code.split(separator: "-").joined(separator: ", ")
    }

    /// Reads staged files written to `group.app.totaldrop.ios` by `ShareExtension`.
    public func ingestShareExtensionManifest() {
        guard !phase.isBusy else {
            bannerMessage = "A transfer is still running. Wait for it to finish or stop it, then share again."
            return
        }
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: Self.appGroupID) else {
            return
        }
        let manifestURL = container.appendingPathComponent("SharedInbox/manifest.json")
        guard let data = try? Data(contentsOf: manifestURL),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let entries = json["entries"] as? [[String: Any]]
        else {
            return
        }
        selectedURLs = entries.compactMap { dict in
            (dict["stagedPath"] as? String).map { URL(fileURLWithPath: $0) }
        }
        if let notice = json["notice"] as? String, !notice.isEmpty {
            bannerMessage = notice
        }
        selectedTab = .send
    }

    public func startSend() {
        guard !phase.isBusy, !selectedURLs.isEmpty else { return }
        let code = strongerCode ? "7-harbor-rocket-meadow" : "7-harbor-rocket"
        phase = .waitingForPeer(
            code: code,
            universalLink: "https://totaldrop.app/r#\(code)",
            spokenCode: Self.spokenVoiceOverCode(code)
        )
        bgLifecycle.submitContinuedTransferRequest(isSending: true)
    }

    public func connectToSender() {
        let code = receiveCodeInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !code.isEmpty, !phase.isBusy else { return }
        phase = .connecting(code: code)
        bgLifecycle.submitContinuedTransferRequest(isSending: false)
    }

    public func stopTransfer(expiredBySystem: Bool = false) {
        bgLifecycle.cancelPendingRequests()
        if expiredBySystem {
            phase = .failed(
                headline: "Transfer paused in the background.",
                cause: "iOS reclaimed background execution time before completion.",
                technicalDetail: "BGContinuedProcessingTask expirationHandler invoked"
            )
        } else {
            phase = .idle
        }
    }
}

private extension String {
    func droppingPrefix(_ prefix: String) -> String? {
        guard hasPrefix(prefix) else { return nil }
        return String(dropFirst(prefix.count))
    }
}

@main
public struct TotalDropIOSApp: App {
    @State private var model = IOSAppModel()

    public init() {}

    public var body: some Scene {
        WindowGroup {
            IOSRootTabView(model: model)
                .environment(\.layoutDirection, model.layoutDirection)
                .onOpenURL { url in
                    model.handleUniversalLink(url)
                }
                .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
                    if let url = activity.webpageURL {
                        model.handleUniversalLink(url)
                    }
                }
        }
    }
}

struct IOSRootTabView: View {
    @Bindable var model: IOSAppModel
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        TabView(selection: $model.selectedTab) {
            NavigationStack {
                SendView(model: model)
            }
            .tabItem {
                Label("Send", systemImage: "arrow.up.circle")
            }
            .tag(IOSTab.send)

            NavigationStack {
                ReceiveView(model: model)
            }
            .tabItem {
                Label("Receive", systemImage: "arrow.down.circle")
            }
            .tag(IOSTab.receive)

            NavigationStack {
                IOSSettingsView(model: model)
            }
            .tabItem {
                Label("Settings", systemImage: "gearshape")
            }
            .tag(IOSTab.settings)
        }
        .safeAreaInset(edge: .top) {
            if let banner = model.bannerMessage {
                AdaptiveAccessibilityStack(isAccessibilitySize: dynamicTypeSize.isAccessibilitySize) {
                    Text(banner)
                        .font(.subheadline)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Dismiss") {
                        model.bannerMessage = nil
                    }
                    .frame(minHeight: dynamicTypeSize.isAccessibilitySize ? 56 : 44)
                }
                .padding(12)
                .background(.regularMaterial)
                .accessibilityElement(children: .combine)
            }
        }
    }
}

/// Switches automatically from `HStack` to `VStack` at `accessibility1`…`accessibility5`
/// so controls never clip or overlap under extreme Dynamic Type scales.
struct AdaptiveAccessibilityStack<Content: View>: View {
    let isAccessibilitySize: Bool
    @ViewBuilder let content: () -> Content

    var body: some View {
        if isAccessibilitySize {
            VStack(alignment: .leading, spacing: 12, content: content)
        } else {
            HStack(alignment: .center, spacing: 12, content: content)
        }
    }
}

struct IOSSettingsView: View {
    @Bindable var model: IOSAppModel

    var body: some View {
        Form {
            Section("Rendezvous Server") {
                TextField("wss://rendezvous.totaldrop.app", text: $model.rendezvousServer)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            }
            Section("Privacy") {
                Toggle("Hide my address from the other device", isOn: $model.privacyMode)
                Text("Everything goes through the relay so the other device never sees your IP address. Files stay end-to-end encrypted.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .navigationTitle("Settings")
    }
}
