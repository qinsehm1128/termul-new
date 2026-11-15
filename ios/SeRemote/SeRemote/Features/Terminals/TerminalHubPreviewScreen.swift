#if DEBUG
import SwiftUI

/// DEBUG-only visual fixture for the terminal hub, so the layout can be
/// checked on a simulator without a paired host.
///
/// Launch with `-SeHubPreview`; optional `-SeHubQuery <text>`,
/// `-SeHubStatus all|running|agent|idle`, `-SeHubGroup <group id>` and
/// `-SeHubSwitcher` (renders the in-terminal switcher with a selected shell).
struct TerminalHubPreviewScreen: View {
    @State private var query: TerminalHubIndex.Query
    private let switcher: Bool

    @Environment(\.sePalette) private var palette

    init(arguments: [String] = ProcessInfo.processInfo.arguments) {
        func value(_ flag: String) -> String? {
            guard let i = arguments.firstIndex(of: flag), arguments.indices.contains(i + 1) else { return nil }
            return arguments[i + 1]
        }
        _query = State(initialValue: TerminalHubIndex.Query(
            text: value("-SeHubQuery") ?? "",
            status: value("-SeHubStatus").flatMap(TerminalHubIndex.StatusFilter.init(rawValue:)) ?? .all,
            groupID: value("-SeHubGroup")
        ))
        switcher = arguments.contains("-SeHubSwitcher")
    }

    static var isRequested: Bool { ProcessInfo.processInfo.arguments.contains("-SeHubPreview") }

    var body: some View {
        TerminalHubView(
            index: Self.index(query: query),
            query: $query,
            selectedTerminalID: switcher ? "t-dev" : nil,
            actions: TerminalHubActions(
                openShell: { _ in }, openQuick: { _ in }, newTerminal: {}, newTerminalInProject: { _ in },
                renameQuick: { _ in }, closeQuick: { _ in }, deleteQuick: { _ in }, closeShell: { _ in },
                refresh: {}
            )
        ) {
            if switcher {
                Text(verbatim: hubText("Switch terminal"))
                    .font(SeNocturneType.font(17, weight: .semibold))
                    .foregroundStyle(palette.ink)
                    .padding(.top, 16)
            } else {
                HStack(alignment: .firstTextBaseline) {
                    Text(verbatim: hubText("Terminals"))
                        .font(SeNocturneType.font(28, weight: .semibold))
                        .foregroundStyle(palette.ink)
                    Spacer()
                    HStack(spacing: 6) {
                        Circle().fill(palette.live).frame(width: 8, height: 8)
                        Text(verbatim: "9")
                            .font(SeNocturneType.font(13, mono: true))
                            .foregroundStyle(palette.inkSecondary)
                    }
                }
                .padding(.top, 16)
            }
        }
    }

    static func index(query: TerminalHubIndex.Query) -> TerminalHubIndex {
        func summary(_ id: String, _ project: String?, _ title: String, _ cwd: String, _ branch: String? = nil) -> LiveTerminalSummary {
            LiveTerminalSummary(
                id: id, shell: "/bin/zsh", cwd: cwd, pid: 1, cols: 80, rows: 24,
                terminalScreenEngine: nil, conversationId: nil, projectId: project,
                title: title, gitBranch: branch, screenObservation: nil
            )
        }
        func decode<T: Decodable>(_ json: String) -> T {
            // swiftlint:disable:next force_try
            try! JSONDecoder().decode(T.self, from: Data(json.utf8))
        }
        let projects: [HostProject] = decode("""
        [{"id":"termul","name":"termul","path":"/Users/qs/project/me/termul","liveTerminalCount":3},
         {"id":"portal","name":"dslpayment-portal","path":"/Users/qs/project/dsl/dslpayment-portal","liveTerminalCount":2},
         {"id":"ns","name":"ns-payment-control-client","path":"/Users/qs/project/ns/ns-payment-control-client","liveTerminalCount":0},
         {"id":"qin","name":"qin-code","path":"/Users/qs/project/qs/qin-code","liveTerminalCount":1},
         {"id":"deploy","name":"se-server-deploy","path":"/Users/qs/project/ops/se-server-deploy","liveTerminalCount":2},
         {"id":"wechat","name":"wechat-parser-mcp","liveTerminalCount":0}]
        """)
        let groups: [HostProjectGroup] = decode("""
        [{"id":"g-se","name":"Se","projectIds":["termul","deploy","qin"]},
         {"id":"g-pay","name":"支付","projectIds":["portal","ns"]}]
        """)
        let records: [QuickTerminalRecordWire] = decode("""
        [{"schemaVersion":1,"id":"q-dev","title":"Se dev","target":{"kind":"project_root","projectId":"termul","projectRoot":"/Users/qs/project/me/termul"},
          "cwd":"/Users/qs/project/me/termul","createdAtUtc":"x","updatedAtUtc":"x","terminalIds":["t-dev"],"origin":"created"},
         {"schemaVersion":1,"id":"q-portal","title":"portal dev","target":{"kind":"project_root","projectId":"portal","projectRoot":"/p"},
          "cwd":"/Users/qs/project/dsl/dslpayment-portal","createdAtUtc":"x","updatedAtUtc":"x","terminalIds":["t-portal"],"origin":"created"},
         {"schemaVersion":1,"id":"q-logs","title":"ns logs","target":{"kind":"project_root","projectId":"ns","projectRoot":"/p"},
          "cwd":"/Users/qs/project/ns/ns-payment-control-client","createdAtUtc":"x","updatedAtUtc":"x","terminalIds":[],"origin":"created"},
         {"schemaVersion":1,"id":"q-dmit","title":"dmit","target":{"kind":"project_root","projectId":"deploy","projectRoot":"/p"},
          "cwd":"/Users/qs/project/ops/se-server-deploy","createdAtUtc":"x","updatedAtUtc":"x","terminalIds":["t-dmit-1","t-dmit-2"],"origin":"created"}]
        """)
        return TerminalHubIndex.make(
            summaries: [
                summary("t-dev", "termul", "pnpm tauri dev", "/Users/qs/project/me/termul", "main"),
                summary("t-claude", "termul", "claude", "/Users/qs/project/me/termul", "main"),
                summary("t-zsh", "termul", "zsh", "/Users/qs/project/me/termul/ios/SeRemote", "main"),
                summary("t-codex", "portal", "codex", "/Users/qs/project/dsl/dslpayment-portal", "feat/login-split"),
                summary("t-portal", "portal", "pnpm dev", "/Users/qs/project/dsl/dslpayment-portal", "feat/login-split"),
                summary("t-cargo", "qin", "cargo test -p qin-core", "/Users/qs/project/qs/qin-code", "main"),
                summary("t-dmit-1", "deploy", "ssh dmit", "/Users/qs/project/ops/se-server-deploy"),
                summary("t-dmit-2", "deploy", "ssh dmit", "/Users/qs/project/ops/se-server-deploy"),
                summary("t-home", nil, "zsh", "/Users/qs"),
            ],
            projects: projects,
            projectGroups: groups,
            quickRecords: records,
            agentTerminalIDs: ["t-claude", "t-codex"],
            allowsCreate: true,
            query: query
        )
    }
}
#endif
