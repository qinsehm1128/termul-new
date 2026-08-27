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

    private var http: HostHTTP?

    func attach(http: HostHTTP) {
        self.http = http
    }

    func openRoot(_ path: String) async {
        crumbs = [DirectoryEntry(name: rootName(path), path: path, type: "directory")]
        preview = nil
        previewName = nil
        await load(path)
    }

    func open(_ entry: DirectoryEntry) async {
        if entry.isDirectory {
            crumbs.append(entry)
            preview = nil
            previewName = nil
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
        await load(entry.path)
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
        guard let http else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            preview = try await http.get("fs/read", query: ["path": entry.path])
            previewName = entry.name
        } catch {
            errorMessage = error.localizedDescription
            preview = nil
        }
    }

    private func rootName(_ path: String) -> String {
        URL(fileURLWithPath: path).lastPathComponent
    }
}
