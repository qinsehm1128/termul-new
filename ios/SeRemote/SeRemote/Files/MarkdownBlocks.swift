import SwiftUI

/// Block-level Markdown the preview can render without third-party packages.
/// Inline emphasis/links stay with `AttributedString(markdown:)`; this parser
/// only decides structure, which keeps it pure and testable.
nonisolated enum MarkdownBlock: Equatable, Sendable {
    case heading(level: Int, text: String)
    case paragraph(String)
    case listItem(text: String, ordered: Bool, index: Int, depth: Int)
    case quote(String)
    case code(language: String?, text: String)
    case rule
    case table(header: [String], rows: [[String]])
}

enum MarkdownBlockParser {
    nonisolated static func parse(_ markdown: String) -> [MarkdownBlock] {
        let normalized = markdown
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
        let lines = normalized.components(separatedBy: "\n")
        var blocks: [MarkdownBlock] = []
        var paragraph: [String] = []
        var index = 0

        func flushParagraph() {
            guard !paragraph.isEmpty else { return }
            blocks.append(.paragraph(paragraph.joined(separator: " ")))
            paragraph.removeAll()
        }

        while index < lines.count {
            let raw = lines[index]
            let trimmed = raw.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty {
                flushParagraph()
                index += 1
                continue
            }

            if let fence = fenceMarker(trimmed) {
                flushParagraph()
                var codeLines: [String] = []
                index += 1
                while index < lines.count {
                    let candidate = lines[index].trimmingCharacters(in: .whitespaces)
                    if candidate.hasPrefix(String(repeating: fence.character, count: fence.count)) {
                        index += 1
                        break
                    }
                    codeLines.append(lines[index])
                    index += 1
                }
                blocks.append(.code(language: fence.language, text: codeLines.joined(separator: "\n")))
                continue
            }

            if let heading = heading(trimmed) {
                flushParagraph()
                blocks.append(heading)
                index += 1
                continue
            }

            if isRule(trimmed) {
                flushParagraph()
                blocks.append(.rule)
                index += 1
                continue
            }

            if trimmed.hasPrefix(">") {
                flushParagraph()
                var quoteLines: [String] = []
                while index < lines.count {
                    let candidate = lines[index].trimmingCharacters(in: .whitespaces)
                    guard candidate.hasPrefix(">") else { break }
                    quoteLines.append(
                        String(candidate.dropFirst()).trimmingCharacters(in: .whitespaces)
                    )
                    index += 1
                }
                blocks.append(.quote(quoteLines.joined(separator: " ")))
                continue
            }

            if let table = parseTable(lines, index: &index) {
                flushParagraph()
                blocks.append(table)
                continue
            }

            if let item = listItem(raw) {
                flushParagraph()
                blocks.append(item)
                index += 1
                continue
            }

            paragraph.append(trimmed)
            index += 1
        }

        flushParagraph()
        return blocks
    }

    // MARK: - Line classifiers

    private nonisolated static func fenceMarker(
        _ line: String
    ) -> (character: Character, count: Int, language: String?)? {
        for character: Character in ["`", "~"] {
            guard line.hasPrefix(String(repeating: character, count: 3)) else { continue }
            let count = line.prefix { $0 == character }.count
            let info = line.dropFirst(count).trimmingCharacters(in: .whitespaces)
            let language = info.split(separator: " ").first.map(String.init)
            return (character, count, language?.isEmpty == true ? nil : language)
        }
        return nil
    }

    private nonisolated static func heading(_ line: String) -> MarkdownBlock? {
        let level = line.prefix { $0 == "#" }.count
        guard (1 ... 6).contains(level) else { return nil }
        let rest = line.dropFirst(level)
        guard rest.first == " " else { return nil }
        let text = rest.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return nil }
        return .heading(level: level, text: text)
    }

    private nonisolated static func isRule(_ line: String) -> Bool {
        guard line.count >= 3, let character = line.first, "-*_".contains(character) else {
            return false
        }
        return line.allSatisfy { $0 == character }
    }

    private nonisolated static func listItem(_ raw: String) -> MarkdownBlock? {
        let indent = raw.prefix { $0 == " " || $0 == "\t" }.reduce(0) { partial, character in
            partial + (character == "\t" ? 2 : 1)
        }
        let depth = min(indent / 2, 4)
        let body = raw.dropFirst(raw.prefix { $0 == " " || $0 == "\t" }.count)

        if let marker = body.first, "-*+".contains(marker) {
            let rest = body.dropFirst()
            guard rest.first == " " else { return nil }
            let text = rest.dropFirst().trimmingCharacters(in: .whitespaces)
            guard !text.isEmpty else { return nil }
            return .listItem(text: text, ordered: false, index: 0, depth: depth)
        }

        let digits = body.prefix { $0.isNumber }
        guard !digits.isEmpty, let number = Int(digits) else { return nil }
        let rest = body.dropFirst(digits.count)
        guard rest.hasPrefix(". ") || rest.hasPrefix(") ") else { return nil }
        let text = rest.dropFirst(2).trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return nil }
        return .listItem(text: text, ordered: true, index: number, depth: depth)
    }

    private nonisolated static func parseTable(
        _ lines: [String],
        index: inout Int
    ) -> MarkdownBlock? {
        guard index + 1 < lines.count else { return nil }
        let headerLine = lines[index]
        guard headerLine.contains("|"), isTableSeparator(lines[index + 1]) else { return nil }
        let header = cells(headerLine)
        guard !header.isEmpty, header.count == cells(lines[index + 1]).count else { return nil }

        var rows: [[String]] = []
        var cursor = index + 2
        while cursor < lines.count {
            let line = lines[cursor]
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard !trimmed.isEmpty, line.contains("|") else { break }
            rows.append(cells(line))
            cursor += 1
        }
        index = cursor
        return .table(header: header, rows: rows)
    }

    private nonisolated static func cells(_ line: String) -> [String] {
        var body = line.trimmingCharacters(in: .whitespaces)
        if body.hasPrefix("|") { body.removeFirst() }
        if body.hasSuffix("|") { body.removeLast() }
        return body
            .split(separator: "|", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
    }

    private nonisolated static func isTableSeparator(_ line: String) -> Bool {
        guard line.contains("|") else { return false }
        let columns = cells(line)
        guard !columns.isEmpty else { return false }
        return columns.allSatisfy { cell in
            var dashes = Substring(cell.trimmingCharacters(in: .whitespaces))
            if dashes.hasPrefix(":") { dashes = dashes.dropFirst() }
            if dashes.hasSuffix(":") { dashes = dashes.dropLast() }
            return dashes.count >= 3 && dashes.allSatisfy { $0 == "-" }
        }
    }
}

