import SwiftUI

/// 屏幕 — the two ways to look at the host screen without leaving 工具:
/// 快照 is the read-only text observation, 桌面 is the live remote desktop.
///
/// Both modes share one system navigation bar (the desktop feature owns its own
/// toolbar visibility, so a second custom bar would stack above it). The bar is
/// styled with the Nocturne title, mono subtitle and round back control; the
/// segmented control below it switches modes.
enum ScreenSegment: String, CaseIterable, Identifiable {
    case snapshot
    case desktop

    var id: String { rawValue }

    var title: String {
        switch self {
        case .snapshot: String(localized: "Snapshot")
        case .desktop: String(localized: "Desktop")
        }
    }
}

struct ScreenFeatureView: View {
    @Environment(\.sePalette) private var palette
    @Environment(\.dismiss) private var dismiss

    let session: WorkspaceSession

    @State private var segment: ScreenSegment = .snapshot
    /// Shared with the 快照 segment so the page subtitle keeps the terminal name
    /// and switching to 桌面 and back does not lose the selection.
    @State private var selectedTerminalId: String?

    var body: some View {
        VStack(spacing: 0) {
            NocturneSegmented(
                segments: ScreenSegment.allCases.map { NocturneSegment(value: $0, title: $0.title) },
                selection: $segment
            )

            switch segment {
            case .snapshot:
                ScreenTextFeatureView(session: session, selectedId: $selectedTerminalId)
            case .desktop:
                DesktopFeatureView(session: session)
            }
        }
        .background(palette.canvas.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .navigationBarBackButtonHidden(true)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.visible, for: .navigationBar)
        .toolbarBackground(palette.canvas, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { dismiss() } label: {
                    SeIconView(icon: .back, size: 15, stroke: 1.7, color: palette.inkSecondary)
                        .frame(width: 32, height: 32)
                        .background(palette.card, in: RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius))
                        .overlay {
                            RoundedRectangle(cornerRadius: NocturneToolMetrics.controlRadius)
                                .strokeBorder(palette.stroke, lineWidth: 1)
                        }
                        .frame(minWidth: NocturneToolMetrics.minimumTarget,
                               minHeight: NocturneToolMetrics.minimumTarget)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(String(localized: "Back"))
            }

            ToolbarItem(placement: .principal) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Screen")
                        .font(SeNocturneType.font(14, weight: .semibold))
                        .foregroundStyle(palette.ink)
                        .lineLimit(1)
                    Text(verbatim: subtitle)
                        .font(SeNocturneType.monoSmall)
                        .foregroundStyle(palette.inkTertiary)
                        .lineLimit(1)
                }
            }
        }
    }

    /// Host, plus the selected terminal only while 快照 is showing. 桌面 has its
    /// own connection state, so it never borrows a terminal name.
    private var subtitle: String {
        let host = session.origin.host() ?? String(localized: "Host")
        guard segment == .snapshot,
              let terminal = session.terminals.terminals.first(where: { $0.id == selectedTerminalId })
        else { return host }
        let title = terminal.title.isEmpty ? terminal.id : terminal.title
        return "\(host) · \(title)"
    }
}
