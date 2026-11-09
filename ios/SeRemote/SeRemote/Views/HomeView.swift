import SwiftUI

@MainActor
struct HomeView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var typeSize
    @Environment(\.sePalette) private var palette
    @State private var selectedHostID: UUID?
    @State private var usesSplitLayout = false
    @State private var showPairing = false
    @State private var showSettings = false
    @State private var pendingLink: RemoteLink?
    @State private var renamingLink: RemoteLink?
    @State private var renameTitle = ""
    @State private var deletingLink: RemoteLink?
    @State private var activity = ShellPairingActivity()

    var body: some View {
        GeometryReader { geometry in
            let split =
                SeLayout.columns(
                    width: geometry.size.width,
                    isRegular: sizeClass == .regular,
                    largeText: typeSize.isAccessibilitySize
                ) != .single
            Group {
                if split {
                    NavigationSplitView {
                        hostList
                            .navigationSplitViewColumnWidth(min: 280, ideal: 320, max: 380)
                    } detail: {
                        selectedHostDetail
                    }
                    .navigationSplitViewStyle(.balanced)
                } else {
                    NavigationStack { hostList }
                }
            }
            .onChange(of: split, initial: true) { _, value in usesSplitLayout = value }
        }
        .onAppear { activity.reload() }
        .sheet(isPresented: $showPairing, onDismiss: connectPendingLink) {
            ShellPairingView { link in
                pendingLink = link
                showPairing = false
            }
            .presentationDetents([.large])
            .presentationDragIndicator(.visible)
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(settings: settings, store: store)
        }
        .alert(
            "Rename",
            isPresented: Binding(
                get: { renamingLink != nil },
                set: { if !$0 { renamingLink = nil } }
            ), presenting: renamingLink
        ) { link in
            TextField("Name", text: $renameTitle)
            Button("Cancel", role: .cancel) { renamingLink = nil }
            Button("Save") {
                store.rename(link, title: renameTitle)
                renamingLink = nil
            }
            .disabled(renameTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .confirmationDialog(
            "Delete saved host?",
            isPresented: Binding(
                get: { deletingLink != nil },
                set: { if !$0 { deletingLink = nil } }
            ), titleVisibility: .visible, presenting: deletingLink
        ) { link in
            Button("Delete", role: .destructive) {
                activity.forget(link)
                store.forget(link)
                deletingLink = nil
            }
            Button("Cancel", role: .cancel) { deletingLink = nil }
        } message: { _ in
            Text("This removes the saved host and its access secret from this device. It does not change your desktop.")
        }
        .tint(palette.accent)
    }

    private var hostList: some View {
        List {
            Section {
                introduction
                    .listRowSeparator(.hidden)
                    .listRowInsets(rowInsets)
            }

            Section {
                if store.savedLinks.isEmpty {
                    SeEmptyState(
                        title: String(localized: "Your desktop, within reach."),
                        systemImage: "desktopcomputer",
                        message: String(
                            localized: "Pair a trusted host to return to your sessions, terminal and files.")
                    )
                    .listRowSeparator(.hidden)
                    .listRowInsets(rowInsets)
                } else {
                    ForEach(store.savedLinks) { link in
                        hostRow(link)
                            .listRowBackground(
                                Color.clear
                            )
                            .accessibilityAddTraits(usesSplitLayout && selectedHost?.id == link.id ? .isSelected : [])
                            .listRowInsets(EdgeInsets(top: SeSpace.xs, leading: SeSpace.screenInset, bottom: SeSpace.xs, trailing: SeSpace.screenInset))
                            .listRowSeparator(.hidden)
                            .contextMenu { hostActions(link) }
                            .swipeActions(edge: .trailing, allowsFullSwipe: false) {
                                Button(role: .destructive) {
                                    deletingLink = link
                                } label: {
                                    Label("Delete", systemImage: "trash")
                                }
                                Button {
                                    beginRename(link)
                                } label: {
                                    Label("Rename", systemImage: "pencil")
                                }
                                .tint(palette.inkSecondary)
                            }
                            .swipeActions(edge: .leading, allowsFullSwipe: false) {
                                Button {
                                    open(link)
                                } label: {
                                    Label("Open", systemImage: "arrow.up.right")
                                }
                                .tint(palette.accent)
                            }
                            .accessibilityAction(named: Text("Rename")) { beginRename(link) }
                            .accessibilityAction(named: Text("Delete")) { deletingLink = link }
                    }
                }
            } header: {
                Text("Saved hosts")
                    .font(SeNocturneType.sectionTitle)
                    .foregroundStyle(palette.inkSecondary)
                    .padding(.top, SeSpace.sm)
                    .textCase(nil)
            }

            Section {
                pairingAction
                    .listRowSeparator(.hidden)
                    .listRowInsets(rowInsets)
            }

            if let error = store.errorMessage {
                Section {
                    SeErrorBanner(message: error)
                        .accessibilityIdentifier("home.error")
                        .listRowSeparator(.hidden)
                        .listRowInsets(rowInsets)
                }
            }
        }
        .listStyle(.plain)
        .listRowBackground(palette.canvas)
        .scrollContentBackground(.hidden)
        .frame(maxWidth: SeLayout.readableColumnMaxWidth)
        .frame(maxWidth: .infinity)
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Text("SeRemote")
                    .font(SeFont.identity)
                    .foregroundStyle(palette.ink)
                    .accessibilityAddTraits(.isHeader)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    showSettings = true
                } label: {
                    Image(systemName: "gearshape")
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                }
                .accessibilityLabel("Settings")
            }
        }
    }

    private var selectedHost: RemoteLink? {
        store.savedLinks.first { $0.id == selectedHostID } ?? store.savedLinks.first
    }

    private var selectedHostDetail: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SeSpace.xl) {
                if let link = selectedHost {
                    Image(systemName: "desktopcomputer")
                        .font(.largeTitle)
                        .foregroundStyle(palette.inkSecondary)
                        .accessibilityHidden(true)
                    Text(link.title)
                        .font(SeFont.screenTitle)
                        .accessibilityAddTraits(.isHeader)
                    Text(link.originHost)
                        .font(SeFont.body)
                        .foregroundStyle(palette.inkSecondary)
                    Label("Not connected", systemImage: "circle.slash")
                        .font(SeFont.support)
                        .foregroundStyle(palette.inkSecondary)
                    lastActivityLabel(link)
                    Button {
                        open(link)
                    } label: {
                        Label("Open host", systemImage: "arrow.up.right")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(SePrimaryButtonStyle())
                    HStack(spacing: SeSpace.md) {
                        Button {
                            beginRename(link)
                        } label: {
                            Label("Rename", systemImage: "pencil")
                        }
                        Spacer()
                        Button(role: .destructive) {
                            deletingLink = link
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
                    .frame(minHeight: SeSpace.hitTarget)
                    Text("Host status is checked when you open it. Git and Files are read-only.")
                        .font(SeFont.support)
                        .foregroundStyle(palette.inkSecondary)
                } else {
                    introduction
                    pairingAction
                }
            }
            .frame(maxWidth: SeLayout.readableColumnMaxWidth, alignment: .leading)
            .padding(SeSpace.xxl)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(palette.canvas)
        .foregroundStyle(palette.ink)
        .navigationTitle(selectedHost?.title ?? String(localized: "Your hosts"))
        .navigationBarTitleDisplayMode(.inline)
    }

    private var rowInsets: EdgeInsets {
        EdgeInsets(top: SeSpace.md, leading: SeSpace.screenInset, bottom: SeSpace.md, trailing: SeSpace.screenInset)
    }

    private var introduction: some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            Text("REMOTE WORKSPACE")
                .font(SeFont.label)
                .tracking(1.2)
                .foregroundStyle(palette.inkSecondary)
            Text("Your hosts.")
                .font(SeFont.screenTitle)
                .accessibilityAddTraits(.isHeader)
            Text("Connect to a trusted desktop to return to your sessions, terminal and files.")
                .font(SeFont.body)
                .foregroundStyle(palette.inkSecondary)
        }
        .padding(.vertical, SeSpace.md)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func hostRow(_ link: RemoteLink) -> some View {
        Button {
            selectedHostID = link.id
            if !usesSplitLayout { open(link) }
        } label: {
            HStack(alignment: .top, spacing: SeSpace.md) {
                Image(systemName: "desktopcomputer")
                    .font(.title3)
                    .foregroundStyle(palette.inkSecondary)
                    .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                    .overlay {
                        RoundedRectangle(cornerRadius: SeNocturneRadius.control)
                            .strokeBorder(palette.stroke, lineWidth: 1)
                    }
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: SeSpace.xs) {
                    Text(link.title)
                        .font(SeFont.rowTitle)
                        .foregroundStyle(palette.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(link.originHost)
                        .font(SeFont.support)
                        .foregroundStyle(palette.inkSecondary)
                        .lineLimit(2)
                    Label {
                        Text(store.activeLink?.id == link.id ? "Connecting" : "Not connected")
                    } icon: {
                        Image(
                            systemName: store.activeLink?.id == link.id ? "arrow.triangle.2.circlepath" : "circle.slash"
                        )
                    }
                    .font(SeFont.caption)
                    .foregroundStyle(palette.inkSecondary)
                    lastActivityLabel(link)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                Image(systemName: "chevron.right")
                    .font(SeFont.caption)
                    .foregroundStyle(palette.inkSecondary)
                    .padding(.top, SeSpace.sm)
                    .accessibilityHidden(true)
            }
            .padding(SeSpace.md)
            .background(usesSplitLayout && selectedHost?.id == link.id ? palette.cardAlt : palette.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.card))
            .overlay {
                RoundedRectangle(cornerRadius: SeNocturneRadius.card)
                    .strokeBorder(usesSplitLayout && selectedHost?.id == link.id ? palette.strokeStrong : palette.stroke, lineWidth: 1)
            }
            .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressableRowStyle())
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("home.host.\(link.id.uuidString)")
        .accessibilityHint(usesSplitLayout ? Text("Show host details") : Text("Open this host"))
    }

    @ViewBuilder
    private func lastActivityLabel(_ link: RemoteLink) -> some View {
        if let date = activity.connectedDate(for: link) {
            Text("Last connected \(date, format: .relative(presentation: .named))")
                .font(SeFont.timing)
                .foregroundStyle(palette.inkSecondary)
        } else if let date = activity.date(for: link) {
            // An Open action alone is not evidence of a completed handshake.
            Text("Last opened \(date, format: .relative(presentation: .named))")
                .font(SeFont.timing)
                .foregroundStyle(palette.inkSecondary)
        } else {
            Text("Added \(link.createdAt, style: .date)")
                .font(SeFont.timing)
                .foregroundStyle(palette.inkSecondary)
        }
    }

    private var pairingAction: some View {
        VStack(alignment: .leading, spacing: SeSpace.md) {
            Button {
                store.dismissError()
                showPairing = true
            } label: {
                Label("Pair a host", systemImage: "plus")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(SePrimaryButtonStyle())
            .accessibilityIdentifier("home.pair")

            Text("Open SeRemote on your desktop. Scan or paste its access link. You choose when to connect.")
                .font(SeFont.support)
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)

            HStack(alignment: .top, spacing: SeSpace.sm) {
                Rectangle().fill(palette.accent).frame(width: 2)
                Text("Git and Files are read-only. Pairing secrets stay in the Keychain on this phone.")
                    .font(SeFont.support)
                    .foregroundStyle(palette.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.vertical, SeSpace.md)
    }

    @ViewBuilder
    private func hostActions(_ link: RemoteLink) -> some View {
        Button {
            open(link)
        } label: {
            Label("Open", systemImage: "arrow.up.right")
        }
        Button {
            beginRename(link)
        } label: {
            Label("Rename", systemImage: "pencil")
        }
        Button(role: .destructive) {
            deletingLink = link
        } label: {
            Label("Delete", systemImage: "trash")
        }
    }

    private func beginRename(_ link: RemoteLink) {
        renameTitle = link.title
        renamingLink = link
    }

    private func open(_ link: RemoteLink) {
        activity.recordOpen(link)
        store.connect(link: link)
    }

    private func connectPendingLink() {
        guard let link = pendingLink else { return }
        pendingLink = nil
        open(link)
    }
}
