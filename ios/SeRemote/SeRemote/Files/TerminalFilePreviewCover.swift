import SwiftUI
import UIKit

/// Full-screen read-only preview for a file opened from the terminal file
/// panel or the iPad inspector.
///
/// Top chrome carries ✕, the file name with its line count and ⋯; the content
/// uses the existing Markdown, sandboxed HTML, image and text renderers. The
/// bottom pill inserts the shell-quoted path into the terminal. Tapping the
/// content toggles the chrome (immersive); a downward swipe only dismisses from
/// the grabber or top bar, so scrolling and selection never close the cover.
@MainActor
struct TerminalFilePreviewCover: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession
    let request: FilePreviewRequest
    let agentAvailable: Bool
    let onInsertPath: (String) -> Void
    let onSendToAgent: (String) -> Void
    let onClose: () -> Void

    @State private var chromeVisible = true
    @State private var didCopy = false

    init(
        session: WorkspaceSession,
        request: FilePreviewRequest,
        agentAvailable: Bool,
        onInsertPath: @escaping (String) -> Void,
        onSendToAgent: @escaping (String) -> Void,
        onClose: @escaping () -> Void
    ) {
        self.session = session
        self.request = request
        self.agentAvailable = agentAvailable
        self.onInsertPath = onInsertPath
        self.onSendToAgent = onSendToAgent
        self.onClose = onClose
    }

    var body: some View {
        ZStack(alignment: .top) {
            palette.canvas.ignoresSafeArea()
            contentScroll
            if chromeVisible {
                topChrome
                    .transition(.opacity)
            }
            VStack(spacing: 0) {
                Spacer(minLength: 0)
                if chromeVisible {
                    insertPill
                        .padding(.horizontal, 18)
                        .padding(.bottom, 12)
                        .transition(.opacity)
                }
            }
        }
        .background(palette.canvas)
        .animation(reduceMotion ? nil : SeNocturneMotion.quick, value: chromeVisible)
        .task(id: request.path) { await ensureLoaded() }
        .onChange(of: request.path) { _, _ in
            didCopy = false
            chromeVisible = true
        }
        .seHaptic(trigger: didCopy) { _, copied in copied ? .success : nil }
        .accessibilityAction(named: chromeVisible ? Text("Hide controls") : Text("Show controls")) {
            chromeVisible.toggle()
        }
        .accessibilityIdentifier("file-preview-cover")
    }

    // MARK: - Loaded state

    private var isCurrent: Bool { session.files.currentPreviewPath == request.path }

    private var name: String {
        if isCurrent, let previewName = session.files.previewName {
            return previewName
        }
        let last = URL(fileURLWithPath: request.path).lastPathComponent
        return last.isEmpty ? request.path : last
    }

    private var content: FileContent? { isCurrent ? session.files.preview : nil }
    private var kind: FilePreviewKind? { isCurrent ? session.files.previewKind : nil }
    private var imageData: Data? { isCurrent ? session.files.previewImage : nil }
    private var failure: FilePreviewFailure? { isCurrent ? session.files.previewFailure : nil }

    private var lineCount: Int? {
        FilePanelPresentation.lineCount(content: content, kind: kind)
    }

    private var isLoading: Bool { !isCurrent || session.files.isLoading }

    private var insertText: String {
        FilePanelPresentation.insertText(for: request.path)
    }

    private var availability: FilePanelPresentation.MenuAvailability {
        let hasText = content?.content.isEmpty == false
        let hasImage = imageData != nil
        return FilePanelPresentation.menu(
            agentAvailable: agentAvailable,
            hasContent: hasText || hasImage
        )
    }

    private var shareText: String? {
        guard let text = content?.content, !text.isEmpty else { return nil }
        return text
    }

    private var shareImage: (data: Data, image: UIImage)? {
        guard let imageData, let image = UIImage(data: imageData) else { return nil }
        return (imageData, image)
    }

    // MARK: - Content

    private var contentScroll: some View {
        GeometryReader { proxy in
            ScrollView {
                FilePreviewBody(
                    content: content,
                    kind: kind,
                    imageData: imageData,
                    failure: failure,
                    isLoading: isLoading,
                    availableHeight: proxy.size.height
                )
                .id(request.path)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 18)
                .padding(.top, chromeVisible ? 92 : 10)
                .padding(.bottom, chromeVisible ? 104 : 24)
            }
            .scrollIndicators(.hidden)
            .simultaneousGesture(
                TapGesture().onEnded { chromeVisible.toggle() }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityIdentifier("file-preview-content")
        }
    }

    // MARK: - Chrome

    private var topChrome: some View {
        VStack(spacing: 6) {
            Capsule()
                .fill(palette.inkTertiary.opacity(0.35))
                .frame(width: 36, height: 4)
                .accessibilityHidden(true)
            ZStack {
                VStack(spacing: 2) {
                    Text(name)
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .textSelection(.enabled)
                    Text(FilePanelPresentation.lineCountText(lineCount: lineCount))
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
                .padding(.horizontal, 56)
                .accessibilityElement(children: .combine)
                .accessibilityAddTraits(.isHeader)

                HStack(spacing: 2) {
                    chromeButton(
                        systemImage: "xmark",
                        label: String(localized: "Close preview"),
                        action: onClose
                    )
                    .accessibilityIdentifier("file-preview-close")
                    Spacer(minLength: 0)
                    if let info = availability.sendToAgentInfo {
                        SeInfoTip(
                            text: info,
                            accessibilityLabel: String(localized: "Send to Agent unavailable"),
                            size: 32
                        )
                    }
                    moreMenu
                }
            }
            .padding(.horizontal, 18)
        }
        .padding(.top, 6)
        .padding(.bottom, 8)
        .background(palette.canvas)
        .simultaneousGesture(swipeDown)
        .accessibilityIdentifier("file-preview-chrome")
    }

    private var moreMenu: some View {
        Menu {
            Button {
                onInsertPath(insertText)
            } label: {
                Label(String(localized: "Insert path"), systemImage: "arrow.right.to.line")
            }
            Button {
                onSendToAgent(request.path)
            } label: {
                Label(String(localized: "Send to Agent"), systemImage: "paperplane")
            }
            .disabled(!availability.sendToAgent)
            Divider()
            Button {
                copyContent()
            } label: {
                Label(
                    didCopy ? String(localized: "Copied") : String(localized: "Copy"),
                    systemImage: didCopy ? "checkmark" : "doc.on.doc"
                )
            }
            .disabled(!availability.copy)
            if let shareText {
                ShareLink(item: shareText) {
                    Label(String(localized: "Share"), systemImage: "square.and.arrow.up")
                }
            } else if let shareImage {
                ShareLink(
                    item: SharedImageFile(data: shareImage.data, fileName: name),
                    preview: SharePreview(name, image: Image(uiImage: shareImage.image))
                ) {
                    Label(String(localized: "Share"), systemImage: "square.and.arrow.up")
                }
            }
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 34, height: 34)
                .background(mode.insetFill, in: Circle())
                .overlay {
                    Circle().strokeBorder(mode.insetStroke, lineWidth: 1)
                }
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
        }
        .accessibilityLabel(Text("More actions"))
        .accessibilityIdentifier("file-preview-more")
    }

    private var insertPill: some View {
        Button {
            onInsertPath(insertText)
        } label: {
            HStack(spacing: 7) {
                Image(systemName: "arrow.right.to.line")
                    .font(.system(size: 13, weight: .semibold))
                Text(String(localized: "Insert path"))
                    .font(SeNocturneType.font(13, weight: .semibold))
            }
            .foregroundStyle(palette.canvas)
            .padding(.horizontal, 20)
            .frame(minHeight: 44)
            .background(palette.ink, in: Capsule(style: .continuous))
            .shadow(color: .black.opacity(0.22), radius: 14, y: 6)
            .contentShape(Capsule())
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(Text("Insert path into terminal"))
        .accessibilityIdentifier("file-preview-insert")
    }

    private func chromeButton(
        systemImage: String,
        label: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 34, height: 34)
                .background(mode.insetFill, in: Circle())
                .overlay {
                    Circle().strokeBorder(mode.insetStroke, lineWidth: 1)
                }
                .frame(minWidth: 44, minHeight: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(SePressable())
        .accessibilityLabel(Text(label))
    }

    private var swipeDown: some Gesture {
        DragGesture(minimumDistance: 12)
            .onEnded { value in
                guard value.translation.height > 70,
                      abs(value.translation.height) > abs(value.translation.width) else { return }
                onClose()
            }
    }

    // MARK: - Loading and actions

    /// The panel normally loads the file before reporting the open. Opening the
    /// cover from the inspector, or after a panel reload, still needs the bytes.
    private func ensureLoaded() async {
        guard session.files.currentPreviewPath != request.path else { return }
        let url = URL(fileURLWithPath: request.path)
        let last = url.lastPathComponent
        let entry = DirectoryEntry(
            name: last.isEmpty ? request.path : last,
            path: request.path,
            type: "file",
            fileExtension: url.pathExtension.lowercased(),
            size: nil,
            modifiedAt: nil
        )
        await session.files.openReturningSuccess(entry)
    }

    private func copyContent() {
        if let shareText {
            UIPasteboard.general.string = shareText
        } else if let shareImage {
            UIPasteboard.general.image = shareImage.image
        }
        didCopy = true
    }
}

