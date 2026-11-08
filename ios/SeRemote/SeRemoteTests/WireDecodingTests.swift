import Foundation
import Testing
@testable import SeRemote

@Suite("Wire decoding")
struct WireDecodingTests {
    @Test("JSONValue decodes every JSON kind")
    func jsonValueKinds() throws {
        let data = Data(#"{"a":null,"b":true,"c":2,"d":"x","e":[1,"y"],"f":{"g":"h"}}"#.utf8)
        let value = try JSONDecoder().decode(JSONValue.self, from: data)
        let object = try #require(value.object)
        #expect(object["a"] == .null)
        #expect(object["b"] == .bool(true))
        #expect(object["c"] == .number(2))
        #expect(object["d"]?.string == "x")
        #expect(object["e"]?.stringArray == ["y"])
        #expect(object["f"]?.stringMap == ["g": "h"])
    }

    @Test("scalarString renders integers without a fraction", arguments: [
        (JSONValue.number(3), "3"), (.number(1.5), "1.5"), (.bool(false), "false"), (.string("opus"), "opus"),
    ])
    func scalarString(value: JSONValue, expected: String) {
        #expect(value.scalarString == expected)
    }

    @Test("scalarString ignores containers")
    func scalarStringContainers() {
        #expect(JSONValue.array([]).scalarString == nil)
        #expect(JSONValue.object([:]).scalarString == nil)
    }

    @Test("HostTimestamp parses fractional and basic ISO-8601")
    func timestamps() {
        #expect(HostTimestamp.date(from: "2026-11-08T15:36:28.789Z") != nil)
        #expect(HostTimestamp.date(from: "2026-11-08T15:36:28Z") != nil)
        #expect(HostTimestamp.date(from: "yesterday") == nil)
        #expect(HostTimestamp.date(from: nil) == nil)
    }

    @Test("HostTimestamp folder name takes the last path component")
    func folderName() {
        #expect(HostTimestamp.folderName(from: "/Users/qs/project/me/termul") == "termul")
        #expect(HostTimestamp.folderName(from: "/") == "/")
        #expect(HostTimestamp.folderName(from: "") == nil)
    }

    private func observation(_ json: String) throws -> HostScreenObservation {
        try JSONDecoder().decode(HostScreenObservation.self, from: Data(json.utf8))
    }

    @Test("A consistent ghostty-vt report is ready")
    func screenReady() throws {
        let value = try observation(#"""
        {"terminalScreenEngine":"ghostty-vt","state":"ready","reason":null,"capturedSeq":12,"appliedSeq":12,"observerBuildId":"b1"}
        """#)
        #expect(value.isReady)
        #expect(value.observerBuildId == "b1")
    }

    @Test("Ready with mismatched sequences fails closed")
    func screenSequenceMismatch() throws {
        let value = try observation(#"""
        {"terminalScreenEngine":"ghostty-vt","state":"ready","reason":null,"capturedSeq":12,"appliedSeq":11,"observerBuildId":null}
        """#)
        #expect(!value.isReady)
        #expect(value.reason == .protocol)
    }

    @Test("Unknown engine fails closed instead of looking supported")
    func screenUnknownEngine() throws {
        let value = try observation(#"""
        {"terminalScreenEngine":"future-engine","state":"ready","reason":null,"capturedSeq":1,"appliedSeq":1,"observerBuildId":null}
        """#)
        #expect(!value.isReady)
        #expect(value.state == .unsupported)
        #expect(value.reason == .protocol)
    }

    @Test("Missing required keys fail closed")
    func screenMissingKeys() throws {
        let value = try observation(#"{"terminalScreenEngine":"ghostty-vt","state":"ready","capturedSeq":1,"appliedSeq":1}"#)
        #expect(!value.isReady)
        #expect(value.terminalScreenEngine == .ghosttyVT)
    }

    @Test("Degraded reports keep their reason")
    func screenDegraded() throws {
        let value = try observation(#"""
        {"terminalScreenEngine":"ghostty-vt","state":"catching-up","reason":"overflow","capturedSeq":3,"appliedSeq":1,"observerBuildId":"b"}
        """#)
        #expect(value.state == .catchingUp)
        #expect(value.reason == .overflow)
        #expect(!value.isReady)
    }
}
