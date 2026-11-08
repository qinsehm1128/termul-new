import Foundation
import Observation

@MainActor
@Observable
final class FileStore {
    var crumbs: [DirectoryEntry] = []
    var entries: [DirectoryEntry] = []
    var preview: FileContent?
    var previewName: String?
    var isLoading = false
    var errorMessage: String?

    private var previewPath: String?
    private var http: HostHTTP?

    func attach(http: HostHTTP) {
        self.http = http
    }

    func openRoot(_ path: String) async {
        crumbs = [DirectoryEntry(name: rootName(path), path: path, type: "directory")]
        preview = nil
        previewName = nil
        previewPath = nil
        await load(path)
    }

    func open(_ entry: DirectoryEntry) async {
        _ = await openReturningSuccess(entry)
    }

    /// Same as `open`, but reports whether this specific operation succeeded,
    /// so callers never infer the outcome from shared, older error state.
    @discardableResult
    func openReturningSuccess(_ entry: DirectoryEntry) async -> Bool {
        if entry.isDirectory {
            crumbs.append(entry)
            preview = nil
            previewName = nil
            previewPath = nil
            return await load(entry.path)
        } else {
            return await read(entry)
        }
    }

    func popTo(_ entry: DirectoryEntry) async {
        guard let index = crumbs.firstIndex(of: entry) else { return }
        crumbs = Array(crumbs.prefix(through: index))
        preview = nil
        previewName = nil
        previewPath = nil
        await load(entry.path)
    }

    /// Pull-to-refresh entry point: reload the visible directory, or reopen
    /// the file preview when one is showing. Failures land in `errorMessage`.
    func refresh() async {
        if let path = previewPath, let name = previewName {
            await read(name: name, path: path)
        } else if let current = crumbs.last {
            await load(current.path)
        }
    }

    @discardableResult
    private func load(_ path: String) async -> Bool {
        guard let http else { return false }
        isLoading = true
        defer { isLoading = false }
        do {
            let list: [DirectoryEntry] = try await http.get("fs/browse", query: ["path": path])
            entries = list.sorted { lhs, rhs in
                if lhs.isDirectory != rhs.isDirectory { return lhs.isDirectory }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
            // A successful load supersedes an earlier failure.
            errorMessage = nil
            return true
        } catch {
            HostLog.session.error("File list load failed")
            errorMessage = error.localizedDescription
            entries = []
            return false
        }
    }

    @discardableResult
    private func read(_ entry: DirectoryEntry) async -> Bool {
        await read(name: entry.name, path: entry.path)
    }

    @discardableResult
    private func read(name: String, path: String) async -> Bool {
        guard let http else { return false }
        isLoading = true
        defer { isLoading = false }
        do {
            preview = try await http.get("fs/read", query: ["path": path])
            previewName = name
            previewPath = path
            errorMessage = nil
            return true
        } catch {
            HostLog.session.error("File preview load failed")
            errorMessage = error.localizedDescription
            preview = nil
            return false
        }
    }

    private func rootName(_ path: String) -> String {
        URL(fileURLWithPath: path).lastPathComponent
    }
}
