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
        if entry.isDirectory {
            crumbs.append(entry)
            preview = nil
            previewName = nil
            previewPath = nil
            await load(entry.path)
        } else {
            await read(entry)
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

    private func load(_ path: String) async {
        guard let http else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            let list: [DirectoryEntry] = try await http.get("fs/browse", query: ["path": path])
            entries = list.sorted { lhs, rhs in
                if lhs.isDirectory != rhs.isDirectory { return lhs.isDirectory }
                return lhs.name.localizedCaseInsensitiveCompare(rhs.name) == .orderedAscending
            }
        } catch {
            errorMessage = error.localizedDescription
            entries = []
        }
    }

    private func read(_ entry: DirectoryEntry) async {
        await read(name: entry.name, path: entry.path)
    }

    private func read(name: String, path: String) async {
        guard let http else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            preview = try await http.get("fs/read", query: ["path": path])
            previewName = name
            previewPath = path
        } catch {
            errorMessage = error.localizedDescription
            preview = nil
        }
    }

    private func rootName(_ path: String) -> String {
        URL(fileURLWithPath: path).lastPathComponent
    }
}
