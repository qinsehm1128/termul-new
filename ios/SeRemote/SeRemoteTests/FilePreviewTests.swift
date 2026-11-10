import Foundation
import Testing
@testable import SeRemote

@Suite("File preview type dispatch")
struct FilePreviewKindTests {
    @Test("Markdown extensions render as Markdown, case-insensitively")
    func markdownExtensions() {
        for name in ["README.md", "notes.markdown", "page.mdx", "UPPER.MD"] {
            #expect(FilePreviewKind.resolve(name: name, encoding: nil) == .markdown)
            #expect(FilePreviewKind.resolve(name: name, encoding: "utf-8") == .markdown)
        }
    }

    @Test("HTML extensions render as sandboxed HTML")
    func htmlExtensions() {
        for name in ["index.html", "page.htm", "INDEX.HTML"] {
            #expect(FilePreviewKind.resolve(name: name, encoding: nil) == .html)
        }
    }

    @Test("Host raster allowlist maps to its MIME types")
    func imageExtensions() {
        let expected: [String: String] = [
            "png": "image/png",
            "apng": "image/png",
            "jpg": "image/jpeg",
            "jpeg": "image/jpeg",
            "jfif": "image/jpeg",
            "gif": "image/gif",
            "webp": "image/webp",
            "bmp": "image/bmp",
            "ico": "image/x-icon",
            "avif": "image/avif",
        ]
        for (fileExtension, mime) in expected {
            #expect(
                FilePreviewKind.resolve(name: "photo.\(fileExtension)", encoding: nil)
                    == .image(.init(mime: mime))
            )
            #expect(
                FilePreviewKind.resolve(name: "PHOTO.\(fileExtension.uppercased())", encoding: nil)
                    == .image(.init(mime: mime))
            )
        }
    }

    @Test("SVGs stay text because the host refuses them as images")
    func svgIsText() {
        #expect(FilePreviewKind.resolve(name: "icon.svg", encoding: nil) == .text)
        #expect(FilePreviewKind.resolve(name: "icon.svg", encoding: "utf-8") == .text)
    }

    @Test("HEIC and HEIF are recognised images the host cannot serve")
    func unsupportedImages() {
        #expect(FilePreviewKind.resolve(name: "IMG_0001.HEIC", encoding: nil) == .unsupportedImage)
        #expect(FilePreviewKind.resolve(name: "IMG_0001.heif", encoding: nil) == .unsupportedImage)
    }

    @Test("Unknown and extensionless names stay plain text")
    func plainText() {
        #expect(FilePreviewKind.resolve(name: "Makefile", encoding: nil) == .text)
        #expect(FilePreviewKind.resolve(name: "Package.swift", encoding: "utf-8") == .text)
        #expect(FilePreviewKind.resolve(name: "archive.tar.gz", encoding: nil) == .text)
    }

    @Test("Non-UTF-8 encodings are not previewable text")
    func nonUTF8IsUnsupported() {
        #expect(FilePreviewKind.resolve(name: "notes.md", encoding: "utf-16") == .unsupported)
        #expect(FilePreviewKind.resolve(name: "notes.txt", encoding: "latin1") == .unsupported)
        #expect(FilePreviewKind.resolve(name: "index.html", encoding: "utf-16") == .unsupported)
    }

    @Test("Rendered mode is offered only for Markdown and HTML")
    func renderedMode() {
        #expect(FilePreviewKind.markdown.supportsRenderedMode)
        #expect(FilePreviewKind.html.supportsRenderedMode)
        #expect(!FilePreviewKind.text.supportsRenderedMode)
        #expect(!FilePreviewKind.image(.init(mime: "image/png")).supportsRenderedMode)
        #expect(!FilePreviewKind.unsupported.supportsRenderedMode)
    }
}

@Suite("File preview failure mapping")
struct FilePreviewFailureTests {
    @Test("Host codes map to calm states")
    func hostCodes() {
        #expect(FilePreviewFailure.resolve(HostError.ipc("x", "FILE_TOO_LARGE")) == .tooLarge)
        #expect(FilePreviewFailure.resolve(HostError.ipc("x", "BINARY_FILE")) == .binary)
        #expect(FilePreviewFailure.resolve(HostError.ipc("x", "UNSUPPORTED_IMAGE_FORMAT")) == .unsupportedImage)
        #expect(FilePreviewFailure.resolve(HostError.ipc("x", "INVALID_IMAGE")) == .unsupportedImage)
        #expect(FilePreviewFailure.resolve(HostError.ipc("x", "READ_ERROR")) == .unavailable)
        #expect(FilePreviewFailure.resolve(HostError.ipc("x", "PATH_TRAVERSAL")) == .unavailable)
    }

    @Test("Transport failures never leak a raw message into the card")
    func transportFailures() {
        #expect(FilePreviewFailure.resolve(HostError.network("socket closed")) == .unavailable)
        #expect(FilePreviewFailure.resolve(HostError.unexpected("bad json")) == .unavailable)
        #expect(FilePreviewFailure.resolve(URLError(.timedOut)) == nil)
    }
}

@Suite("HTML sandbox policy")
struct HTMLSandboxPolicyTests {
    @Test("Only about:blank and data URLs are local")
    func allowedSchemes() throws {
        #expect(HTMLSandboxPolicy.allows(nil))
        #expect(HTMLSandboxPolicy.allows(try #require(URL(string: "about:blank"))))
        #expect(HTMLSandboxPolicy.allows(try #require(URL(string: "data:text/html,<p>hi</p>"))))
        #expect(HTMLSandboxPolicy.allows(try #require(URL(string: "data:image/png;base64,AAAA"))))
    }

