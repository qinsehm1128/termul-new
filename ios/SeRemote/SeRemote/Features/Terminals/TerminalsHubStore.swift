import Foundation
import Observation

/// Loads the live host terminals for the 终端 hub.
///
/// One unfiltered `list` over the session's existing `TerminalSocket` is the
/// primary source. Old hosts answer VALIDATION_ERROR for that request; the
/// store then lists once per project reporting live terminals. Nothing here
/// invents a terminal: an empty host response is an empty hub.
@MainActor
@Observable
final class TerminalsHubStore {
    enum State: Equatable {
        case idle
        case loading
        case loaded
        case empty
        case failed(String)
    }

    private(set) var summaries: [LiveTerminalSummary] = []
    private(set) var state: State = .idle
    /// True only when the host rejected the unfiltered list and the store
    /// listed project by project instead.
    private(set) var usedProjectFallback = false

    var errorMessage: String? {
        if case let .failed(message) = state { return message }
        return nil
    }

    /// The host-reported row for one terminal. Used to resolve a deep link or
    /// the last-open terminal; `nil` means the host no longer reports it.
    func summary(for terminalID: String) -> LiveTerminalSummary? {
        summaries.first(where: { $0.id == terminalID })
    }

    @ObservationIgnored private var socket: TerminalSocket?
    @ObservationIgnored private var ensureConnection: (@MainActor () async throws -> Void)?
    @ObservationIgnored private var requestID = UUID()

    func attach(
        socket: TerminalSocket,
        ensureConnection: @escaping @MainActor () async throws -> Void
    ) {
        self.socket = socket
        self.ensureConnection = ensureConnection
        requestID = UUID()
        summaries = []
        state = .idle
        usedProjectFallback = false
    }

    func refresh(projects: [HostProject]) async {
        guard let socket else { return }
        let currentRequest = UUID()
        requestID = currentRequest
        if summaries.isEmpty {
            state = .loading
        }
        do {
            try await ensureConnection?()
            let listed: [LiveTerminalSummary]
            var fallback = false
            do {
                listed = try await socket.list(conversationId: nil, projectId: nil)
            } catch {
                guard TerminalsHubPresentation.shouldFallBackToListPerProject(error) else {
                    throw error
                }
                fallback = true
                var pages: [[LiveTerminalSummary]] = []
                for projectId in TerminalsHubPresentation.fallbackProjectIds(projects) {
                    pages.append(try await socket.list(conversationId: nil, projectId: projectId))
                }
                listed = TerminalsHubPresentation.merged(pages)
            }
            // A newer refresh (15 s tick, foreground, pull) owns the state.
            guard currentRequest == requestID else { return }
            summaries = listed
            usedProjectFallback = fallback
            state = summaries.isEmpty ? .empty : .loaded
        } catch is CancellationError {
            return
        } catch {
            guard currentRequest == requestID else { return }
            summaries = []
            state = .failed(error.localizedDescription)
            HostLog.session.error("Terminal hub list failed")
        }
    }
}
