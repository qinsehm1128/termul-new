import Foundation
import Observation

enum SearchLoadState: Sendable, Hashable {
    case loading, loaded, empty
    case failed(String)
}

@Observable
@MainActor
final class SearchStore {
    private(set) var state: SearchLoadState = .empty
    private(set) var response: FileSearchResponse?
    private(set) var query = ""
    private(set) var scopeRoot = ""
    private(set) var rootPath = ""
    var results: [FileSearchResult] { response?.results ?? [] }
    var isLoading: Bool { state == .loading }
    var errorMessage: String? {
        if case let .failed(message) = state { return message }
        return nil
    }

    private var client: SearchClient?
    private var pending: Task<Void, Never>?
    private var generation = UUID()

    func attach(http: HostHTTP) {
        cancelSearch()
        client = SearchClient(http: http)
        response = nil
        state = .empty
    }

    func configure(scopeRoot: String, rootPath: String) {
        cancelSearch()
        self.scopeRoot = scopeRoot
        self.rootPath = rootPath
        response = nil
        state = .empty
    }

    /// Cancels both the debounce and the URLSession request through Swift Task
    /// cancellation. A generation guard also rejects late host responses.
    func setQuery(_ query: String, debounce: Duration = .milliseconds(300)) {
        cancelSearch()
        self.query = query
        response = nil
        guard validateQuery() else { return }
        let generation = generation
        state = .loading
        pending = Task { [weak self] in
            do {
                try await Task.sleep(for: debounce)
                guard let self else { return }
                await self.load(generation: generation)
            } catch {
                // Cancellation is not a user-visible search failure.
            }
        }
    }

    func refresh() async {
        cancelSearch()
        guard validateQuery() else { return }
        state = .loading
        let generation = generation
        let task = Task { [weak self] in
            guard let self else { return }
            await self.load(generation: generation)
        }
        pending = task
        await withTaskCancellationHandler {
            await task.value
        } onCancel: {
            task.cancel()
        }
        if generation == self.generation { pending = nil }
    }

    func cancelSearch() {
        pending?.cancel()
        pending = nil
        generation = UUID()
        if state == .loading { state = results.isEmpty ? .empty : .loaded }
    }

    private func validateQuery() -> Bool {
        do {
            try SearchQuery.validate(query)
        } catch {
            response = nil
            state = .failed(error.localizedDescription)
            return false
        }
        guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            response = nil
            state = .empty
            return false
        }
        guard client != nil, !scopeRoot.isEmpty, !rootPath.isEmpty else {
            state = .failed(String(localized: "Choose a connected host and search folder first."))
            return false
        }
        return true
    }

    private func load(generation: UUID) async {
        guard let client, generation == self.generation else { return }
        let body = SearchContentBody(scopeRoot: scopeRoot, rootPath: rootPath, query: query)
        do {
            let response = try await client.content(body)
            guard !Task.isCancelled, generation == self.generation else { return }
            self.response = response
            state = response.results.isEmpty ? .empty : .loaded
        } catch {
            guard generation == self.generation else { return }
            if Task.isCancelled {
                state = results.isEmpty ? .empty : .loaded
                return
            }
            HostLog.session.error("Search refresh failed")
            response = nil
            state = .failed(error.localizedDescription)
        }
    }
}