    @Test("Network, file and script URLs are blocked")
    func blockedSchemes() throws {
        for raw in [
            "https://example.com/a.js",
            "http://example.com/a.png",
            "file:///Users/me/secret.txt",
            "javascript:alert(1)",
            "mailto:a@example.com",
            "about:config",
            "blob:https://example.com/id",
        ] {
            let url = try #require(URL(string: raw))
            #expect(!HTMLSandboxPolicy.allows(url), "Expected \(raw) to be blocked")
        }
    }

    @Test("The CSP is injected inside head and before author markup")
    func policyInjection() {
        let document = HTMLSandboxPolicy.sanitizedDocument(
            "<!DOCTYPE html><html><head><title>t</title></head><body><p>hi</p></body></html>"
        )
        let csp = document.range(of: "Content-Security-Policy")
        let title = document.range(of: "<title>")
        #expect(csp != nil)
        #expect(title != nil)
        if let csp, let title {
            #expect(csp.lowerBound < title.lowerBound)
        }
        #expect(document.contains("default-src 'none'"))
        #expect(document.contains("script-src 'none'"))
        #expect(document.contains("img-src data:"))
    }

    @Test("Documents without a head still start with the policy")
    func fragmentInjection() {
        let document = HTMLSandboxPolicy.sanitizedDocument("<p>hello</p>")
        #expect(document.hasPrefix("<meta http-equiv=\"Content-Security-Policy\""))
        #expect(document.hasSuffix("<p>hello</p>"))
    }

    @Test("Nocturne text colours are injected as CSS values")
    func colours() {
        let document = HTMLSandboxPolicy.sanitizedDocument(
            "<html><head></head><body>x</body></html>",
            textColor: "#F2F0E8",
            backgroundColor: "transparent"
        )
        #expect(document.contains("color: #F2F0E8"))
        #expect(document.contains("background: transparent"))
    }
}

@Suite("Image preview request")
struct ImagePreviewRequestTests {
    @Test("The image URL keeps the full host path in the query")
    func queryEncoding() throws {
        let origin = try #require(URL(string: "https://mac.example.com/"))
        let path = "/Users/q/My Project/图 1.png"
        let url = try #require(HostHTTP.fileImageURL(origin: origin, path: path))
        let components = try #require(URLComponents(url: url, resolvingAgainstBaseURL: false))
        #expect(components.path == "/fs/image")
        #expect(components.queryItems?.first { $0.name == "path" }?.value == path)
        #expect(!url.absoluteString.contains("//fs/image"))
    }

    @Test("A trailing origin slash does not double up")
    func trailingSlash() throws {
        let withSlash = try #require(URL(string: "https://mac.example.com/base/"))
        let withoutSlash = try #require(URL(string: "https://mac.example.com/base"))
        let first = try #require(HostHTTP.fileImageURL(origin: withSlash, path: "/a/b.png"))
        let second = try #require(HostHTTP.fileImageURL(origin: withoutSlash, path: "/a/b.png"))
        #expect(first == second)
    }

    @Test("Host image errors keep their codes for the preview state")
    func errorParsing() throws {
        let cases: [(Int, String, String)] = [
            (413, "FILE_TOO_LARGE", "FILE_TOO_LARGE"),
            (415, "UNSUPPORTED_IMAGE_FORMAT", "UNSUPPORTED_IMAGE_FORMAT"),
            (415, "INVALID_IMAGE", "INVALID_IMAGE"),
        ]
        for (status, bodyCode, expected) in cases {
            let body = try #require(
                #"{"success":false,"error":"nope","code":"\#(bodyCode)"}"#.data(using: .utf8)
            )
            let error = try #require(HostHTTP.fileResponseError(statusCode: status, data: body))
            guard case let .ipc(_, code) = error else {
                Issue.record("Expected an ipc error for \(bodyCode)")
                continue
            }
            #expect(code == expected)
        }
    }
}

@Suite("Markdown block parser")
struct MarkdownBlockParserTests {
    @Test("Headings, lists, quotes and rules keep their structure")
    func structure() {
        let blocks = MarkdownBlockParser.parse(
            """
            # Title

            - one
              - nested
            1. first

            > quoted

            ---
            """
        )
        #expect(blocks == [
            .heading(level: 1, text: "Title"),
            .listItem(text: "one", ordered: false, index: 0, depth: 0),
            .listItem(text: "nested", ordered: false, index: 0, depth: 1),
            .listItem(text: "first", ordered: true, index: 1, depth: 0),
            .quote("quoted"),
            .rule,
        ])
    }

    @Test("Fenced code keeps its language and raw lines")
    func fencedCode() {
        let blocks = MarkdownBlockParser.parse(
            """
            ```swift
            let x = 1

            let y = 2
            ```
            """
        )
        #expect(blocks == [.code(language: "swift", text: "let x = 1\n\nlet y = 2")])
    }

    @Test("Soft-wrapped paragraph lines join")
    func paragraphs() {
        #expect(MarkdownBlockParser.parse("one\ntwo\n\nthree") == [
            .paragraph("one two"),
            .paragraph("three"),
        ])
    }

    @Test("Pipe tables keep header and rows")
    func tables() {
        let blocks = MarkdownBlockParser.parse(
            """
            | Name | Value |
            | --- | ---: |
            | a | 1 |
            | b | 2 |
            """
        )
        #expect(blocks == [
            .table(header: ["Name", "Value"], rows: [["a", "1"], ["b", "2"]]),
        ])
    }

    @Test("A pipe line without a separator stays a paragraph")
    func notATable() {
        #expect(MarkdownBlockParser.parse("a | b") == [.paragraph("a | b")])
    }
}
