import SwiftUI

// MARK: - Model

struct SessionFileItem: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let size: String
    let modified: String
}

// MARK: - Screen

/// Files mode of the session workbench. Read only by contract: the phone can
/// read and preview, but never pretends to be an editor.
struct SessionFilesView: View {
    @Environment(\.sePalette) private var palette

    var project: String
    var host: String
    var path: [String]
    var files: [SessionFileItem]
    var preview: [String]

    var onBack: () -> Void = {}
    var onSelectMode: (WorkspaceTab) -> Void = { _ in }
    var onOpenFile: (SessionFileItem) -> Void = { _ in }
    var onNavigateUp: () -> Void = {}

    var body: some View {
        VStack(spacing: 0) {
            header
            modeSwitch
                .padding(.horizontal, SeSpace.lg)
                .padding(.top, SeSpace.md)
            breadcrumb
                .padding(.horizontal, SeSpace.lg)
                .padding(.top, 12)
            fileList
            previewCard
        }
        .background(palette.canvas.ignoresSafeArea())
    }

    private var header: some View {
        HStack(spacing: 10) {
            iconButton(systemImage: "chevron.left", label: String(localized: "Back"), action: onBack)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: project)
                    .font(SeNocturneType.font(SeNocturneType.secondarySize + 1.5, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Text(verbatim: "\(host) · read only")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            Button(action: onNavigateUp) {
                Image(systemName: "arrow.up.left")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: 34, height: 34)
                    .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
            }
            .buttonStyle(.plain)
            .accessibilityLabel(String(localized: "Up one level"))
        }
        .padding(.horizontal, SeSpace.lg)
        .padding(.top, SeSpace.xs)
    }

    private var modeSwitch: some View {
        HStack(spacing: 3) {
            ForEach(WorkspaceTab.allCases) { tab in
                let isOn = tab == .files
                Button {
                    guard !isOn else { return }
                    onSelectMode(tab)
                } label: {
                    Text(tab.title)
                        .font(SeNocturneType.font(SeNocturneType.secondarySize, weight: isOn ? .medium : .regular))
                        .foregroundStyle(isOn ? palette.onAccent : palette.inkSecondary)
                        .frame(maxWidth: .infinity)
                        .frame(height: 30)
                        .background(
                            isOn ? palette.ink : .clear,
                            in: RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.control + 3)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
    }

    private var breadcrumb: some View {
        HStack(spacing: 5) {
            ForEach(Array(path.enumerated()), id: \.offset) { index, part in
                Text(verbatim: part)
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(index == path.count - 1 ? palette.ink : palette.inkTertiary)
                if index < path.count - 1 {
                    Text(verbatim: "/")
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkTertiary)
                }
            }
            Spacer(minLength: 8)
            Text("RO")
                .font(SeNocturneType.monoSmall)
                .foregroundStyle(palette.inkTertiary)
                .padding(.horizontal, 6)
                .frame(height: 18)
                .background(palette.card, in: RoundedRectangle(cornerRadius: 4))
                .overlay {
                    RoundedRectangle(cornerRadius: 4)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
        }
    }

    private var fileList: some View {
        ScrollView {
            VStack(spacing: 0) {
                ForEach(files) { file in
                    Button {
                        onOpenFile(file)
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: "doc")
                                .font(.system(size: 13, weight: .regular))
                                .foregroundStyle(palette.inkTertiary)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(verbatim: file.name)
                                    .font(SeNocturneType.rowTitle)
                                    .foregroundStyle(palette.ink)
                                    .lineLimit(1)
                                Text(verbatim: "\(file.size) · modified \(file.modified)")
                                    .font(SeNocturneType.font(SeNocturneType.labelSize))
                                    .foregroundStyle(palette.inkTertiary)
                                    .lineLimit(1)
                            }
                            Spacer(minLength: 8)
                            Image(systemName: "chevron.right")
                                .font(.system(size: 9, weight: .semibold))
                                .foregroundStyle(palette.inkTertiary)
                        }
                        .padding(.vertical, 12)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)

                    Rectangle()
                        .fill(palette.stroke)
                        .frame(height: 1)
                }
            }
            .padding(.horizontal, SeSpace.lg)
            .padding(.top, 4)
        }
        .scrollIndicators(.hidden)
    }

    private var previewCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Preview")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                Spacer()
                Text("Read only")
                    .font(SeNocturneType.monoSmall)
                    .foregroundStyle(palette.inkTertiary)
                    .padding(.horizontal, 6)
                    .frame(height: 18)
                    .background(palette.cardAlt, in: RoundedRectangle(cornerRadius: 4))
            }
            VStack(alignment: .leading, spacing: 2) {
                ForEach(Array(preview.enumerated()), id: \.offset) { _, line in
                    Text(verbatim: line.isEmpty ? " " : line)
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.codeInk)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(9)
            .background(palette.codeCanvas, in: RoundedRectangle(cornerRadius: 7))
            .overlay {
                RoundedRectangle(cornerRadius: 7)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
        }
        .padding(12)
        .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
        .overlay {
            RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                .strokeBorder(palette.stroke, lineWidth: 1)
        }
        .padding(.horizontal, SeSpace.lg)
        .padding(.vertical, 12)
    }

    private func iconButton(systemImage: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 14, weight: .regular))
                .foregroundStyle(palette.inkSecondary)
                .frame(width: 34, height: 34)
                .background(palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                .overlay {
                    RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                        .strokeBorder(palette.stroke, lineWidth: 1)
                }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

// MARK: - Previews

#Preview("Files · Night") {
    SessionFilesView(
        project: "Termul",
        host: "MacBook Pro",
        path: ["Termul", "src", "net"],
        files: [
            SessionFileItem(id: "f1", name: "TerminalSocket.swift", size: "8.2 KB", modified: "2m"),
            SessionFileItem(id: "f2", name: "ReconnectPolicy.swift", size: "3.1 KB", modified: "2m"),
            SessionFileItem(id: "f3", name: "AcpSocket.swift", size: "5.7 KB", modified: "yesterday"),
            SessionFileItem(id: "f4", name: "HostHTTP.swift", size: "9.4 KB", modified: "yesterday")
        ],
        preview: [
            "func reconnect() {",
            "  guard let id = sessionID else { return nil }",
            "  return resumeSession(id: id)",
            "}"
        ]
    )
    .seThemeMode(.night)
}

#Preview("Files · Day") {
    SessionFilesView(
        project: "Termul",
        host: "MacBook Pro",
        path: ["Termul", "src", "net"],
        files: [
            SessionFileItem(id: "f1", name: "TerminalSocket.swift", size: "8.2 KB", modified: "2m")
        ],
        preview: ["func reconnect() {"]
    )
    .seThemeMode(.day)
}
