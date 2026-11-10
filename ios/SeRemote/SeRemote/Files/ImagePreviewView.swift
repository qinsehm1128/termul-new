import CoreTransferable
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Shared image bytes keep their original file extension so the share sheet
/// offers the real format instead of a re-encoded PNG.
struct SharedImageFile: Transferable, Sendable {
    let data: Data
    let fileName: String

    nonisolated static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(exportedContentType: .data) { item in
            let directory = FileManager.default.temporaryDirectory
                .appendingPathComponent("SeRemoteShare", isDirectory: true)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let safeName = URL(fileURLWithPath: item.fileName).lastPathComponent
            let url = directory.appendingPathComponent(safeName.isEmpty ? "image" : safeName)
            try item.data.write(to: url, options: .atomic)
            return SentTransferredFile(url)
        }
    }
}

/// Fit-to-width image with pinch zoom (1×–6×) and double-tap fit/zoom.
struct ZoomableImagePreview: View {
    @Environment(\.sePalette) private var palette
    let image: UIImage
    let data: Data
    let name: String

    @State private var scale: CGFloat = 1
    @GestureState private var pinch: CGFloat = 1

    var body: some View {
        VStack(spacing: 8) {
            GeometryReader { geometry in
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .frame(width: geometry.size.width, height: geometry.size.height)
                    .scaleEffect(scale * pinch)
                    .animation(.snappy(duration: 0.2), value: scale)
                    .clipped()
            }
            .frame(height: 240)
            .background(palette.codeCanvas.opacity(0.5), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .contentShape(Rectangle())
            .gesture(
                MagnifyGesture()
                    .updating($pinch) { value, state, _ in
                        state = value.magnification
                    }
                    .onEnded { value in
                        scale = min(max(scale * value.magnification, 1), 6)
                    }
            )
            .onTapGesture(count: 2) {
                scale = scale > 1 ? 1 : 2
            }
            .accessibilityElement()
            .accessibilityLabel(Text("Image preview"))
            .accessibilityHint(Text("Pinch to zoom"))
            .accessibilityIdentifier("files-preview-image")

            HStack(spacing: 8) {
                ShareLink(
                    item: SharedImageFile(data: data, fileName: name),
                    preview: SharePreview(name, image: Image(uiImage: image))
                ) {
                    Label("Share image", systemImage: "square.and.arrow.up")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(palette.inkSecondary)
                        .padding(.horizontal, 10)
                        .frame(minHeight: 32)
                        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .overlay {
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .stroke(palette.stroke, lineWidth: 1)
                        }
                        .frame(minHeight: SeSpace.hitTarget)
                        .contentShape(Rectangle())
                }
                .accessibilityIdentifier("files-share-image")
                Spacer(minLength: 0)
            }
        }
        .accessibilityHint(Text("Double tap to fit"))
    }
}
