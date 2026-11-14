// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import SwiftUI
import UniformTypeIdentifiers
#if canImport(UIKit)
import UIKit
#endif

/// iOS/iPadOS Share Extension view staging security-scoped `NSItemProvider` attachments
/// into the `group.app.totaldrop.ios` shared container manifest.
public struct ShareExtensionView: View {
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    public let stagedItemNames: [String]
    public let skippedCount: Int
    public let onConfirm: @MainActor () -> Void
    public let onCancel: @MainActor () -> Void

    public init(
        stagedItemNames: [String],
        skippedCount: Int,
        onConfirm: @escaping @MainActor () -> Void,
        onCancel: @escaping @MainActor () -> Void
    ) {
        self.stagedItemNames = stagedItemNames
        self.skippedCount = skippedCount
        self.onConfirm = onConfirm
        self.onCancel = onCancel
    }

    private var minTapHeight: CGFloat {
        dynamicTypeSize >= .accessibility3 ? 68 : (dynamicTypeSize.isAccessibilitySize ? 56 : 44)
    }

    public var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("Send with TotalDrop")
                        .font(.title3.weight(.semibold))

                    if skippedCount > 0 {
                        Text(skippedCount == 1
                             ? "One shared item could not be read and was left out."
                             : "\(skippedCount) shared items could not be read and were left out.")
                            .font(.footnote)
                            .foregroundStyle(.orange)
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    ForEach(stagedItemNames, id: \.self) { name in
                        HStack {
                            Image(systemName: "doc.fill")
                                .foregroundStyle(.tint)
                                .accessibilityHidden(true)
                            Text(name)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 10))
                    }

                    Button(action: onConfirm) {
                        Text("Create code in TotalDrop")
                            .font(.headline)
                            .frame(maxWidth: .infinity, minHeight: minTapHeight)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(stagedItemNames.isEmpty)

                    Button(role: .cancel, action: onCancel) {
                        Text("Cancel")
                            .frame(maxWidth: .infinity, minHeight: minTapHeight)
                    }
                    .buttonStyle(.bordered)
                }
                .padding()
            }
        }
    }
}

/// Helper that stages security-scoped `URL`s into `group.app.totaldrop.ios/SharedInbox`
/// and writes `manifest.json` matching `totaldrop-apple-mobile-bridge`.
public enum ShareExtensionManifestStaging {
    public static let appGroupID = "group.app.totaldrop.ios"

    public static func stageSecurityScopedURLs(_ urls: [URL], containerRoot: URL) -> (names: [String], skipped: Int) {
        let inbox = containerRoot.appendingPathComponent("SharedInbox", isDirectory: true)
        try? FileManager.default.createDirectory(at: inbox, withIntermediateDirectories: true)

        var entries: [[String: Any]] = []
        var acceptedNames: [String] = []
        var seenCollisionKeys = Set<String>()
        var skipped = 0

        for (index, url) in urls.enumerated() {
            let accessed = url.startAccessingSecurityScopedResource()
            defer {
                if accessed {
                    url.stopAccessingSecurityScopedResource()
                }
            }
            let safeName = url.lastPathComponent.precomposedStringWithCanonicalMapping
            let collisionKey = safeName.lowercased()
            guard !safeName.isEmpty,
                  safeName != ".",
                  safeName != "..",
                  !seenCollisionKeys.contains(collisionKey)
            else {
                skipped += 1
                continue
            }

            let slotDir = inbox.appendingPathComponent("\(index)", isDirectory: true)
            let dest = slotDir.appendingPathComponent(safeName)
            do {
                try FileManager.default.createDirectory(at: slotDir, withIntermediateDirectories: true)
                if FileManager.default.fileExists(atPath: dest.path) {
                    try FileManager.default.removeItem(at: dest)
                }
                try FileManager.default.copyItem(at: url, to: dest)
                let attrs = try FileManager.default.attributesOfItem(atPath: dest.path)
                let size = (attrs[.size] as? NSNumber)?.uint64Value ?? 0
                seenCollisionKeys.insert(collisionKey)
                acceptedNames.append(safeName)
                entries.append([
                    "stagedPath": dest.path,
                    "safeName": safeName,
                    "collisionKey": collisionKey,
                    "byteSize": size,
                ])
            } catch {
                skipped += 1
            }
        }

        let manifest: [String: Any] = [
            "appGroup": appGroupID,
            "skippedCount": skipped,
            "entries": entries,
        ]
        if let data = try? JSONSerialization.data(withJSONObject: manifest, options: [.sortedKeys]) {
            let manifestURL = inbox.appendingPathComponent("manifest.json")
            try? data.write(to: manifestURL, options: [.atomic])
        }
        return (acceptedNames, skipped)
    }
}
