// SPDX-FileCopyrightText: 2026 The TotalDrop Contributors
// SPDX-License-Identifier: GPL-3.0-or-later

import CoreImage.CIFilterBuiltins
import SwiftUI
#if canImport(UIKit)
import UIKit
#endif

/// Native Apple tvOS 10-foot Send & QR Pairing screen with Siri Remote directional
/// focus cards and living-room readable typography.
public struct TvSendView: View {
    @Bindable var model: TvAppModel
    @FocusState private var focusedSlot: TvFocusSlot?

    public init(model: TvAppModel) {
        self.model = model
    }

    public var body: some View {
        VStack(spacing: 42) {
            VStack(spacing: 10) {
                Text("Send from Apple TV")
                    .font(.largeTitle.weight(.bold))
                Text("Scan the QR code from across the room or enter the code on your phone or computer")
                    .font(.title3)
                    .foregroundStyle(.secondary)
            }

            switch model.phase {
            case .waitingToPair(let code, let qrPayload):
                pairingCards(code: code, qrPayload: qrPayload)
            default:
                setupCards
            }
        }
        .padding(60)
        .onExitCommand {
            if model.phase != .idle {
                model.cancelOrReset()
            }
        }
    }

    private var setupCards: some View {
        VStack(spacing: 32) {
            Button {
                model.strongerCode.toggle()
            } label: {
                HStack(spacing: 24) {
                    Image(systemName: model.strongerCode ? "checkmark.shield.fill" : "shield")
                        .font(.title2)
                    VStack(alignment: .leading, spacing: 6) {
                        Text("Stronger code (3 words)")
                            .font(.headline)
                        Text(model.strongerCode ? "Enabled — three random words" : "Standard — two random words")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(28)
                .frame(maxWidth: 760, alignment: .leading)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .codeBanner)

            Button {
                model.startPairingSend()
            } label: {
                Label("Show Pairing Code & QR", systemImage: "qrcode")
                    .font(.title3.weight(.semibold))
                    .padding(.horizontal, 44)
                    .padding(.vertical, 20)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .primaryAction)
        }
        .defaultFocus($focusedSlot, .primaryAction)
    }

    private func pairingCards(code: String, qrPayload: String) -> some View {
        VStack(spacing: 38) {
            HStack(spacing: 48) {
                Button {} label: {
                    VStack(alignment: .leading, spacing: 16) {
                        Text("Your code")
                            .font(.callout.weight(.semibold))
                            .foregroundStyle(.secondary)
                        Text(code)
                            .font(.system(size: 46, weight: .bold, design: .monospaced))
                        Text("Open TotalDrop on your iPhone, Mac, Android, Linux, or BSD device and enter this code.")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: 520, alignment: .leading)
                    }
                    .padding(36)
                }
                .buttonStyle(.card)
                .focused($focusedSlot, equals: .codeBanner)

                Button {} label: {
                    VStack(spacing: 14) {
                        #if canImport(UIKit)
                        if let qr = Self.makeLargeTVQRCode(payload: qrPayload) {
                            Image(uiImage: qr)
                                .interpolation(.none)
                                .resizable()
                                .frame(width: 280, height: 280)
                                .padding(18)
                                .background(.white, in: RoundedRectangle(cornerRadius: 20))
                        }
                        #endif
                        Text("Scan with Camera")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                    .padding(28)
                }
                .buttonStyle(.card)
                .focused($focusedSlot, equals: .qrCard)
            }

            Button(role: .destructive) {
                model.cancelOrReset()
            } label: {
                Label("Stop Pairing", systemImage: "xmark.circle")
                    .font(.title3)
                    .padding(.horizontal, 40)
                    .padding(.vertical, 16)
            }
            .buttonStyle(.card)
            .focused($focusedSlot, equals: .cancelAction)
        }
        .defaultFocus($focusedSlot, .qrCard)
    }

    #if canImport(UIKit)
    static func makeLargeTVQRCode(payload: String) -> UIImage? {
        let context = CIContext()
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(payload.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        guard let cg = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        return UIImage(cgImage: cg)
    }
    #endif
}
