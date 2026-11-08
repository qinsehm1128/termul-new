import Foundation

@MainActor
protocol GitReading {
    func status(cwd: String) async throws -> [GitStatusDetailWire]
    func diff(cwd: String, path: String, staged: Bool) async throws -> String
    func log(cwd: String, limit: Int?) async throws -> [GitCommitWire]
    func commitContext(cwd: String) async throws -> GitCommitContextWire
    func branches(cwd: String) async throws -> [GitBranch]
    func stashes(cwd: String) async throws -> [GitStashInfoWire]
}

/// Intentionally exposes only the host's six read routes. HostHTTP owns auth,
/// envelope decoding, timeouts and transport failures.
@MainActor
struct GitClient: GitReading {
    let http: HostHTTP

    func status(cwd: String) async throws -> [GitStatusDetailWire] {
        try await http.post("git/status", body: ["cwd": cwd])
    }

    func diff(cwd: String, path: String, staged: Bool = false) async throws -> String {
        // Rust get_diff returns IpcBody<String>, not an object with a diff field.
        try await http.post("git/diff", body: ["cwd": cwd, "path": path, "staged": staged])
    }

    func log(cwd: String, limit: Int? = nil) async throws -> [GitCommitWire] {
        var body: [String: Any] = ["cwd": cwd]
        if let limit { body["limit"] = limit }
        return try await http.post("git/log", body: body)
    }

    func commitContext(cwd: String) async throws -> GitCommitContextWire {
        try await http.post("git/commit-context", body: ["cwd": cwd])
    }

    func branches(cwd: String) async throws -> [GitBranch] {
        try await http.get("git/branch-list", query: ["cwd": cwd])
    }

    func stashes(cwd: String) async throws -> [GitStashInfoWire] {
        try await http.get("git/stash-list", query: ["cwd": cwd])
    }
}
