import Foundation
import Testing
@testable import SeRemote

@Suite("Terminal file panel presentation")
struct FilePanelPresentationTests {
    @Test("Root title is the last path component")
    func rootTitle() {
        #expect(FilePanelPresentation.title(rootPath: "/Users/qs/project/termul") == "termul")
        #expect(FilePanelPresentation.title(rootPath: "/Users/qs/project/termul/") == "termul")
        #expect(FilePanelPresentation.title(rootPath: "  /Users/qs/project/termul  ") == "termul")
        #expect(FilePanelPresentation.title(rootPath: "/") == "/")
        #expect(FilePanelPresentation.title(rootPath: "~") == "~")
        #expect(FilePanelPresentation.title(rootPath: "") == String(localized: "Files"))
    }

    @Test("A root reload happens only when the requested root differs")
    func shouldReloadRoot() {
        #expect(FilePanelPresentation.shouldReloadRoot(currentRoot: nil, requestedRoot: "/a/b"))
        #expect(FilePanelPresentation.shouldReloadRoot(currentRoot: "/a/b", requestedRoot: "/a/c"))
        #expect(!FilePanelPresentation.shouldReloadRoot(currentRoot: "/a/b", requestedRoot: "/a/b"))
        #expect(!FilePanelPresentation.shouldReloadRoot(currentRoot: "/a/b/", requestedRoot: "/a/b"))
        #expect(!FilePanelPresentation.shouldReloadRoot(currentRoot: "/a/b", requestedRoot: " /a/b/ "))
        #expect(FilePanelPresentation.shouldReloadRoot(currentRoot: "/a", requestedRoot: "/a/b"))
        #expect(FilePanelPresentation.shouldReloadRoot(currentRoot: "/a/b", requestedRoot: "/"))
        #expect(!FilePanelPresentation.shouldReloadRoot(currentRoot: nil, requestedRoot: "   "))
        #expect(!FilePanelPresentation.shouldReloadRoot(currentRoot: "/a", requestedRoot: ""))
    }

    @Test("Root normalization keeps the filesystem root and drops trailing slashes")
    func normalizedRoot() {
        #expect(FilePanelPresentation.normalizedRoot("/a/b/") == "/a/b")
        #expect(FilePanelPresentation.normalizedRoot("/") == "/")
        #expect(FilePanelPresentation.normalizedRoot("//") == "/")
        #expect(FilePanelPresentation.normalizedRoot("  /a/b  ") == "/a/b")
        #expect(FilePanelPresentation.normalizedRoot("") == "")
    }

    @Test("Line count follows the file's newline shape")
    func lineCount() {
        #expect(FilePanelPresentation.lineCount(in: "") == 0)
        #expect(FilePanelPresentation.lineCount(in: "a") == 1)
        #expect(FilePanelPresentation.lineCount(in: "a\nb") == 2)
        #expect(FilePanelPresentation.lineCount(in: "a\nb\n") == 2)
        #expect(FilePanelPresentation.lineCount(in: "a\n\n") == 2)
        #expect(FilePanelPresentation.lineCount(in: "\n") == 1)
        #expect(FilePanelPresentation.lineCount(in: "a\r\nb\r\n") == 2)
    }

    @Test("Line count exists only for loaded text previews")
    func lineCountForContent() {
        let text = FileContent(content: "one\ntwo\nthree\n", encoding: "utf-8", size: 14, modifiedAt: nil)
        #expect(FilePanelPresentation.lineCount(content: text, kind: .markdown) == 3)
        #expect(FilePanelPresentation.lineCount(content: text, kind: .text) == 3)
        #expect(FilePanelPresentation.lineCount(content: text, kind: .html) == 3)
        #expect(FilePanelPresentation.lineCount(content: text, kind: .image(.init(mime: "image/png"))) == nil)
        #expect(FilePanelPresentation.lineCount(content: text, kind: .unsupported) == nil)
        #expect(FilePanelPresentation.lineCount(content: nil, kind: .text) == nil)
        #expect(FilePanelPresentation.lineCount(content: nil, kind: nil) == nil)
        let empty = FileContent(content: "", encoding: "utf-8", size: 0, modifiedAt: nil)
        #expect(FilePanelPresentation.lineCount(content: empty, kind: .text) == 0)
    }

    @Test("The cover subtitle reads lines followed by the read-only marker")
    func lineCountText() {
        #expect(FilePanelPresentation.lineCountText(lineCount: nil) == String(localized: "Read only"))
        #expect(FilePanelPresentation.lineCountText(lineCount: 342) == String(localized: "\(342) lines · Read only"))
        #expect(FilePanelPresentation.lineCountText(lineCount: 0) == String(localized: "\(0) lines · Read only"))
        #expect(FilePanelPresentation.lineCountText(lineCount: 1) == String(localized: "1 line · Read only"))
    }

    @Test("Insert path is shell-quoted and commits the prompt")
    func insertText() {
        #expect(FilePanelPresentation.insertText(for: "/Users/qs/a.swift") == "/Users/qs/a.swift ")
        #expect(FilePanelPresentation.insertText(for: "/Users/qs/my file.swift") == "'/Users/qs/my file.swift' ")
        #expect(FilePanelPresentation.insertText(for: "/tmp/a'b") == "'/tmp/a'\\''b' ")
    }

    @Test("Send to Agent follows the terminal's agent availability")
    func agentAvailability() {
        let available = FilePanelPresentation.menu(agentAvailable: true, hasContent: true)
        #expect(available.insertPath)
        #expect(available.sendToAgent)
        #expect(available.copy)
        #expect(available.share)
        #expect(available.sendToAgentInfo == nil)

        let unavailable = FilePanelPresentation.menu(agentAvailable: false, hasContent: true)
        #expect(unavailable.insertPath)
        #expect(!unavailable.sendToAgent)
        #expect(unavailable.copy)
        #expect(unavailable.share)
        #expect(unavailable.sendToAgentInfo == String(localized: "No agent is attached to this terminal."))
    }

    @Test("Copy and share need content; insert and agent do not")
    func menuWithoutContent() {
        let empty = FilePanelPresentation.menu(agentAvailable: true, hasContent: false)
        #expect(empty.insertPath)
        #expect(empty.sendToAgent)
        #expect(!empty.copy)
        #expect(!empty.share)
        #expect(empty.sendToAgentInfo == nil)
    }
}
