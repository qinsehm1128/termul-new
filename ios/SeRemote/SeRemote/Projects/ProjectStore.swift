import Foundation
import Observation

@MainActor
@Observable
final class ProjectStore {
    var projects: [HostProject] = []
    var groups: [HostProjectGroup] = []
    var active: HostProject?
    var isLoading = false
    var errorMessage: String?

    private var http: HostHTTP?
    private weak var socket: AcpSocket?
    /// Monotonic request guard: only the newest `refresh()` may publish.
    @ObservationIgnored private var refreshID = UUID()

    func attach(http: HostHTTP, socket: AcpSocket) {
        self.http = http
        self.socket = socket
        // A response from the previous host must never land in this session.
        refreshID = UUID()
    }

    func refresh() async {
        guard let http else { return }
        let requestID = UUID()
        refreshID = requestID
        isLoading = true
        defer { if requestID == refreshID { isLoading = false } }
        do {
            let payload: ProjectListPayload = try await http.get("projects")
            guard requestID == refreshID else { return }
            projects = payload.projects
            groups = payload.groups
            if let current = active {
                active = projects.first(where: { $0.id == current.id })
            }
        } catch {
            guard requestID == refreshID else { return }
            errorMessage = error.localizedDescription
        }
    }

    func clearSelection() {
        active = nil
    }

    func select(_ project: HostProject) async {
        active = project
        HostLog.session.info("Selected project workspace for terminal watch")
    }
}