/// Rendered Markdown in the Nocturne content language: headings, readable
/// lists, quoted text, horizontally scrolling code, and pipe tables.
struct MarkdownPreviewView: View {
    @Environment(\.sePalette) private var palette
    private let blocks: [MarkdownBlock]

    init(markdown: String) {
        blocks = MarkdownBlockParser.parse(markdown)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                blockView(block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func blockView(_ block: MarkdownBlock) -> some View {
        switch block {
        case let .heading(level, text):
            Text(inline(text))
                .font(headingFont(level))
                .foregroundStyle(palette.ink)
                .textSelection(.enabled)
                .accessibilityAddTraits(.isHeader)

        case let .paragraph(text):
            Text(inline(text))
                .font(.system(size: 13))
                .lineSpacing(4)
                .foregroundStyle(palette.inkSecondary)
                .textSelection(.enabled)

        case let .listItem(text, ordered, index, depth):
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                Text(ordered ? "\(index)." : "•")
                    .font(.system(size: 13, design: ordered ? .monospaced : .default))
                    .foregroundStyle(palette.inkTertiary)
                    .frame(minWidth: ordered ? 18 : 9, alignment: .trailing)
                Text(inline(text))
                    .font(.system(size: 13))
                    .lineSpacing(3)
                    .foregroundStyle(palette.inkSecondary)
                    .textSelection(.enabled)
            }
            .padding(.leading, CGFloat(depth) * 14)

        case let .quote(text):
            HStack(alignment: .top, spacing: 9) {
                RoundedRectangle(cornerRadius: 1, style: .continuous)
                    .fill(palette.strokeStrong)
                    .frame(width: 2)
                Text(inline(text))
                    .font(.system(size: 13))
                    .italic()
                    .lineSpacing(3)
                    .foregroundStyle(palette.inkSecondary)
                    .textSelection(.enabled)
            }
            .fixedSize(horizontal: false, vertical: true)

        case let .code(language, text):
            VStack(alignment: .leading, spacing: 6) {
                if let language, !language.isEmpty {
                    Text(language)
                        .font(.system(size: 9, weight: .medium, design: .monospaced))
                        .foregroundStyle(palette.inkTertiary)
                }
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(text)
                        .font(.system(size: 11.5, design: .monospaced))
                        .lineSpacing(3)
                        .foregroundStyle(palette.codeInk)
                        .textSelection(.enabled)
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .background(palette.codeCanvas, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }

        case .rule:
            Rectangle()
                .fill(palette.stroke)
                .frame(height: 1)
                .padding(.vertical, 2)

        case let .table(header, rows):
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    tableRow(header, isHeader: true)
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        Rectangle().fill(palette.stroke).frame(height: 1)
                        tableRow(row, isHeader: false)
                    }
                }
            }
            .background(palette.cardAlt.opacity(0.4), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(palette.stroke, lineWidth: 1)
            }
        }
    }

    private func tableRow(_ cells: [String], isHeader: Bool) -> some View {
        HStack(alignment: .top, spacing: 0) {
            ForEach(cells.indices, id: \.self) { index in
                Text(inline(cells[index]))
                    .font(.system(size: 11.5, weight: isHeader ? .semibold : .regular))
                    .foregroundStyle(isHeader ? palette.ink : palette.inkSecondary)
                    .textSelection(.enabled)
                    .frame(minWidth: 64, maxWidth: 180, alignment: .leading)
                    .padding(.horizontal, 9)
                    .padding(.vertical, 7)
                if index < cells.count - 1 {
                    Rectangle().fill(palette.stroke).frame(width: 1)
                }
            }
        }
    }

    private func headingFont(_ level: Int) -> Font {
        switch level {
        case 1: .system(.title2).weight(.bold)
        case 2: .system(.title3).weight(.semibold)
        case 3: .system(.headline)
        default: .system(.subheadline).weight(.semibold)
        }
    }

    private func inline(_ text: String) -> AttributedString {
        var options = AttributedString.MarkdownParsingOptions()
        options.interpretedSyntax = .inlineOnlyPreservingWhitespace
        options.failurePolicy = .returnPartiallyParsedIfPossible
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}
