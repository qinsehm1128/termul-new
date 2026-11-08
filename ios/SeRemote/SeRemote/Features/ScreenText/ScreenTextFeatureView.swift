import SwiftUI
import UIKit

/// Manual, read-only host observation. It never watches, claims, or mutates a PTY.
struct ScreenTextFeatureView: View {
    var session: WorkspaceSession
    @State private var selectedId: String?
    @State private var reply: HostScreenReadReply?
    @State private var capturedAt: Date?
    @State private var loading = false
    @State private var errorMessage: String?
    @State private var copied = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SeSpace.md) {
                Label(session.origin.host() ?? String(localized: "Connected host"), systemImage: "server.rack")
                    .font(SeFont.label).foregroundStyle(SeTheme.secondaryInk)
                Text("A read-only text snapshot from the host. This is not live screen sharing.")
                    .font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                if session.terminals.terminals.isEmpty {
                    SeEmptyState(title: String(localized: "No terminals"), systemImage: "terminal",
                        message: String(localized: "Open a terminal in this workspace first, then return to capture its text."))
                } else {
                    Picker("Terminal", selection: $selectedId) {
                        Text("Choose a terminal").tag(String?.none)
                        ForEach(session.terminals.terminals) { terminal in
                            Text(terminal.title.isEmpty ? terminal.id : terminal.title).tag(Optional(terminal.id))
                        }
                    }
                    .pickerStyle(.menu).frame(minHeight: SeSpace.hitTarget)
                    .disabled(loading)
                    Button {
                        Task { await capture() }
                    } label: {
                        Label("Refresh snapshot", systemImage: "arrow.clockwise")
                            .frame(minHeight: SeSpace.hitTarget)
                    }
                    .buttonStyle(SeSecondaryButtonStyle()).disabled(loading || selectedId == nil)
                }
                if loading { ProgressView("Reading up to 80 lines…") }
                if let errorMessage {
                    SeErrorBanner(message: errorMessage) { Task { await capture() } }
                }
                if let reply {
                    VStack(alignment: .leading, spacing: SeSpace.sm) {
                        HStack {
                            SeStatusBadge(status: ScreenTextPresentation.semanticStatus(reply.status.state))
                            Text(reply.status.state.rawValue.capitalized).font(SeFont.label)
                        }
                        Text(ScreenTextPresentation.explanation(reply.status.state)).font(SeFont.support)
                        if let reason = reply.status.reason {
                            Label(reason.rawValue, systemImage: "info.circle")
                                .font(SeFont.caption).foregroundStyle(SeTheme.secondaryInk)
                        }
                        if let capturedAt {
                            LabeledContent("Captured") { Text(capturedAt, format: .dateTime.hour().minute().second()) }
                                .font(SeFont.caption)
                        }
                        if reply.truncated {
                            Label("Only the last 80 lines are shown. The host truncated this snapshot.", systemImage: "text.badge.minus")
                                .font(SeFont.support).foregroundStyle(SeTheme.warning)
                        }
                        if let text = reply.text {
                            SeCodeBlock(text: text)
                            if text.isEmpty {
                                Text("The terminal screen is empty.").font(SeFont.support).foregroundStyle(SeTheme.secondaryInk)
                            }
                            Button("Copy snapshot", systemImage: "doc.on.doc") {
                                UIPasteboard.general.string = text
                                copied += 1
                            }
                            .frame(minHeight: SeSpace.hitTarget)
                            .buttonStyle(SeSecondaryButtonStyle())
                        } else {
                            SeEmptyState(title: String(localized: "Screen text is not available"), systemImage: "text.viewfinder",
                                message: String(localized: "No text is shown because the host could not confirm a consistent screen. Refresh after the terminal settles."))
                        }
                    }
                } else if !loading && errorMessage == nil && !session.terminals.terminals.isEmpty {
                    SeEmptyState(title: String(localized: "Ready to capture"), systemImage: "text.viewfinder",
                        message: String(localized: "Select a terminal and refresh. Snapshots do not update automatically."))
                }
            }.padding(SeSpace.screenInset)
        }
        .background(SeTheme.canvas).navigationTitle("Host screen text")
        .seHaptic(.selection, trigger: copied)
        .task {
            selectedId = session.terminals.activeId ?? session.terminals.terminals.first?.id
            if selectedId != nil { await capture() }
        }
        .onChange(of: selectedId) { _, _ in
            reply = nil
            capturedAt = nil
            errorMessage = nil
        }
    }


    private func capture() async {
        guard !loading, let id = selectedId else { return }
        loading = true
        errorMessage = nil
        reply = nil
        capturedAt = nil
        defer { loading = false }
        do {
            let observation = try await session.terminalSocket.screenRead(terminalId: id, maxLines: 80)
            guard selectedId == id else { return }
            reply = observation
            capturedAt = .now
        } catch {
            guard selectedId == id else { return }
            errorMessage = error.localizedDescription
            HostLog.ui.error("Host screen text capture failed")
        }
    }
}
