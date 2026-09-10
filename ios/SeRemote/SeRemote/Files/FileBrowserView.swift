import SwiftUI

struct FileBrowserView: View {
    @Bindable var session: WorkspaceSession
    var embedded = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        Group {
            if embedded {
                embeddedBody
            } else {
                NavigationStack {
                    embeddedBody
                        .navigationTitle(session.files.previewName ?? session.files.crumbs.last?.name ?? String(localized: "Files"))
                        .navigationBarTitleDisplayMode(.inline)
                        .toolbar {
                            ToolbarItem(placement: .cancellationAction) {
                                Button("Close") { dismiss() }
                            }
                        }
                }
            }
        }
        .task {
            if session.files.crumbs.isEmpty {
                if let path = session.projects.active?.path {
                    await session.files.openRoot(path)
                } else if let cwd = session.conversations.active?.workspaceCwd {
                    await session.files.openRoot(cwd)
                }
            }
        }
    }

    @ViewBuilder
    private var embeddedBody: some View {
        VStack(spacing: 0) {
            crumbs
            if let preview = session.files.preview {
                previewPane(preview)
            } else {
                fileList
                desktopOnlyFooter
            }
        }
        .background(SeTheme.canvas)
    }

    private var crumbs: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 4) {
                ForEach(Array(session.files.crumbs.enumerated()), id: \.element.id) { index, crumb in
                    if index > 0 {
                        Text("/")
                            .font(.caption)
                            .foregroundStyle(SeTheme.muted)
                    }
                    Button {
                        if session.files.preview != nil {
                            session.files.preview = nil
                            session.files.previewName = nil
                        }
                        Task { await session.files.popTo(crumb) }
                    } label: {
                        Text(index == 0 ? "~" : crumb.name)
                            .font(.subheadline.weight(.medium))
                            .padding(.horizontal, 6)
                            .frame(minHeight: 44)
                    }
                    .disabled(index == session.files.crumbs.count - 1 && session.files.preview == nil)
                }
                if session.files.previewName != nil {
                    Text("/")
                        .font(.caption)
                        .foregroundStyle(SeTheme.muted)
                    Text(session.files.previewName ?? "")
                        .font(.subheadline.weight(.medium))
                        .padding(.horizontal, 6)
                }
            }
            .padding(.horizontal, 12)
        }
        .overlay(alignment: .bottom) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
        .accessibilityIdentifier("files-crumbs")
    }

    private func previewPane(_ preview: FileContent) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text(session.files.previewName ?? "")
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(1)
                Spacer(minLength: 8)
                Button {
                    UIPasteboard.general.string = preview.content
                } label: {
                    Image(systemName: "doc.on.doc")
                        .frame(minWidth: 36, minHeight: 36)
                }
                .accessibilityLabel(Text("Copy"))
                ShareLink(item: preview.content)
                    .frame(minWidth: 36, minHeight: 36)
                    .accessibilityLabel(Text("Share"))
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 2)
            .background(SeTheme.surface)
            .overlay(alignment: .bottom) {
                Rectangle().fill(SeTheme.stroke).frame(height: 1)
            }
            ScrollView {
                Text(preview.content)
                    .font(.body.monospaced())
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
                    .textSelection(.enabled)
            }
            .refreshable {
                await session.files.refresh()
            }
        }
    }

    /// The host's fs mutation routes are loopback-only by policy, so a paired
    /// phone can never create/rename/delete. State that explicitly instead of
    /// leaving the read-only tree to be discovered by trial.
    private var desktopOnlyFooter: some View {
        HStack(spacing: 6) {
            Image(systemName: "lock.shield")
                .font(.caption2)
            Text("File editing is desktop-only. Changes need the host's local web client.")
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 6)
        .background(.ultraThinMaterial)
        .overlay(alignment: .top) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
        .accessibilityIdentifier("files-desktop-only")
    }

    @ViewBuilder
    private var fileList: some View {
        List(session.files.entries) { entry in
            Button {
                Task { await session.files.open(entry) }
            } label: {
                Label(entry.name, systemImage: entry.isDirectory ? "folder" : "doc")
                    .foregroundStyle(.primary)
                    .frame(minHeight: 36, alignment: .leading)
            }
            .listRowBackground(SeTheme.canvas)
            .listRowSeparatorTint(SeTheme.stroke)
            .contextMenu {
                Button {
                    UIPasteboard.general.string = entry.path
                } label: {
                    Label(String(localized: "Copy Path"), systemImage: "doc.on.doc")
                }
            }
            .accessibilityLabel(Text(entry.name))
            .accessibilityHint(Text(entry.isDirectory ? "Folder" : "File"))
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .refreshable {
            await session.files.refresh()
        }
        .overlay {
            if session.files.isLoading {
                ProgressView()
            } else if session.files.entries.isEmpty {
                ContentUnavailableView(
                    "Empty folder",
                    systemImage: "folder",
                    description: Text("This directory has no visible files.")
                )
            }
        }
    }
}
