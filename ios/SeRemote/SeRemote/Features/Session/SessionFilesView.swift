import SwiftUI

/// Read-only browsing, navigation, refresh, copy and sharing stay owned by FileBrowserView.
struct SessionFilesView: View {
    @Bindable var session: WorkspaceSession

    var body: some View {
        FileBrowserView(session: session, embedded: true)
    }
}

/// Pure mappings of host metadata. Do not invent branch, age, size or preview text.
enum SessionFilesPresentation {
    nonisolated static func byteCount(_ size: Int?) -> String? {
        guard let size, size >= 0 else { return nil }
        return ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file)
    }

    nonisolated static func modificationDate(_ timestamp: Double?) -> Date? {
        guard let timestamp, timestamp.isFinite, timestamp > 0 else { return nil }
        return Date(timeIntervalSince1970: timestamp / 1_000)
    }

    nonisolated static func supportsTextPreview(encoding: String?) -> Bool {
        guard let encoding else { return true }
        return ["utf-8", "utf8"].contains(encoding.lowercased())
    }
}
