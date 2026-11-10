import PhotosUI
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// The workbench `+` sheet: pick a photo, take one, or choose an image file,
/// then stage it on the host and deliver its path to the terminal or the agent.
///
/// Images only: the host stage route accepts PNG/JPEG up to 8 MiB and 4096 px
/// (`src-tauri/src/web/remote_attachments_api.rs`), so the Files row is limited
/// to `UTType.image` and `AttachmentImagePolicy` preflights every pick.
struct AttachmentSheet: View {
    @State private var model: AttachmentPickerModel
    @State private var photoItem: PhotosPickerItem?
    @State private var showsCamera = false
    @State private var showsFiles = false

    @Environment(\.sePalette) private var palette

    init(
        session: WorkspaceSession,
        terminalID: String,
        terminalTitle: String,
        agentReady: Bool,
        onWriteTerminal: @escaping (String) async -> Void,
        onDraftAgent: @escaping (String, String) -> Void,
        onClose: @escaping () -> Void
    ) {
        _model = State(initialValue: AttachmentPickerModel(
            http: session.http,
            terminalID: terminalID,
            terminalTitle: terminalTitle,
            agentReady: agentReady,
            onWriteTerminal: onWriteTerminal,
            onDraftAgent: onDraftAgent,
            onClose: onClose
        ))
    }

    var body: some View {
        VStack(spacing: SeSpace.sm) {
            grabber
            targetRow
            pickerCard
            status
            Spacer(minLength: 0)
        }
        .padding(.top, 6)
        .padding(.horizontal, SeSpace.md)
        .padding(.bottom, SeSpace.md)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(palette.canvas)
        .presentationDetents([.height(384), .medium, .large])
        .presentationDragIndicator(.hidden)
        .presentationCornerRadius(SeNocturneRadius.sheet)
        .presentationBackground(palette.canvas)
        .interactiveDismissDisabled(model.isUploading)
        .fileImporter(
            isPresented: $showsFiles,
            allowedContentTypes: [.image],
            allowsMultipleSelection: false
        ) { result in
            handleFileImport(result)
        }
        .fullScreenCover(isPresented: $showsCamera) {
            AttachmentCameraPicker(
                onCapture: handleCamera,
                onCancel: { showsCamera = false }
            )
            .ignoresSafeArea()
        }
        .onChange(of: photoItem) { _, item in
            guard let item else { return }
            Task { await loadPhoto(item) }
        }
    }

    // MARK: - Target

    private var grabber: some View {
        Capsule()
            .fill(palette.inkTertiary.opacity(0.35))
            .frame(width: 36, height: 4)
            .padding(.bottom, 8)
            .accessibilityHidden(true)
    }

    private var targetRow: some View {
        Group {
            if model.agentReady {
                Menu {
                    targetMenu
                } label: {
                    targetRowBody(showsChevron: true)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("Send to \(model.targetName), \(model.targetBadge)"))
                .accessibilityHint(Text("Switch between the terminal and the agent"))
            } else {
                targetRowBody(showsChevron: false)
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(Text("Send to \(model.targetName)"))
            }
        }
        .disabled(model.isUploading)
    }

    @ViewBuilder private var targetMenu: some View {
        Button {
            model.sendToTerminal()
        } label: {
            Label(model.terminalTitle, systemImage: "apple.terminal")
        }
        Button {
            model.sendToAgent()
        } label: {
            Label(String(localized: "Agent"), systemImage: "sparkles")
        }
    }

