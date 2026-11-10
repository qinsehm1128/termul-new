import SwiftUI
import UIKit

/// 快照 segment of the 屏幕 page: a manual, read-only host observation. It never
/// watches, claims or mutates a PTY. Text is gated on a correlated `appliedSeq`;
/// when the host cannot prove the snapshot matches a ready observer, the text
/// stays hidden.
///
/// Page chrome (nav bar, 快照 / 桌面 switch) belongs to `ScreenFeatureView`; this
/// view only owns the scrolling snapshot content and the terminal selection.
struct ScreenTextFeatureView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    let session: WorkspaceSession
    @Binding var selectedId: String?

    @State private var reply: HostScreenReadReply?
    @State private var capturedAt: Date?
    @State private var loading = false
    @State private var errorMessage: String?
    @State private var copied = 0

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                content
            }
            .padding(.bottom, 30)
        }
        .scrollIndicators(.hidden)
        .refreshable { await capture() }
        .task {
            if selectedId == nil {
                selectedId = session.terminals.activeId ?? session.terminals.terminals.first?.id
            }
            if selectedId != nil { await capture() }
        }
        .onChange(of: selectedId) { _, _ in
            reply = nil
            capturedAt = nil
            errorMessage = nil
        }
        .seHaptic(.selection, trigger: copied)
    }

    // MARK: Data

    private var terminal: LiveTerminal? {
        session.terminals.terminals.first(where: { $0.id == selectedId })
    }

    /// The single gate: a non-nil appliedSeq and a ready observation. `text` is
    /// already nil upstream unless it also correlates with that sequence.
    private var gatedText: String? {
        guard let reply, reply.appliedSeq != nil, reply.status.isReady else { return nil }
        return reply.text
    }

    // MARK: Body

    @ViewBuilder
    private var content: some View {
        intro

        if session.terminals.terminals.isEmpty {
            NocturneEmptyState(
                systemImage: "terminal",
                title: String(localized: "No terminals"),
                message: String(localized: "Open a terminal in this workspace first.")
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
            .padding(.top, 18)
        } else {
            terminalPicker
            captureButton

            if loading {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Reading up to 80 lines…")
                        .font(SeNocturneType.font(11, weight: .regular))
                        .foregroundStyle(palette.inkSecondary)
                }
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 14)
                .frame(minHeight: NocturneToolMetrics.minimumTarget)
            }

            if let errorMessage {
                NocturneNotice(message: errorMessage, isError: true)
                    .padding(.horizontal, NocturneToolMetrics.gutter)
                    .padding(.top, 14)
            }

            if let reply {
                statusCard(reply)
                textSection(reply)
            } else if !loading && errorMessage == nil {
                NocturneEmptyState(
                    systemImage: "text.viewfinder",
                    title: String(localized: "Ready to capture"),
                    icon: .textview
                )
                .padding(.horizontal, NocturneToolMetrics.gutter)
                .padding(.top, 18)
            }
        }
    }

    private var intro: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                SeIconView(icon: .server, size: 13, stroke: 1.6, color: palette.inkTertiary)
                Text(verbatim: session.origin.host() ?? String(localized: "Connected host"))
                    .font(SeNocturneType.font(11, weight: .medium))
                    .foregroundStyle(palette.inkSecondary)
                SeInfoTip(text: String(localized: "A read-only text snapshot from the host. This is not live screen sharing."), size: 24)
                Spacer(minLength: 0)
            }
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 16)
    }

    private var terminalPicker: some View {
        Menu {
            ForEach(session.terminals.terminals) { terminal in
                Button {
                    selectedId = terminal.id
                } label: {
                    Text(verbatim: terminal.title.isEmpty ? terminal.id : terminal.title)
                }
            }
        } label: {
            HStack(spacing: 10) {
                SeIconView(icon: .terminal, size: 14, stroke: 1.6, color: palette.inkTertiary)
                Text(verbatim: terminal.map { $0.title.isEmpty ? $0.id : $0.title }
                     ?? String(localized: "Choose a terminal"))
                    .font(SeNocturneType.font(12, weight: .medium))
                    .foregroundStyle(palette.ink)
                    .lineLimit(1)
                Spacer(minLength: 8)
                SeIconView(icon: .chevronUpDown, size: 11, stroke: 1.7, color: palette.inkTertiary)
            }
            .padding(.horizontal, 12)
            .frame(maxWidth: .infinity, minHeight: NocturneToolMetrics.minimumTarget, alignment: .leading)
            .background(palette.card, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius))
            .overlay {
                RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius)
                    .strokeBorder(palette.stroke, lineWidth: 1)
            }
            .contentShape(Rectangle())
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 16)
        .disabled(loading)
    }

    private var captureButton: some View {
        NocturneRunButton(title: String(localized: "Refresh snapshot"), systemImage: "arrow.clockwise") {
            Task { await capture() }
        }
        .disabled(loading || selectedId == nil)
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 10)
    }

    private func statusCard(_ reply: HostScreenReadReply) -> some View {
        NocturneCard {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    NocturnePill(text: reply.status.state.rawValue.capitalized,
                                 tone: ScreenTextPresentation.semanticStatus(reply.status.state) == .completed ? .live
                                     : (ScreenTextPresentation.semanticStatus(reply.status.state) == .working ? .blue : .wait))
                    Spacer(minLength: 8)
                    if let appliedSeq = reply.appliedSeq {
                        Text(verbatim: "seq \(appliedSeq)")
                            .font(SeNocturneType.font(9, weight: .regular, mono: true))
                            .foregroundStyle(palette.inkTertiary)
                    }
                }
                Text(verbatim: ScreenTextPresentation.explanation(reply.status.state))
                    .font(SeNocturneType.font(11, weight: .regular))
                    .foregroundStyle(palette.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let reason = reply.status.reason {
                    Text(verbatim: reason.rawValue)
                        .font(SeNocturneType.font(9.5, weight: .regular, mono: true))
                        .foregroundStyle(palette.inkTertiary)
                }
                if let capturedAt {
                    Text("Captured \(capturedAt, format: .dateTime.hour().minute().second())")
                        .font(SeNocturneType.font(9.5, weight: .regular))
                        .foregroundStyle(palette.inkTertiary)
                }
                if reply.truncated {
                    Text("The host truncated this snapshot to the last 80 lines.")
                        .font(SeNocturneType.font(10, weight: .regular))
                        .foregroundStyle(palette.warning)
                }
            }
        }
        .padding(.horizontal, NocturneToolMetrics.gutter)
        .padding(.top, 16)
    }

    @ViewBuilder
    private func textSection(_ reply: HostScreenReadReply) -> some View {
        NocturneGroupLabel(
            title: String(localized: "Snapshot"),
            detail: gatedText == nil ? String(localized: "Hidden") : String(localized: "Read only")
        )

        if let text = gatedText {
            if text.isEmpty {
                NocturneEmptyState(
                    systemImage: "terminal",
                    title: String(localized: "The terminal screen is empty")
                )
                .padding(.horizontal, NocturneToolMetrics.gutter)
            } else {
                NocturneTerminalBlock(maxHeight: 320) {
                    Text(verbatim: text)
                        .foregroundStyle(palette.terminalInk)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.horizontal, 14)

                NocturneRunButton(title: String(localized: "Copy snapshot"), systemImage: "doc.on.doc") {
                    UIPasteboard.general.string = text
                    copied += 1
                }
                .padding(.horizontal, 14)
                .padding(.top, 10)
            }
        } else {
            NocturneEmptyState(
                systemImage: "text.viewfinder",
                title: String(localized: "Screen text is not available"),
                message: String(localized: "Refresh after the terminal settles."),
                icon: .textview
            )
            .padding(.horizontal, NocturneToolMetrics.gutter)
        }
    }

    // MARK: Capture

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
