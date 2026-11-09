import Foundation
import Observation

/// One agent process the host proved is running under a live terminal.
///
/// The host derives this from its process table (`GET /cli-sessions/live`), so
/// a row here is evidence of a live process, never a session file on disk.
struct HostLiveAgentSession: Identifiable, Hashable, Decodable, Sendable {
    var terminalId: String
    var projectId: String?
    var rootPid: Int
    var agentId: String
    var sessionId: String
    var selfDev: Bool?
    var codexHome: String?
    var cwd: String?

    var id: String { "\(terminalId):\(agentId):\(sessionId)" }

    /// Project attribution when the host reported one. The `Live` destination
    /// opens that project's running terminals.
    var destinationProjectId: String? {
        guard let projectId, !projectId.isEmpty else { return nil }
        return projectId
    }
}

struct HostLiveAgentSessionList: Decodable, Sendable {
    var sessions: [HostLiveAgentSession]
    var scannedAt: String?

    enum CodingKeys: String, CodingKey {
        case sessions, scannedAt
    }

    init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: CodingKeys.self)
        sessions = try fields.decodeIfPresent([HostLiveAgentSession].self, forKey: .sessions) ?? []
        scannedAt = try fields.decodeIfPresent(String.self, forKey: .scannedAt)
    }
}

/// Keeps the host's live agent detection in step with the phone.
///
/// A failed refresh clears the snapshot: "running" is a claim about now, and a
/// stale snapshot must not keep a stopped agent on screen.
@MainActor
@Observable
final class LiveAgentSessionStore {
    var sessions: [HostLiveAgentSession] = []
    var errorMessage: String?

    private var http: HostHTTP?

    func attach(http: HostHTTP) {
        self.http = http
    }

    func refresh() async {
        guard let http else { return }
        do {
            let payload: HostLiveAgentSessionList = try await http.get("cli-sessions/live")
            sessions = payload.sessions
            errorMessage = nil
        } catch {
            sessions = []
            errorMessage = error.localizedDescription
            HostLog.session.error("Live agent session refresh failed")
        }
    }

    /// Agent sessions per project id, for the Workspace project rows.
    var countsByProject: [String: Int] {
        var counts: [String: Int] = [:]
        for session in sessions {
            guard let projectId = session.destinationProjectId else { continue }
            counts[projectId, default: 0] += 1
        }
        return counts
    }
}
