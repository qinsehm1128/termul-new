import CoreImage
import SwiftUI
import UIKit

/// Pairing is a round trip: scan, confirm on the host, credentials land in the
/// keychain. This sheet is the only place that starts it.
struct PairingSheetView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss

    var link: String
    var onCopyLink: () -> Void = {}
    var onManualEntry: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Capsule()
                .fill(palette.strokeStrong)
                .frame(width: 36, height: 4)
                .frame(maxWidth: .infinity)
                .padding(.top, 10)

            HStack(spacing: 10) {
                Text("Pair a host")
                    .font(SeNocturneType.sheetTitle)
                    .foregroundStyle(palette.ink)
                Spacer()
                Button {
                    dismiss()
                } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(palette.inkSecondary)
                        .frame(width: 32, height: 32)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                        .overlay {
                            RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                                .strokeBorder(palette.stroke, lineWidth: 1)
                        }
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "Close"))
            }
            .padding(.horizontal, SeSpace.md)
            .padding(.top, 14)

            Text("Open SeRemote on your Mac → Remote access, then scan this code. The host confirms the connection.")
                .font(SeNocturneType.secondary)
                .foregroundStyle(palette.inkTertiary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 4)

            qrCard
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 16)

            linkBox
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 14)

            steps
                .padding(.horizontal, SeSpace.md)
                .padding(.top, 16)

            HStack(spacing: 8) {
                Button(action: onCopyLink) {
                    Text("Copy pairing link")
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .semibold))
                        .foregroundStyle(palette.surface)
                        .frame(maxWidth: .infinity)
                        .frame(height: 44)
                        .background(palette.ink, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                }
                .buttonStyle(.plain)

                Button(action: onManualEntry) {
                    Text("Enter manually")
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: .medium))
                        .foregroundStyle(palette.ink)
                        .frame(height: 44)
                        .padding(.horizontal, 14)
                        .background(Color.clear, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                        .overlay {
                            RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                                .strokeBorder(palette.strokeStrong, lineWidth: 1)
                        }
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, SeSpace.md)
            .padding(.top, 18)
            .padding(.bottom, SeSpace.lg)
        }
        .background(palette.surface)
        .presentationDragIndicator(.hidden)
        .presentationCornerRadius(SeNocturneRadius.sheet)
    }

    private var qrCard: some View {
        HStack {
            Spacer(minLength: 0)
            Group {
                if let image = Self.makeQRCode(link) {
                    Image(uiImage: image)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                } else {
                    Image(systemName: "qrcode")
                        .resizable()
                        .scaledToFit()
                        .foregroundStyle(palette.ink)
                }
            }
            .frame(width: 168, height: 168)
            .padding(12)
            .background(Color.white, in: RoundedRectangle(cornerRadius: 12))
            .overlay {
                RoundedRectangle(cornerRadius: 12)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
            Spacer(minLength: 0)
        }
    }

    private var linkBox: some View {
        HStack(spacing: 8) {
            Text(verbatim: link)
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkSecondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 6)
            Button(action: onCopyLink) {
                Text("Copy")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkSecondary)
                    .padding(.horizontal, 8)
                    .frame(height: 26)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 6))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 10)
        .frame(height: 40)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 9) {
            step(1, "Confirm this request on the host: device name and place.")
            step(2, "Credentials go into the keychain, scoped to this host.")
            step(3, "The first connection is read only; writes are approved one by one.")
        }
    }

    private func step(_ index: Int, _ text: LocalizedStringKey) -> some View {
        HStack(alignment: .top, spacing: 9) {
            Text(verbatim: "\(index)")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
                .frame(width: 16, height: 16)
                .overlay {
                    Circle().strokeBorder(palette.strokeStrong, lineWidth: 1)
                }
            Text(text)
                .font(SeNocturneType.font(SeNocturneType.labelSize))
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private static func makeQRCode(_ text: String) -> UIImage? {
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return nil }
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        filter.setValue("M", forKey: "inputCorrectionLevel")
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
        let context = CIContext()
        guard let cgImage = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        return UIImage(cgImage: cgImage)
    }
}

#Preview("Pairing · Night") {
    PairingSheetView(link: "seremote://pair/9F2C-71A0")
        .seThemeMode(.night)
}

#Preview("Pairing · Day") {
    PairingSheetView(link: "seremote://pair/9F2C-71A0")
        .seThemeMode(.day)
}
