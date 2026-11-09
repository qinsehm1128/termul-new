import Foundation
import Testing
@testable import SeRemote

@Suite("Nocturne real-store session files")
@MainActor
struct NocturneSessionFilesTests {
    @Test("Unavailable byte counts stay absent instead of using prototype metadata")
    func byteCountMissing() {
        #expect(SessionFilesPresentation.byteCount(nil) == nil)
        #expect(SessionFilesPresentation.byteCount(-1) == nil)
        #expect(SessionFilesPresentation.byteCount(0) == ByteCountFormatter.string(fromByteCount: 0, countStyle: .file))
    }

    @Test("Host file sizes use the localized system byte formatter")
    func byteCountReal() {
        for bytes in [1, 8_192, 4_000_000, Int.max] {
            #expect(SessionFilesPresentation.byteCount(bytes) == ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file))
        }
    }

    @Test("Host modification timestamps are milliseconds, never a fabricated age")
    func modificationDate() {
        #expect(SessionFilesPresentation.modificationDate(1_700_000_000_000) == Date(timeIntervalSince1970: 1_700_000_000))
        #expect(SessionFilesPresentation.modificationDate(0) == nil)
        #expect(SessionFilesPresentation.modificationDate(nil) == nil)
        #expect(SessionFilesPresentation.modificationDate(-1) == nil)
        #expect(SessionFilesPresentation.modificationDate(.nan) == nil)
        #expect(SessionFilesPresentation.modificationDate(.infinity) == nil)
    }

    @Test("Read-only text preview accepts only the supported host encoding")
    func textEncoding() {
        for encoding: String? in [nil, "utf-8", "UTF-8", "utf8", "UTF8"] {
            #expect(SessionFilesPresentation.supportsTextPreview(encoding: encoding))
        }
        for encoding in ["base64", "binary", "utf-16", "", "unknown"] {
            #expect(!SessionFilesPresentation.supportsTextPreview(encoding: encoding))
        }
    }

    @Test("Terminal geometry displays host values and hides missing or invalid values")
    func terminalDimensions() {
        #expect(SessionTerminalPresentation.dimensions(cols: 132, rows: 48) == "132 × 48")
        #expect(SessionTerminalPresentation.dimensions(cols: nil, rows: 24) == nil)
        #expect(SessionTerminalPresentation.dimensions(cols: 80, rows: nil) == nil)
        #expect(SessionTerminalPresentation.dimensions(cols: 0, rows: 24) == nil)
        #expect(SessionTerminalPresentation.dimensions(cols: 80, rows: -1) == nil)
        #expect(SessionTerminalPresentation.dimensions(cols: nil, rows: nil) == nil)
    }
}
