import Foundation
import Observation

@MainActor
@Observable
final class QuickTerminalStore {
    enum State: Equatable {
        case idle
        case loading
        case loaded
        case empty
        case failed(String)
    }

    private(set) var records: [QuickTerminalRecordWire] = []
    private(set) var state: State = .idle
    var isLoading: Bool { state == .loading }
    var errorMessage: String? {
        if case let .failed(message) = state { return message }
        return nil
    }

    @ObservationIgnored private var client: (any QuickTerminalServing)?
    @ObservationIgnored private var attachment = UUID()
    @ObservationIgnored private var refreshID = UUID()

    init() {}

    /// Injection seam for deterministic state tests, without changing HostHTTP.
    init(client: any QuickTerminalServing) {
        self.client = client
    }

    func attach(http: HostHTTP) {
        attachment = UUID()
        refreshID = UUID()
        client = QuickTerminalClient(http: http)
        records = []
        state = .idle
        HostLog.session.info("Quick terminals attached")
    }

    func refresh() async {
        guard let client else { return }
        let currentAttachment = attachment
        let requestID = UUID()
        refreshID = requestID
        state = .loading
        HostLog.session.info("Quick terminals refresh started")
        do {
            let result = try await client.list()
            guard currentAttachment == attachment, requestID == refreshID else { return }
            records = result
            settle()
            HostLog.session.info("Quick terminals refresh completed")
        } catch {
            guard currentAttachment == attachment, requestID == refreshID else { return }
            state = .failed(error.localizedDescription)
            HostLog.session.error("Quick terminals refresh failed")
        }
    }

    @discardableResult
    func create(target: QuickTerminalTargetWire, title: String? = nil) async throws -> QuickTerminalRecordWire {
        try await update { try await $0.create(.init(target: target, title: title)) }
    }

    /// Returns the unmodified claim for TerminalSocket.attach(terminalId:claim:...).
    /// No PTY is spawned or attached by the store itself.
    func open(id: String, cols: UInt16, rows: UInt16, terminalId: String? = nil) async throws -> QuickTerminalOpenedWire {
        try await openResult { try await $0.open(.init(id: id, cols: cols, rows: rows, terminalId: terminalId)) }
    }

    func createTerminal(id: String, cols: UInt16, rows: UInt16) async throws -> QuickTerminalOpenedWire {
        try await openResult { try await $0.createTerminal(.init(id: id, cols: cols, rows: rows)) }
    }

    @discardableResult
    func closeTerminal(id: String, terminalId: String) async throws -> QuickTerminalRecordWire {
        try await update { try await $0.closeTerminal(.init(id: id, terminalId: terminalId)) }
    }

    @discardableResult
    func rename(id: String, title: String?) async throws -> QuickTerminalRecordWire {
        try await update { try await $0.rename(.init(id: id, title: title)) }
    }

    @discardableResult
    func close(id: String) async throws -> QuickTerminalRecordWire {
        try await update { try await $0.close(.init(id: id)) }
    }

    func delete(id: String) async throws {
        _ = try await perform { client in
            try await client.delete(.init(id: id))
            return id
        } apply: { id in
            records.removeAll { $0.id == id }
        }
    }

    private func update(_ operation: @MainActor (any QuickTerminalServing) async throws -> QuickTerminalRecordWire) async throws -> QuickTerminalRecordWire {
        try await perform(operation) { upsert($0) }
    }

    private func openResult(_ operation: @MainActor (any QuickTerminalServing) async throws -> QuickTerminalOpenedWire) async throws -> QuickTerminalOpenedWire {
        try await perform(operation) { upsert($0.record) }
    }

    private func perform<T>(_ operation: @MainActor (any QuickTerminalServing) async throws -> T, apply: (T) -> Void) async throws -> T {
        guard let client else {
            throw HostError.unexpected("Quick terminals are not attached to a host.")
        }
        let currentAttachment = attachment
        HostLog.session.info("Quick terminal operation started")
        do {
            let result = try await operation(client)
            // Never hand an old host's PTY claim to a newly attached host.
            guard currentAttachment == attachment else { throw CancellationError() }
            // Invalidate in-flight list results that predate this mutation.
            refreshID = UUID()
            apply(result)
            settle()
            HostLog.session.info("Quick terminal operation completed")
            return result
        } catch {
            if currentAttachment == attachment {
                refreshID = UUID()
                state = .failed(error.localizedDescription)
            }
            HostLog.session.error("Quick terminal operation failed")
            throw error
        }
    }

    private func upsert(_ record: QuickTerminalRecordWire) {
        if let index = records.firstIndex(where: { $0.id == record.id }) {
            records[index] = record
        } else {
            records.insert(record, at: 0)
        }
    }

    private func settle() {
        state = records.isEmpty ? .empty : .loaded
    }
}
