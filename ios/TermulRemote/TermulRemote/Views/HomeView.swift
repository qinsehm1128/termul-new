import SwiftUI
import UIKit

struct HomeView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings
    @State private var draftURL = ""
    @State private var showSettings = false
    @State private var pendingScan: String?
    @State private var renaming: RemoteLink?
    @State private var renameDraft = ""
    @FocusState private var urlFocused: Bool

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                header
                composer
                recents
            }
            .padding(.horizontal, 20)
            .padding(.top, 20)
            .padding(.bottom, 40)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .background(SeTheme.canvas.ignoresSafeArea())
        .safeAreaInset(edge: .top, spacing: 0) {
            toolbar
        }
        .fullScreenCover(isPresented: $store.isScanning, onDismiss: applyPendingScan) {
            ScannerView { scanned in
                pendingScan = scanned
                store.isScanning = false
            }
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(settings: settings)
        }
        .alert(
            String(localized: "Rename desk"),
            isPresented: Binding(
                get: { renaming != nil },
                set: { if !$0 { renaming = nil } }
            )
        ) {
            TextField(String(localized: "Desk name"), text: $renameDraft)
            Button(String(localized: "Save")) {
                if let link = renaming {
                    store.rename(link, title: renameDraft)
                }
                renaming = nil
            }
            Button(String(localized: "Cancel"), role: .cancel) {
                renaming = nil
            }
        }
    }

    private var toolbar: some View {
        HStack {
            Text("Termul Remote")
                .font(.system(.headline, design: .serif))
            Spacer()
            Button {
                showSettings = true
            } label: {
                Image(systemName: "gearshape")
                    .font(.body)
                    .frame(minWidth: 44, minHeight: 44)
            }
            .accessibilityLabel(Text("Settings"))
        }
        .padding(.horizontal, 12)
        .background(SeTheme.canvas.opacity(0.92))
        .overlay(alignment: .bottom) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Termul Remote")
                .font(.caption.weight(.medium))
                .foregroundStyle(SeTheme.muted)
                .textCase(.uppercase)
                .tracking(1.2)
            Text("Open the desk session")
                .font(SeTheme.wordmark)
                .foregroundStyle(.primary)
                .fixedSize(horizontal: false, vertical: true)
            Text("Scan the desktop QR, or paste the access link. A LAN address stays on Wi-Fi; a Quick Tunnel still goes through Cloudflare.")
                .font(.body)
                .foregroundStyle(.secondary)
        }
    }

    private var composer: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .center, spacing: 10) {
                TextField("Paste access link", text: $draftURL, axis: .vertical)
                    .textContentType(.URL)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .lineLimit(1 ... 3)
                    .submitLabel(.go)
                    .focused($urlFocused)
                    .onSubmit { openDraft() }
                    .frame(minHeight: 44)

                Button {
                    openScanner()
                } label: {
                    Image(systemName: "qrcode.viewfinder")
                        .font(.title3)
                        .frame(minWidth: 44, minHeight: 44)
                }
                .accessibilityLabel(Text("Scan QR code"))
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            .background(SeTheme.surface)
            .clipShape(RoundedRectangle(cornerRadius: SeTheme.radius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: SeTheme.radius, style: .continuous)
                    .stroke(SeTheme.stroke, lineWidth: 1)
            )

            if let error = store.errorMessage, !error.isEmpty {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .accessibilityIdentifier("pair-field-error")
            }

            Button {
                openDraft()
            } label: {
                Text("Open session")
                    .font(.body.bold())
                    .frame(maxWidth: .infinity)
                    .frame(minHeight: 48)
            }
            .buttonStyle(.borderedProminent)
            .tint(SeTheme.accent)
            .disabled(draftURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }

    @ViewBuilder
    private var recents: some View {
        if store.savedLinks.isEmpty {
            ContentUnavailableView(
                "No saved desks",
                systemImage: "laptopcomputer.and.iphone",
                description: Text("A scanned or pasted link stays here for the next open.")
            )
            .frame(maxWidth: .infinity)
            .padding(.top, 12)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                Text("Recent desks")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(SeTheme.muted)
                    .padding(.bottom, 8)
                ForEach(store.savedLinks) { link in
                    RecentDeskRow(
                        link: link,
                        onOpen: { openSaved(link) },
                        onRename: {
                            renameDraft = link.title
                            renaming = link
                        },
                        onDelete: { store.forget(link) }
                    )
                }
            }
        }
    }

    private func openScanner() {
        resignKeyboard()
        store.isScanning = true
    }

    private func openDraft() {
        resignKeyboard()
        store.connect(to: draftURL)
    }

    private func openSaved(_ link: RemoteLink) {
        resignKeyboard()
        store.connect(link: link)
    }

    private func applyPendingScan() {
        guard let scanned = pendingScan else { return }
        pendingScan = nil
        draftURL = scanned
        resignKeyboard()
        store.connect(to: scanned)
    }

    private func resignKeyboard() {
        urlFocused = false
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
}

private struct RecentDeskRow: View {
    let link: RemoteLink
    var onOpen: () -> Void
    var onRename: () -> Void
    var onDelete: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Button(action: onOpen) {
                HStack(spacing: 12) {
                    Image(systemName: "desktopcomputer")
                        .font(.body)
                        .foregroundStyle(SeTheme.muted)
                        .frame(width: 28, height: 28)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(link.title)
                            .font(.body.weight(.semibold))
                            .foregroundStyle(.primary)
                        Text(link.originHost)
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 8)
                    Circle()
                        .fill(SeTheme.lamp)
                        .frame(width: 8, height: 8)
                        .accessibilityHidden(true)
                }
                .padding(.vertical, 12)
                .frame(minHeight: 56)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(link.title))

            Menu {
                Button(String(localized: "Open chat"), action: onOpen)
                Button(String(localized: "Rename desk"), action: onRename)
                Button(String(localized: "Remove"), role: .destructive, action: onDelete)
            } label: {
                Image(systemName: "ellipsis.circle")
                    .font(.body)
                    .frame(minWidth: 44, minHeight: 44)
            }
            .accessibilityLabel(Text("Desk actions"))
        }
        .overlay(alignment: .bottom) {
            Rectangle().fill(SeTheme.stroke).frame(height: 1)
        }
        .contextMenu {
            Button(String(localized: "Open chat"), action: onOpen)
            Button(String(localized: "Rename desk"), action: onRename)
            Button(String(localized: "Remove"), role: .destructive, action: onDelete)
        }
    }
}
