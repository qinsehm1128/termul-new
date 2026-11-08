import Foundation

protocol SearchServing {
    func content(_ body: SearchContentBody) async throws -> FileSearchResponse
}

struct SearchClient: SearchServing {
    let http: HostHTTP

    /// Fixed-string, case-insensitive search. The host supplies all ignore rules.
    func content(_ body: SearchContentBody) async throws -> FileSearchResponse {
        try body.validate()
        try Task.checkCancellation()
        HostLog.session.debug("Content search request")
        do {
            let reply: FileSearchResponse = try await http.post("search/content", body: [
                "scopeRoot": body.scopeRoot, "rootPath": body.rootPath, "query": body.query,
            ])
            try Task.checkCancellation()
            return reply
        } catch {
            if !Task.isCancelled { HostLog.session.error("Content search failed") }
            throw error
        }
    }

    func rgInfo() async throws -> RgInfo {
        HostLog.session.debug("Search engine info request")
        do {
            return try await http.get("search/rg-info")
        } catch {
            HostLog.session.error("Search engine info failed")
            throw error
        }
    }
}
