import Foundation

struct GitStatusDetailWire: Decodable, Sendable, Hashable {
    let path: String
    /// Raw porcelain status; unknown codes remain displayable.
    let status: String
    let staged: Bool
}

struct GitCommitWire: Decodable, Sendable, Hashable {
    let hash: String
    let shortHash: String
    let parents: [String]
    let refs: [String]
    let author: String
    let date: String
    let subject: String
}

struct GitStashInfoWire: Decodable, Sendable, Hashable {
    let index: Int
    let name: String
    let message: String
}

struct GitCommitContextWire: Decodable, Sendable, Hashable {
    let branch: String?
    let hasUpstream: Bool
    let ahead: Int
    let behind: Int
    let stagedCount: Int
    let hasHead: Bool
    let lastSubject: String
    let lastBody: String
}

/// The host returns raw `git branch -a` lines, not branch DTOs.
struct GitBranch: Decodable, Sendable, Hashable {
    let raw: String
    let name: String
    let isCurrent: Bool
    let isRemote: Bool
    let isCheckedOutElsewhere: Bool
    let symbolicTarget: String?

    init(raw: String) {
        self.raw = raw
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        isCurrent = trimmed.hasPrefix("* ")
        isCheckedOutElsewhere = trimmed.hasPrefix("+ ")
        let unmarked = (isCurrent || isCheckedOutElsewhere)
            ? String(trimmed.dropFirst(2)) : trimmed
        let parts = unmarked.components(separatedBy: " -> ")
        name = parts[0]
        symbolicTarget = parts.count > 1 ? parts.dropFirst().joined(separator: " -> ") : nil
        isRemote = name.hasPrefix("remotes/")
    }

    init(from decoder: any Decoder) throws {
        self.init(raw: try decoder.singleValueContainer().decode(String.self))
    }
}
