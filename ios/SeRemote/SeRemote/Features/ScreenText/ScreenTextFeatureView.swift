import SwiftUI
import UIKit

/// Manual, read-only host observation. It never watches, claims, or mutates a PTY.
struct ScreenTextFeatureView: View {
    @Environment(\.sePalette) private var p
    var session: WorkspaceSession
    @State private var selectedId: String?
    @State private var reply: HostScreenReadReply?
    @State private var capturedAt: Date?
    @State private var loading = false
    @State private var errorMessage: String?
    @State private var copied = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Label(session.origin.host() ?? String(localized: "Connected host"), systemImage: "server.rack")
                    .toolNocturneFont(.label).foregroundStyle(p.inkSecondary)
                Text("A read-only text snapshot from the host. This is not live screen sharing.")
                    .toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                if session.terminals.terminals.isEmpty {
                    ToolNocturneEmptyState(title: String(localized: "No terminals"), systemImage: "terminal",
                        message: String(localized: "Open a terminal in this workspace first, then return to capture its text."))
                } else {
                    Picker("Terminal", selection: $selectedId) {
                        Text("Choose a terminal").tag(String?.none)
                        ForEach(session.terminals.terminals) { terminal in
                            Text(terminal.title.isEmpty ? terminal.id : terminal.title).tag(Optional(terminal.id))
                        }
                    }
                    .pickerStyle(.menu).frame(minHeight: 44)
                    .padding(.horizontal, 12)
                    .background(p.card, in: RoundedRectangle(cornerRadius: SeNocturneRadius.control))
                    .overlay { RoundedRectangle(cornerRadius: SeNocturneRadius.control).strokeBorder(p.stroke) }
                    .disabled(loading)
                    Button {
                        Task { await capture() }
                    } label: {
                        Label("Refresh snapshot", systemImage: "arrow.clockwise")
                            .frame(minHeight: 44)
                    }
                    .buttonStyle(ToolNocturneButtonStyle()).disabled(loading || selectedId == nil)
                }
                if loading { ToolNocturneProgress(String(localized: "Reading up to 80 lines…")) }
                if let errorMessage {
                    ToolNocturneErrorBanner(message: errorMessage) { Task { await capture() } }
                }
                if let reply {
                    VStack(alignment: .leading, spacing: 12) {
                        HStack {
                            ToolNocturneStatusBadge(status: ScreenTextPresentation.semanticStatus(reply.status.state))
                            Text(reply.status.state.rawValue.capitalized).toolNocturneFont(.label)
                        }
                        Text(ScreenTextPresentation.explanation(reply.status.state)).toolNocturneFont(.support)
                        if let reason = reply.status.reason {
                            Label(reason.rawValue, systemImage: "info.circle")
                                .toolNocturneFont(.caption).foregroundStyle(p.inkSecondary)
                        }
                        if let capturedAt {
                            LabeledContent("Captured") { Text(capturedAt, format: .dateTime.hour().minute().second()) }
                                .toolNocturneFont(.caption)
                        }
                        if reply.truncated {
                            Label("Only the last 80 lines are shown. The host truncated this snapshot.", systemImage: "text.badge.minus")
                                .toolNocturneFont(.support).foregroundStyle(p.warning)
                        }
                        if let text = reply.text {
                            ToolNocturneCodeBlock(text: text)
                            if text.isEmpty {
                                Text("The terminal screen is empty.").toolNocturneFont(.support).foregroundStyle(p.inkSecondary)
                            }
                            Button("Copy snapshot", systemImage: "doc.on.doc") {
                                UIPasteboard.general.string = text
                                copied += 1
                            }
                            .frame(minHeight: 44)
                            .buttonStyle(ToolNocturneButtonStyle())
                        } else {
                            ToolNocturneEmptyState(title: String(localized: "Screen text is not available"), systemImage: "text.viewfinder",
                                message: String(localized: "No text is shown because the host could not confirm a consistent screen. Refresh after the terminal settles."))
                        }
                    }
                } else if !loading && errorMessage == nil && !session.terminals.terminals.isEmpty {
                    ToolNocturneEmptyState(title: String(localized: "Ready to capture"), systemImage: "text.viewfinder",
                        message: String(localized: "Select a terminal and refresh. Snapshots do not update automatically."))
                }
            }.padding(20)
        }
        .background(p.canvas)
        .foregroundStyle(p.ink)
        .toolNocturneFont(.body)
        .tint(p.accent)
        .navigationTitle("Host screen text")
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
