// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

import SwiftUI

/// Native Apple tvOS 10-foot Receive screen built on the SwiftUI Focus Engine (`@FocusState`)
/// and Siri Remote directional focus cards (`.buttonStyle(.card)`).
public struct TvReceiveView: View {
    @Bindable var model: TvAppModel
    @FocusState private var focusedSlot: TvFocusSlot?

    public init(model: TvAppModel) {
        self.model = model
    }

    public var body: some View {
        VStack(spacing: 42) {
            VStack(spacing: 10) {
                Text("TotalDrop")
                    .font(.largeTitle.weight(.bold))
                Text("Receive end-to-end encrypted files on Apple TV")
                    .font(.title3)
                    .foregroundStyle(.secondary)
            }

            switch model.phase {
            case .connecting(let code):
                connectingFocusCard(code: code)
            case .offerReady(let files, let totalBytes, let words, let names):
                offerFocusGrid(files: files, totalBytes: totalBytes, words: words, names: names)
            case .transferring(let percent, let summary, let words):
                progressFocusCard(percent: percent, summary: summary, words: words)
            case .completed(let summary):
                completedFocusCard(summary: summary)
            case .failed(let headline, let detail):
                failedFocusCard(headline: headline, detail: detail)
            default:
                entryFocusCard
            }
        }
        .padding(60)
        .onExitCommand {
            if model.phase != .idle {
                model.cancelOrReset()
            }
        }
    }

    private var entryFocusCard: some View {
        VStack(spacing: 32) {
            TextField("Enter sender code (e.g. 7-harbor-rocket)", text: $model.codeInput)
                .font(.system(.title2, design: .monospaced))
                .frame(maxWidth: 860)
                .focused($focusedSlot, equals: .codeBanner)
                .onSubmit {
                    model.connectWithCode()
                }

            HStack(spacing: 36) {
                Button {
                    model.connectWithCode()
                } label: {
                    Label("Connect", systemImage: "lock.shield.fill")
                        .font(.title3.weight(.semibold))
                        .padding(.horizontal, 36)
                        .padding(.vertical, 18)
                }
                .buttonStyle(.card)
                .focused($focusedSlot, equals: .primaryAction)
                .disabled(model.codeInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)

                Button {
                    model.codeInput = ""
                } label: {
                    Label("Clear", systemImage: "xmark.circle")
                        .font(.title3)
                        .padding(.horizontal, 32)
                        .padding(.vertical, 18)
                }
                .buttonStyle(.card)
                .focused($focusedSlot, equals: .cancelAction)
            }
        }
        .defaultFocus($focusedSlot, .codeBanner)
    }

    private func connectingFocusCard(code: String) -> some View {
        VStack(spacing: 28) {
            ProgressView()
            Text("Connecting securely with \(code)…")
                .font(.title3.monospaced())
            Button(role: .destructive) {
                model.cancelOrReset()
            } label: {
                Text("Cancel")
                    .padding(.horizontal, 36)
                    .padding(.vertical, 16)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .cancelAction)
        }
        .defaultFocus($focusedSlot, .cancelAction)
    }

    private func offerFocusGrid(files: Int, totalBytes: UInt64, words: [String], names: [String]) -> some View {
        VStack(spacing: 32) {
            Button {} label: {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Verification words")
                        .font(.callout.weight(.semibold))
                        .foregroundStyle(.secondary)
                    Text(words.joined(separator: "   ·   "))
                        .font(.system(.title2, design: .monospaced).weight(.bold))
                    Text("Match these four words with the sender before accepting.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
                .padding(28)
                .frame(maxWidth: 960, alignment: .leading)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .verificationCard)

            VStack(spacing: 8) {
                Text("\(files) file(s) · \(ByteCountFormatter.string(fromByteCount: Int64(clamping: totalBytes), countStyle: .file))")
                    .font(.headline)
                ForEach(names.prefix(5), id: \.self) { item in
                    Text(item)
                        .font(.callout.monospaced())
                        .foregroundStyle(.secondary)
                }
            }

            HStack(spacing: 40) {
                Button {
                    model.phase = .completed(summary: "Verified in Apple TV session cache (\(model.cachesStagingDirectory.path)).")
                } label: {
                    Label("Accept", systemImage: "checkmark.circle.fill")
                        .font(.title3.weight(.semibold))
                        .padding(.horizontal, 42)
                        .padding(.vertical, 18)
                }
                .buttonStyle(.card)
                .focused($focusedSlot, equals: .primaryAction)

                Button(role: .destructive) {
                    model.cancelOrReset()
                } label: {
                    Label("Decline", systemImage: "xmark.circle.fill")
                        .font(.title3)
                        .padding(.horizontal, 42)
                        .padding(.vertical, 18)
                }
                .buttonStyle(.card)
                .focused($focusedSlot, equals: .cancelAction)
            }
        }
        .defaultFocus($focusedSlot, .verificationCard)
    }

    private func progressFocusCard(percent: Int, summary: String, words: [String]) -> some View {
        VStack(spacing: 26) {
            if !words.isEmpty {
                Text(words.joined(separator: " · "))
                    .font(.title3.monospaced())
            }
            ProgressView(value: Double(percent), total: 100.0) {
                Text("Receiving (\(percent)%) — \(summary)")
                    .font(.headline.monospacedDigit())
            }
            .frame(maxWidth: 820)

            Button(role: .destructive) {
                model.cancelOrReset()
            } label: {
                Text("Stop")
                    .padding(.horizontal, 36)
                    .padding(.vertical, 16)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .cancelAction)
        }
    }

    private func completedFocusCard(summary: String) -> some View {
        VStack(spacing: 24) {
            Label("Received and verified", systemImage: "checkmark.seal.fill")
                .font(.title2.weight(.bold))
                .foregroundStyle(.green)
            Text(summary)
                .font(.callout)
                .foregroundStyle(.secondary)
            Button {
                model.codeInput = ""
                model.cancelOrReset()
            } label: {
                Text("Done")
                    .padding(.horizontal, 42)
                    .padding(.vertical, 16)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .primaryAction)
        }
        .defaultFocus($focusedSlot, .primaryAction)
    }

    private func failedFocusCard(headline: String, detail: String) -> some View {
        VStack(spacing: 20) {
            Label(headline, systemImage: "exclamationmark.triangle.fill")
                .font(.title3.weight(.semibold))
                .foregroundStyle(.red)
            Text(detail)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
            Button {
                model.cancelOrReset()
            } label: {
                Text("Try again")
                    .padding(.horizontal, 42)
                    .padding(.vertical, 16)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .primaryAction)
        }
        .defaultFocus($focusedSlot, .primaryAction)
    }
}