    private func targetRowBody(showsChevron: Bool) -> some View {
        HStack(spacing: 8) {
            Text("Send to")
                .font(SeNocturneType.font(SeNocturneType.secondarySize))
                .foregroundStyle(palette.inkTertiary)
            Text("→")
                .font(SeNocturneType.font(SeNocturneType.secondarySize))
                .foregroundStyle(palette.inkTertiary)
                .accessibilityHidden(true)
            Text(model.targetName)
                .font(SeNocturneType.font(14.5, weight: .semibold))
                .foregroundStyle(palette.ink)
                .lineLimit(1)
            if showsChevron {
                SeIconView(icon: .chevronDown, size: 11, stroke: 1.7, color: palette.inkTertiary)
            }
            Spacer(minLength: 6)
            Text(model.targetBadge)
                .font(SeNocturneType.font(SeNocturneType.labelSize))
                .foregroundStyle(palette.inkSecondary)
                .lineLimit(1)
                .padding(.horizontal, 8)
                .frame(minHeight: 24)
                .background(palette.cardAlt, in: Capsule(style: .continuous))
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 54)
        .contentShape(Rectangle())
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    // MARK: - Sources

    private var pickerCard: some View {
        // `PhotosPicker`'s label builder is `@Sendable` and nonisolated, so the
        // label gets plain values instead of this view's main-actor state.
        let uploading = model.isUploading
        let rowPalette = palette
        return VStack(spacing: 0) {
            PhotosPicker(selection: $photoItem, matching: .images, photoLibrary: .shared()) {
                AttachmentSourceRow(
                    icon: .image,
                    title: String(localized: "Photos"),
                    isUploading: uploading,
                    palette: rowPalette
                )
            }
            .buttonStyle(.plain)
            .disabled(model.isUploading)
            .accessibilityLabel(Text("Photos"))
            .accessibilityHint(Text("Choose a photo from the library"))

            if AttachmentCameraPicker.isAvailable {
                divider
                Button {
                    showsCamera = true
                } label: {
                    AttachmentSourceRow(
                        systemImage: "camera",
                        title: String(localized: "Camera"),
                        isUploading: uploading,
                        palette: rowPalette
                    )
                }
                .buttonStyle(.plain)
                .disabled(model.isUploading)
                .accessibilityLabel(Text("Camera"))
                .accessibilityHint(Text("Take a photo"))
            }

            divider
            Button {
                showsFiles = true
            } label: {
                AttachmentSourceRow(
                    icon: .folder,
                    title: String(localized: "Files"),
                    trailing: String(localized: "Images only"),
                    isUploading: uploading,
                    palette: rowPalette
                )
            }
            .buttonStyle(.plain)
            .disabled(model.isUploading)
            .accessibilityLabel(Text("Files"))
            .accessibilityHint(Text("Choose an image file"))
        }
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
        .clipShape(RoundedRectangle(cornerRadius: SeNocturneRadius.card, style: .continuous))
    }

    private var divider: some View {
        Rectangle()
            .fill(palette.stroke)
            .frame(height: 1)
            .padding(.leading, 48)
            .accessibilityHidden(true)
    }

    // MARK: - Status

    @ViewBuilder private var status: some View {
        if model.isUploading {
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                    .tint(palette.inkSecondary)
                Text("Uploading…")
                    .font(SeNocturneType.font(SeNocturneType.secondarySize))
                    .foregroundStyle(palette.inkSecondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 2)
            .accessibilityElement(children: .combine)
            .accessibilityLabel(Text("Uploading the image"))
        } else if let notice = model.notice {
            HStack(spacing: 8) {
                SeIconView(icon: .check, size: 13, stroke: 1.8, color: palette.live)
                Text(notice)
                    .font(SeNocturneType.font(SeNocturneType.secondarySize))
                    .foregroundStyle(palette.ink)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(
                palette.liveSubtle,
                in: RoundedRectangle(cornerRadius: SeNocturneRadius.control, style: .continuous)
            )
            .accessibilityElement(children: .combine)
        } else if let error = model.errorMessage {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                SeIconView(icon: .info, size: 13, stroke: 1.8, color: palette.danger)
                Text(error)
                    .font(SeNocturneType.font(SeNocturneType.secondarySize))
                    .foregroundStyle(palette.ink)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(
                palette.dangerSubtle,
                in: RoundedRectangle(cornerRadius: SeNocturneRadius.control, style: .continuous)
            )
            .accessibilityElement(children: .combine)
        }
    }

    // MARK: - Actions

    private func handleFileImport(_ result: Result<[URL], Error>) {
        switch result {
        case let .success(urls):
            guard let url = urls.first else { return }
            Task { await sendFile(url) }
        case .failure:
            model.fail(String(localized: "That file could not be read."))
        }
    }

    /// The document picker returns a security-scoped URL that is only readable
    /// while the scope is open. Read the bytes inside the scope, on a
    /// background thread, so the copy finishes before the sheet closes.
    private func sendFile(_ url: URL) async {
        let data = await Task.detached(priority: .userInitiated) { () -> Data? in
            let scoped = url.startAccessingSecurityScopedResource()
            defer {
                if scoped { url.stopAccessingSecurityScopedResource() }
            }
            return try? Data(contentsOf: url)
        }.value
        guard let data else {
            model.fail(String(localized: "That file could not be read."))
            return
        }
        await model.send(data: data, name: url.lastPathComponent)
    }

    private func loadPhoto(_ item: PhotosPickerItem) async {
        guard let data = try? await item.loadTransferable(type: Data.self) else {
            model.fail(String(localized: "That photo could not be read."))
            return
        }
        await model.send(data: data, name: photoName(item))
        photoItem = nil
    }

    private func handleCamera(_ image: UIImage) {
        showsCamera = false
        guard let data = image.jpegData(compressionQuality: 0.95) else {
            model.fail(String(localized: "That photo could not be read."))
            return
        }
        Task { await model.send(data: data, name: "camera.jpg") }
    }

    /// A library pick may be HEIC; the staged copy is always PNG or JPEG, so the
    /// notice names the encoded kind.
    private func photoName(_ item: PhotosPickerItem) -> String {
        let isPNG = item.supportedContentTypes.contains { $0.conforms(to: .png) }
        return isPNG ? "photo.png" : "photo.jpg"
    }
}

/// One source row. `nonisolated` because `PhotosPicker`'s label builder is a
/// nonisolated `@ViewBuilder`, so a main-actor-isolated row cannot be built
/// inside it; the body itself still runs on the main actor.
private nonisolated struct AttachmentSourceRow: View {
    var icon: SeIcon?
    var systemImage: String?
    let title: String
    var trailing: String?
    var isUploading: Bool = false
    let palette: SePalette

    var body: some View {
        HStack(spacing: 12) {
            Group {
                if let icon {
                    SeIconView(icon: icon, size: 18, stroke: 1.6, color: palette.inkSecondary)
                } else if let systemImage {
                    Image(systemName: systemImage)
                        .font(.system(size: 15, weight: .regular))
                        .foregroundStyle(palette.inkSecondary)
                }
            }
            .frame(width: 22)
            Text(title)
                .font(SeNocturneType.font(SeNocturneType.bodySize))
                .foregroundStyle(palette.ink)
            Spacer(minLength: 8)
            if let trailing {
                Text(trailing)
                    .font(SeNocturneType.font(SeNocturneType.labelSize, mono: true))
                    .foregroundStyle(palette.inkTertiary)
            }
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 54)
        .contentShape(Rectangle())
        .opacity(isUploading ? 0.45 : 1)
    }
}
