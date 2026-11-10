import Foundation

/// Pure presentation rules for the terminal file panel, the full-screen
/// preview cover and the iPad file inspector: titles, the preview subtitle,
/// what "Insert path" writes, which menu actions are available and when a new
/// root must be loaded. No view state and no host calls, so every rule is
/// unit tested.
nonisolated enum FilePanelPresentation {
    /// Menu rows on the preview cover. Availability always derives from the
    /// terminal's agent surface and from the content that actually loaded.
    struct MenuAvailability: Equatable, Sendable {
        let insertPath: Bool
        let sendToAgent: Bool
        let copy: Bool
        let share: Bool
        /// `nil` when Send to Agent is enabled; otherwise the explanation the
        /// cover shows next to the disabled row via `SeInfoTip`.
        let sendToAgentInfo: String?
    }

    // MARK: - Roots

    /// Title for a listing rooted at `rootPath`: its last component, with the
    /// filesystem root keeping its slash and an unknown root falling back to
    /// the generic panel name.
    static func title(rootPath: String) -> String {
        let root = normalizedRoot(rootPath)
        guard !root.isEmpty else { return String(localized: "Files") }
        guard root != "/" else { return "/" }
        return URL(fileURLWithPath: root).lastPathComponent
    }

    /// Host cwd paths should compare without a trailing slash: `/a/b/` and
    /// `/a/b` are the same folder.
    static func normalizedRoot(_ path: String) -> String {
        var root = path.trimmingCharacters(in: .whitespacesAndNewlines)
        while root.count > 1, root.hasSuffix("/") {
            root.removeLast()
        }
        return root
    }

    /// The panel reloads only when the terminal cwd really changed; opening the
    /// same folder again must keep the current listing, filters and selection.
    static func shouldReloadRoot(currentRoot: String?, requestedRoot: String) -> Bool {
        let requested = normalizedRoot(requestedRoot)
        guard !requested.isEmpty else { return false }
        guard let currentRoot else { return true }
        return normalizedRoot(currentRoot) != requested
    }

    // MARK: - Preview subtitle

    /// Line count for a previewed text file. Images and binary files have no
    /// meaningful line count; a still-loading text file has none yet.
    static func lineCount(content: FileContent?, kind: FilePreviewKind?) -> Int? {
        guard let content, let kind else { return nil }
        switch kind {
        case .markdown, .html, .text:
            return lineCount(in: content.content)
        case .image, .unsupportedImage, .unsupported:
            return nil
        }
    }

    /// Number of displayed lines: a trailing newline terminates the last line
    /// instead of starting an empty one (`"a\nb\n"` is two lines). Counting
    /// UTF-8 line-feed bytes keeps CRLF files correct, where Swift's character
    /// view folds `\r\n` into one grapheme.
    static func lineCount(in text: String) -> Int {
        guard !text.isEmpty else { return 0 }
        let newlines = text.utf8.reduce(0) { count, byte in
            byte == 0x0A ? count + 1 : count
        }
        // Compare UTF-8 bytes too: String suffix matching folds "\r\n" into a
        // single grapheme and would not see the line feed.
        return text.utf8.last == 0x0A ? newlines : newlines + 1
    }

    /// Centre subtitle of the cover: `N lines · Read only`, or just the
    /// read-only marker when the file has no lines to count.
    static func lineCountText(lineCount: Int?) -> String {
        switch lineCount {
        case nil:
            return String(localized: "Read only")
        case 1:
            return String(localized: "1 line · Read only")
        case let count?:
            return String(localized: "\(count) lines · Read only")
        }
    }

    // MARK: - Actions

    /// The shell-quoted path plus a trailing space, ready to commit a prompt.
    static func insertText(for path: String) -> String {
        DesktopShellPath.stagedArgument(path)
    }

    static func menu(agentAvailable: Bool, hasContent: Bool) -> MenuAvailability {
        MenuAvailability(
            insertPath: true,
            sendToAgent: agentAvailable,
            copy: hasContent,
            share: hasContent,
            sendToAgentInfo: agentAvailable ? nil : String(localized: "No agent is attached to this terminal.")
        )
    }

    /// Calm, code-derived preview failures. Raw host messages never reach the
    /// cover.
    static func message(for failure: FilePreviewFailure) -> String {
        switch failure {
        case .tooLarge: String(localized: "This file is too large to preview.")
        case .binary: String(localized: "Binary files can't be previewed.")
        case .unsupportedImage: String(localized: "This image format can't be previewed.")
        case .unavailable: String(localized: "This file can't be previewed.")
        }
    }
}
