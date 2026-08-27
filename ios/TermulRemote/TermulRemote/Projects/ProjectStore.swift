import Foundation
import Observation

@MainActor
@Observable
final class ProjectStore {
    var projects: [HostProject] = []
    var active: HostProject?
    var errorMessage: String?

    private var http: HostHTTP?
    private weak var socket: AcpSocket?

    func attach(http: HostHTTP, socket: AcpSocket) {
        self.http = http
        self.socket = socket
    }

    func refresh() async {
        guard let http else { return }
        do {
            let payload: ProjectListPayload = try await http.get("projects")
            projects = payload.projects
            if let current = active {
                active = projects.first(where: { $0.id == current.id })
            }
        } catch {
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
