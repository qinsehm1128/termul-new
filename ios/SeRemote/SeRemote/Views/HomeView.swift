import SwiftUI

@MainActor
struct HomeView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dynamicTypeSize) private var typeSize
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
            SettingsView(settings: settings)
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
        .tint(SeTheme.accent)
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
                                usesSplitLayout && selectedHost?.id == link.id ? SeTheme.elevated : SeTheme.canvas
                            )
                            .accessibilityAddTraits(usesSplitLayout && selectedHost?.id == link.id ? .isSelected : [])
                            .listRowInsets(rowInsets)
                            .listRowSeparatorTint(SeTheme.stroke)
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
                                .tint(SeTheme.secondaryInk)
                            }
                            .swipeActions(edge: .leading, allowsFullSwipe: false) {
                                Button {
                                    open(link)
                                } label: {
                                    Label("Open", systemImage: "arrow.up.right")
                                }
                                .tint(SeTheme.accent)
                            }
                            .accessibilityAction(named: Text("Rename")) { beginRename(link) }
                            .accessibilityAction(named: Text("Delete")) { deletingLink = link }
                    }
                }
            } header: {
                SeSectionHeader(title: String(localized: "Recent hosts"))
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
                        .listRowSeparator(.hidden)
                        .listRowInsets(rowInsets)
                }
            }
        }
        .listStyle(.plain)
        .listRowBackground(SeTheme.canvas)
        .scrollContentBackground(.hidden)
        .frame(maxWidth: SeLayout.readableColumnMaxWidth)
        .frame(maxWidth: .infinity)
        .background(SeTheme.canvas)
        .foregroundStyle(SeTheme.ink)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Text("SeRemote")
                    .font(SeFont.identity)
                    .foregroundStyle(SeTheme.ink)
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
                        .foregroundStyle(SeTheme.secondaryInk)
                        .accessibilityHidden(true)
                    Text(link.title)
                        .font(SeFont.screenTitle)
                        .accessibilityAddTraits(.isHeader)
                    Text(link.originHost)
                        .font(SeFont.body)
                        .foregroundStyle(SeTheme.secondaryInk)
                    Label("Not connected", systemImage: "circle.slash")
                        .font(SeFont.support)
                        .foregroundStyle(SeTheme.secondaryInk)
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
                        .foregroundStyle(SeTheme.secondaryInk)
                } else {
                    introduction
                    pairingAction
                }
            }
            .frame(maxWidth: SeLayout.readableColumnMaxWidth, alignment: .leading)
            .padding(SeSpace.xxl)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(SeTheme.canvas)
        .foregroundStyle(SeTheme.ink)
        .navigationTitle(selectedHost?.title ?? String(localized: "Your field console"))
        .navigationBarTitleDisplayMode(.inline)
    }

    private var rowInsets: EdgeInsets {
        EdgeInsets(top: SeSpace.md, leading: SeSpace.screenInset, bottom: SeSpace.md, trailing: SeSpace.screenInset)
    }

    private var introduction: some View {
        VStack(alignment: .leading, spacing: SeSpace.sm) {
            Text("YOUR FIELD CONSOLE")
                .font(SeFont.label)
                .tracking(1.2)
                .foregroundStyle(SeTheme.live)
            Text("Back to the field.")
                .font(SeFont.screenTitle)
                .accessibilityAddTraits(.isHeader)
            Text("Your hosts and sessions, right here.")
                .font(SeFont.body)
                .foregroundStyle(SeTheme.secondaryInk)
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
                    .foregroundStyle(SeTheme.secondaryInk)
                    .frame(width: SeSpace.hitTarget, height: SeSpace.hitTarget)
                    .overlay {
                        RoundedRectangle(cornerRadius: SeRadius.content)
                            .strokeBorder(SeTheme.stroke, lineWidth: 1)
                    }
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: SeSpace.xs) {
                    Text(link.title)
                        .font(SeFont.rowTitle)
                        .foregroundStyle(SeTheme.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(link.originHost)
                        .font(SeFont.support)
                        .foregroundStyle(SeTheme.secondaryInk)
                        .lineLimit(2)
                    Label {
                        Text(store.activeLink?.id == link.id ? "Connecting" : "Not connected")
                    } icon: {
                        Image(
                            systemName: store.activeLink?.id == link.id ? "arrow.triangle.2.circlepath" : "circle.slash"
                        )
                    }
                    .font(SeFont.caption)
                    .foregroundStyle(SeTheme.secondaryInk)
                    lastActivityLabel(link)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                Image(systemName: "chevron.right")
                    .font(SeFont.caption)
                    .foregroundStyle(SeTheme.tertiaryInk)
                    .padding(.top, SeSpace.sm)
                    .accessibilityHidden(true)
            }
            .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressableRowStyle())
        .accessibilityElement(children: .combine)
        .accessibilityHint(usesSplitLayout ? Text("Show host details") : Text("Open this host"))
    }

    @ViewBuilder
    private func lastActivityLabel(_ link: RemoteLink) -> some View {
        if let date = activity.connectedDate(for: link) {
            Text("Last connected \(date, format: .relative(presentation: .named))")
                .font(SeFont.timing)
                .foregroundStyle(SeTheme.tertiaryInk)
        } else if let date = activity.date(for: link) {
            // An Open action alone is not evidence of a completed handshake.
            Text("Last opened \(date, format: .relative(presentation: .named))")
                .font(SeFont.timing)
                .foregroundStyle(SeTheme.tertiaryInk)
        } else {
            Text("Added \(link.createdAt, style: .date)")
                .font(SeFont.timing)
                .foregroundStyle(SeTheme.tertiaryInk)
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
                .foregroundStyle(SeTheme.secondaryInk)
                .fixedSize(horizontal: false, vertical: true)

            HStack(alignment: .top, spacing: SeSpace.sm) {
                Rectangle().fill(SeTheme.accent).frame(width: 2)
                Text("Connect to a trusted host. Pick up where you left off.")
                    .font(SeFont.support)
                    .foregroundStyle(SeTheme.secondaryInk)
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
