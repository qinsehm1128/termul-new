import Foundation
import Testing
@testable import SeRemote

@Suite("Nocturne real-store session files")
@MainActor
struct NocturneSessionFilesTests {
    @Test("Unavailable byte counts stay absent instead of using prototype metadata")
    func byteCountMissing() {
        #expect(FilePanelPresentation.byteCount(nil) == nil)
        #expect(FilePanelPresentation.byteCount(-1) == nil)
        #expect(FilePanelPresentation.byteCount(0) == ByteCountFormatter.string(fromByteCount: 0, countStyle: .file))
    }

    @Test("Host file sizes use the localized system byte formatter")
    func byteCountReal() {
        for bytes in [1, 8_192, 4_000_000, Int.max] {
            #expect(FilePanelPresentation.byteCount(bytes) == ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file))
        }
    }

    @Test("Host modification timestamps are milliseconds, never a fabricated age")
    func modificationDate() {
        #expect(FilePanelPresentation.modificationDate(1_700_000_000_000) == Date(timeIntervalSince1970: 1_700_000_000))
        #expect(FilePanelPresentation.modificationDate(0) == nil)
        #expect(FilePanelPresentation.modificationDate(nil) == nil)
        #expect(FilePanelPresentation.modificationDate(-1) == nil)
        #expect(FilePanelPresentation.modificationDate(.nan) == nil)
        #expect(FilePanelPresentation.modificationDate(.infinity) == nil)
    }

    @Test("Read-only text preview accepts only the supported host encoding")
    func textEncoding() {
        for encoding: String? in [nil, "utf-8", "UTF-8", "utf8", "UTF8"] {
            #expect(FilePanelPresentation.supportsTextPreview(encoding: encoding))
        }
        for encoding in ["base64", "binary", "utf-16", "", "unknown"] {
            #expect(!FilePanelPresentation.supportsTextPreview(encoding: encoding))
        }
    }

    @Test("Terminal geometry displays host values and hides missing or invalid values")
    func terminalDimensions() {
        #expect(TerminalPanelPresentation.dimensions(cols: 132, rows: 48) == "132 × 48")
        #expect(TerminalPanelPresentation.dimensions(cols: nil, rows: 24) == nil)
        #expect(TerminalPanelPresentation.dimensions(cols: 80, rows: nil) == nil)
        #expect(TerminalPanelPresentation.dimensions(cols: 0, rows: 24) == nil)
        #expect(TerminalPanelPresentation.dimensions(cols: 80, rows: -1) == nil)
        #expect(TerminalPanelPresentation.dimensions(cols: nil, rows: nil) == nil)
    }
}
