import Foundation

/// Pure presentation decisions for the R2 session workbench. Keeping these
/// here lets the views stay declarative and gives Swift Testing something
/// deterministic to assert without instantiating stores or SwiftUI.
enum NocturneR2Presentation {
    /// Two-letter mark derived from the real agent name (prototype `.agentmark`).
    static func agentMark(_ rawName: String) -> String {
        let name = rawName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return "Ag" }
        let words = name.split(separator: " ")
        if words.count >= 2,
           let first = words.first?.first,
           let second = words.dropFirst().first?.first {
            return String([first, second]).uppercased()
        }
        return String(name.prefix(2)).uppercased()
    }

    /// Real host status mapped to the prototype's state color.
    static func toolTone(_ status: String) -> R2Tone {
        switch status {
        case "failed": .failed
        case "pending", "in_progress": .attention
        case "completed", "success": .live
        default: .quiet
        }
    }

    /// `net · 8 files` — count comes from the host listing only.
    static func folderSummary(name: String?, count: Int) -> String {
        let folder = name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return "\(folder.isEmpty ? "Files" : folder) · \(count) files"
    }

    /// PTY meta line in the prototype's mono voice.
    static func terminalMetaLabel(isConnecting: Bool, isConnected: Bool) -> String {
        if isConnecting { return "PTY · Connecting" }
        return isConnected ? "PTY · Ready" : "PTY · Offline"
    }

    /// Non-directory entries the host actually returned.
    static func readableFileCount(_ entries: [DirectoryEntry]) -> Int {
        entries.reduce(into: 0) { count, entry in
            if !entry.isDirectory { count += 1 }
        }
    }

    /// Ribbon right-hand text: real agent, model and readable-file count only.
    static func ribbonDetail(agent: String?, model: String?, readableFiles: Int) -> String? {
        var parts: [String] = []
        if let agent = agent?.trimmingCharacters(in: .whitespacesAndNewlines), !agent.isEmpty {
            parts.append(agent)
        }
        if let model = model?.trimmingCharacters(in: .whitespacesAndNewlines),
           !model.isEmpty, model != parts.first {
            parts.append(model)
        }
        if readableFiles > 0 {
            parts.append("\(readableFiles) readable files")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// Permission footer: the number of host-provided choices.
    static func optionSummary(_ count: Int) -> String {
        count == 1 ? "1 option" : "\(count) options"
    }

    /// Reasoning disclosure label; no synthetic step count.
    static func reasonLabel(thoughtCount: Int, active: Bool) -> String {
        active
            ? "Reasoning… · \(thoughtCount) steps"
            : "Reasoning summary · \(thoughtCount) steps"
    }
}