/// The cover's content dispatch, mirroring `WorkspaceFilePreviewView` without
/// the card chrome: rendered Markdown, sandboxed HTML, a fit image, or the
/// monospaced source with all failures and empty states kept calm.
@MainActor
struct FilePreviewBody: View {
    @Environment(\.sePalette) private var palette

    let content: FileContent?
    let kind: FilePreviewKind?
    let imageData: Data?
    let failure: FilePreviewFailure?
    let isLoading: Bool
    let availableHeight: CGFloat

    @State private var imageScale: CGFloat = 1
    @GestureState private var imagePinch: CGFloat = 1

    private var text: String? {
        guard let value = content?.content, !value.isEmpty else { return nil }
        return value
    }

    var body: some View {
        Group {
            if let failure {
                message(FilePanelPresentation.message(for: failure))
            } else if let kind {
                switch kind {
                case .markdown:
                    markdown
                case .html:
                    html
                case .image:
                    image
                case .text:
                    source
                case .unsupportedImage:
                    message(String(localized: "This image format can't be previewed."))
                case .unsupported:
                    message(String(localized: "This file can't be previewed."))
                }
            } else if let text {
                sourceText(text)
            } else if isLoading {
                ProgressView()
                    .tint(palette.accent)
                    .frame(maxWidth: .infinity, minHeight: 200)
            } else {
                message(String(localized: "This file has no text to preview."))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var markdown: some View {
        Group {
            if let text {
                MarkdownPreviewView(markdown: text)
                    .frame(maxWidth: 760, alignment: .leading)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .accessibilityIdentifier("file-preview-markdown")
            } else if isLoading {
                ProgressView()
                    .tint(palette.accent)
                    .frame(maxWidth: .infinity, minHeight: 200)
            } else {
                message(String(localized: "This file has no text to preview."))
            }
        }
    }

    private var html: some View {
        Group {
            if let text {
                SandboxedHTMLView(
                    html: text,
                    textColor: UIColor(palette.ink),
                    backgroundColor: .clear
                )
                .frame(height: max(320, availableHeight * 0.62))
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .stroke(palette.stroke, lineWidth: 1)
                }
                .accessibilityIdentifier("file-preview-html")
            } else if isLoading {
                ProgressView()
                    .tint(palette.accent)
                    .frame(maxWidth: .infinity, minHeight: 200)
            } else {
                message(String(localized: "This file has no text to preview."))
            }
        }
    }

    @ViewBuilder
    private var image: some View {
        if let imageData, let image = UIImage(data: imageData) {
            Image(uiImage: image)
                .resizable()
                .scaledToFit()
                .frame(maxWidth: .infinity)
                .scaleEffect(min(max(imageScale * imagePinch, 1), 6))
                .animation(.snappy(duration: 0.2), value: imageScale)
                .gesture(
                    MagnifyGesture()
                        .updating($imagePinch) { value, state, _ in
                            state = value.magnification
                        }
                        .onEnded { value in
                            imageScale = min(max(imageScale * value.magnification, 1), 6)
                        }
                )
                .accessibilityElement()
                .accessibilityLabel(Text("Image preview"))
                .accessibilityHint(Text("Pinch to zoom"))
                .accessibilityIdentifier("file-preview-image")
        } else if isLoading {
            ProgressView()
                .tint(palette.accent)
                .frame(maxWidth: .infinity, minHeight: 200)
        } else {
            message(String(localized: "This file can't be previewed."))
        }
    }

    @ViewBuilder
    private var source: some View {
        if let text {
            sourceText(text)
        } else if isLoading {
            ProgressView()
                .tint(palette.accent)
                .frame(maxWidth: .infinity, minHeight: 200)
        } else {
            message(String(localized: "This file has no text to preview."))
        }
    }

    private func sourceText(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12, design: .monospaced))
            .lineSpacing(5)
            .foregroundStyle(palette.inkSecondary)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityIdentifier("file-preview-text")
    }

    private func message(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundStyle(palette.inkSecondary)
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityIdentifier("file-preview-state")
    }
}
