import SwiftUI

/// The pre-connection home: saved hosts and real open/connection timestamps.
@MainActor
struct HomeView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings

    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    @State private var showPairing = false
    @State private var showSettings = false
    @State private var pendingLink: RemoteLink?
    @State private var renamingLink: RemoteLink?
    @State private var renameTitle = ""
    @State private var deletingLink: RemoteLink?
    @State private var actionLink: RemoteLink?
    @State private var activity = ShellPairingActivity()

    private var recentHost: RemoteLink? {
        var best: (link: RemoteLink, date: Date)?
        for link in store.savedLinks {
            let date = activityDate(for: link)
            if best == nil || date > best!.date {
                best = (link, date)
            }
        }
        return best?.link
    }

    private func activityDate(for link: RemoteLink) -> Date {
        if let connected = activity.connectedDate(for: link) { return connected }
        if let opened = activity.date(for: link) { return opened }
        return link.createdAt
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    pageTop
                    if let recentHost {
                        resumeRow(recentHost)
                            .padding(.horizontal, 18)
                            .padding(.top, 12)
                    }

                    SeGroupLabel(title: String(localized: "Recent hosts"), count: String(format: "%02d", store.savedLinks.count))

                    if store.savedLinks.isEmpty {
                        emptyState
                            .padding(.horizontal, 18)
                    } else {
                        VStack(spacing: 0) {
                            ForEach(Array(store.savedLinks.enumerated()), id: \.element.id) { index, link in
                                SeHostRowView(
                                    index: String(format: "%02d", index + 1),
                                    name: link.title,
                                    meta: meta(for: link),
                                    latency: nil,
                                    online: store.activeLink?.id == link.id,
                                    onTap: { open(link) },
                                    onMore: { actionLink = link }
                                )
                                .contextMenu {
                                    Button("Open") { open(link) }
                                    Button("Rename") { beginRename(link) }
                                    Button("Remove", role: .destructive) { deletingLink = link }
                                }
                            }
                        }
                        .padding(.horizontal, 18)
                    }

                    SeLinkRow(title: String(localized: "Add host with pairing link")) {
                        store.dismissError()
                        showPairing = true
                    }
                    .padding(.horizontal, 18)
                    .padding(.top, 4)

                    if store.isPairing {
                        ProgressView("Pairing device…")
                            .font(SeNocturneType.font(12))
                            .padding(.horizontal, 22)
                    }
                    if let error = store.errorMessage {
                        SeErrorBanner(message: error)
                            .padding(.horizontal, 18)
                            .padding(.top, 8)
                    }

                    Color.clear
                        .frame(height: 120)
                }
                .frame(maxWidth: 680)
                .frame(maxWidth: .infinity)
            }

            dock
        }
        .background(palette.canvas.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .onAppear {
            activity.reload()
            consumePairingRequest()
        }
        // The connected screen asks for a scan when retrying cannot help; the
        // home screen owns the pairing sheet, so the request is consumed here.
        .onChange(of: store.pairingRequested) { _, requested in
            if requested { consumePairingRequest() }
        }
        .sheet(isPresented: $showPairing, onDismiss: connectPendingLink) {
            ShellPairingView { link in
                pendingLink = link
                showPairing = false
            }
            .presentationDetents([.large])
            .presentationDragIndicator(.hidden)
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(settings: settings, store: store)
        }
        .alert("Rename", isPresented: Binding(
            get: { renamingLink != nil },
            set: { if !$0 { renamingLink = nil } }
        ), presenting: renamingLink) { link in
            TextField("Name", text: $renameTitle)
            Button("Cancel", role: .cancel) { renamingLink = nil }
            Button("Save") {
                store.rename(link, title: renameTitle)
                renamingLink = nil
            }
            .disabled(renameTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .confirmationDialog("Host actions", isPresented: Binding(
            get: { actionLink != nil },
            set: { if !$0 { actionLink = nil } }
        ), titleVisibility: .visible, presenting: actionLink) { link in
            Button("Open") { actionLink = nil; open(link) }
            Button("Rename") { actionLink = nil; beginRename(link) }
            Button("Remove", role: .destructive) { actionLink = nil; deletingLink = link }
            Button("Cancel", role: .cancel) { actionLink = nil }
        }
        .confirmationDialog("Unpair host?", isPresented: Binding(
            get: { deletingLink != nil },
            set: { if !$0 { deletingLink = nil } }
        ), titleVisibility: .visible, presenting: deletingLink) { link in
            Button("Unpair", role: .destructive) {
                activity.forget(link)
                store.forget(link)
                deletingLink = nil
            }
            Button("Cancel", role: .cancel) { deletingLink = nil }
        } message: { _ in
            Text("This deletes the host and saved access key from this device. Your desktop is not changed.")
        }
        .tint(palette.accent)
    }

    // MARK: - Page

    private var pageTop: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                SeEyebrow(text: today)
                Spacer(minLength: 0)
                SeIconButton(icon: .refresh, accessibilityLabel: String(localized: "Refresh"), size: 34) {
                    activity.reload()
                }
                SeIconButton(icon: mode == .night ? .moon : .sun, accessibilityLabel: String(localized: "Toggle theme"), size: 34) {
                    settings.appearance = mode.toggled.appearance
                }
                SeIconButton(icon: .gear, accessibilityLabel: String(localized: "Settings"), size: 34) {
                    showSettings = true
                }
            }
            SeDisplayText(text: String(localized: "Hosts"))
                .padding(.top, 4)
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
    }

    private func resumeRow(_ link: RemoteLink) -> some View {
        SeResumeRow(
            title: link.title,
            meta: resumeMeta(link),
            actionTitle: String(localized: "Continue →"),
            onTap: { open(link) }
        )
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 2) {
                Text("No saved hosts yet")
                    .font(SeNocturneType.font(13, weight: .semibold))
                    .foregroundStyle(palette.ink)
                SeInfoTip(text: String(localized: "Open SeRemote on the Mac, then scan the QR code or paste the access link."))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(mode.hairline, lineWidth: 1)
        }
    }

    private var dock: some View {
        SeSolidButton(title: String(localized: "Pair a new host"), icon: .plus) {
            store.dismissError()
            showPairing = true
        }
        .frame(maxWidth: .infinity)
        .accessibilityIdentifier("home.pair")
        .padding(.horizontal, 18)
        .padding(.top, 20)
        .padding(.bottom, 28)
        .background(
            LinearGradient(
                stops: [
                    .init(color: palette.canvas.opacity(0), location: 0),
                    .init(color: palette.canvas.opacity(0.86), location: 0.45),
                    .init(color: palette.canvas, location: 0.72)
                ],
                startPoint: .top, endPoint: .bottom
            )
            .ignoresSafeArea(edges: .bottom)
        )
    }

    // MARK: - Data

    private var today: String {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.setLocalizedDateFormatFromTemplate("EEEE MMM d")
        return formatter.string(from: .now)
    }

    private func meta(for link: RemoteLink) -> String {
        if let date = activity.connectedDate(for: link) {
            return String(localized: "Last connected") + " " + relative(date)
        }
        if let date = activity.date(for: link) {
            return String(localized: "Last opened") + " " + relative(date)
        }
        return String(localized: "Saved") + " " + link.createdAt.formatted(date: .numeric, time: .omitted)
    }

    private func resumeMeta(_ link: RemoteLink) -> String {
        if let date = activity.connectedDate(for: link) { return String(localized: "Last connected") + " · " + relative(date) }
        if let date = activity.date(for: link) { return String(localized: "Last opened") + " · " + relative(date) }
        return String(localized: "Saved")
    }

    private func relative(_ date: Date) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.locale = .current
        formatter.unitsStyle = .short
        return formatter.localizedString(for: date, relativeTo: .now)
    }

    // MARK: - Actions

    private func beginRename(_ link: RemoteLink) {
        renameTitle = link.title
        renamingLink = link
    }

    private func open(_ link: RemoteLink) {
        // A rescan folds into the saved record it belongs to, so record the open
        // against the record that survives rather than the freshly parsed link.
        let stored = store.connect(link: link)
        activity.recordOpen(stored)
    }

    private func connectPendingLink() {
        guard let link = pendingLink else { store.cancelPairing(); return }
        pendingLink = nil
        Task {
            do {
                let stored = try await store.connectScanned(link)
                activity.recordOpen(stored)
            } catch {
                store.errorMessage = error.localizedDescription
            }
        }
    }

    private func consumePairingRequest() {
        guard store.pairingRequested else { return }
        store.pairingRequested = false
        store.dismissError()
        showPairing = true
    }
}
