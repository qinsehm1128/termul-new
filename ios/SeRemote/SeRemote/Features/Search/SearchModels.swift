import Foundation

struct SearchContentBody: Codable, Sendable, Hashable {
    let scopeRoot: String
    let rootPath: String
    let query: String

    func validate() throws {
        try SearchQuery.validate(query)
    }
}

/// Match the host's trimmed Rust `chars().count()`, not UTF-16 or grapheme count.
enum SearchQuery {
    static let maximumLength = 500

    static func validate(_ query: String) throws {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.unicodeScalars.count <= maximumLength else {
            throw SearchQueryError.tooLong
        }
    }
}

enum SearchQueryError: LocalizedError, Sendable, Hashable {
    case tooLong

    var errorDescription: String? {
        String(localized: "Search queries must be 500 characters or fewer.")
    }
}

struct FileSearchMatch: Decodable, Sendable, Hashable {
    let lineNumber: Int
    let lineText: String
}

struct FileSearchResult: Decodable, Sendable, Hashable {
    let filePath: String
    let matches: [FileSearchMatch]
}

struct FileSearchResponse: Decodable, Sendable, Hashable {
    let results: [FileSearchResult]
    let truncated: Bool
    let scannedFiles: Int
    let failedFiles: Int
}

struct RgInfo: Decodable, Sendable, Hashable {
    let sidecarBinaryName: String
    let resolvedPath: String
    let source: String
    let exists: Bool
}
