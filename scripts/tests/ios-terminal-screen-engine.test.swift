#!/usr/bin/env swift
import Foundation

// Run from any directory: swift scripts/tests/ios-terminal-screen-engine.test.swift
// Compile the real Foundation-only project-list models, not a copied test model.
// The rest of ProjectModels.swift contains unrelated chat/catalog dependencies.
let repository = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
let modelURL = repository.appendingPathComponent("ios/SeRemote/SeRemote/Projects/ProjectModels.swift")
let source = try String(contentsOf: modelURL, encoding: .utf8)
guard let catalogBoundary = source.range(of: "struct CatalogAgent:") else {
    fatalError("ProjectModels.swift layout changed; update the standalone model-test boundary")
}
let projectModels = String(source[..<catalogBoundary.lowerBound])
let observationURL = repository.appendingPathComponent("ios/SeRemote/SeRemote/Terminal/HostScreenObservation.swift")
let observationModel = try String(contentsOf: observationURL, encoding: .utf8)
let socketURL = repository.appendingPathComponent("ios/SeRemote/SeRemote/Networking/TerminalSocket.swift")
let socketSource = try String(contentsOf: socketURL, encoding: .utf8)
guard let socketBoundary = socketSource.range(of: "@MainActor") else {
    fatalError("TerminalSocket.swift layout changed; update the standalone reply-model boundary")
}
let socketModels = String(socketSource[..<socketBoundary.lowerBound])
let assertions = #"""

func check(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else { fatalError(message) }
}

let fixtures: [(String, String, HostTerminalScreenEngine)] = [
    ("missing", #"{"id":"p","name":"Project"}"#, .existing),
    ("existing", #"{"id":"p","name":"Project","terminalScreenEngine":"existing"}"#, .existing),
    ("ghostty-vt", #"{"id":"p","name":"Project","terminalScreenEngine":"ghostty-vt"}"#, .ghosttyVT),
    ("unknown", #"{"id":"p","name":"Project","terminalScreenEngine":"future-engine"}"#, .existing),
    ("null", #"{"id":"p","name":"Project","terminalScreenEngine":null}"#, .existing)
]
let decoder = JSONDecoder()
for (name, json, expected) in fixtures {
    let project = try decoder.decode(HostProject.self, from: Data(json.utf8))
    check(project.terminalScreenEngine == expected, "\(name): incorrect engine")
    check(project.id == "p" && project.name == "Project", "\(name): identity changed")
    check(!project.isArchived && !project.isDefault && !project.isRunning,
          "\(name): older host defaults changed")
    check(project.liveTerminalCount == 0, "\(name): terminal count default changed")
    print("PASS \(name) -> \(project.terminalScreenEngine.rawValue)")
}

let listJSON = "{\"projects\":[" + fixtures.map { $0.1 }.joined(separator: ",") + "]}"
let payload = try decoder.decode(ProjectListPayload.self, from: Data(listJSON.utf8))
check(payload.projects.map(\.terminalScreenEngine) == fixtures.map { $0.2 },
      "Mixed-version project list failed to decode")
check(payload.groups.isEmpty && payload.defaultProjectId == nil, "List defaults changed")
check(HostTerminalScreenEngine.existing.rawValue == "existing", "Existing wire key changed")
check(HostTerminalScreenEngine.ghosttyVT.rawValue == "ghostty-vt", "Ghostty wire key changed")
print("PASS mixed-version project list and exact public wire values")

func decodeObservation(_ json: String) throws -> HostScreenObservation {
    try decoder.decode(HostScreenObservation.self, from: Data(json.utf8))
}
func statusJSON(
    engine: String = "ghostty-vt", state: String = "ready", reason: String = "null",
    captured: String = "42", applied: String = "42", build: String = "\"observer-test\""
) -> String {
    """
    {"terminalScreenEngine":"\(engine)","state":"\(state)","reason":\(reason),
     "capturedSeq":\(captured),"appliedSeq":\(applied),"observerBuildId":\(build)}
    """
}

let missingSummary = try decoder.decode(LiveTerminalSummary.self, from: Data(#"{"id":"old"}"#.utf8))
let missingObservation = missingSummary.screenObservation ?? .legacyHost
check(missingObservation.state == .unsupported && missingObservation.reason == .legacyHost,
      "Missing old-host observation must be unsupported/legacy-host")
check(!missingObservation.isReady, "Missing host state must never be ready")
let preferenceOnly = try decoder.decode(LiveTerminalSummary.self, from: Data(
    #"{"id":"new","terminalScreenEngine":"ghostty-vt"}"#.utf8))
check(!(preferenceOnly.screenObservation ?? .legacyHost).isReady,
      "Engine metadata alone must not imply observer readiness")
print("PASS missing observation and engine-only metadata never imply ready")

let malformed: [(String, String)] = [
    ("missing status", "{}"),
    ("missing state", #"{"terminalScreenEngine":"ghostty-vt","reason":null,"capturedSeq":1,"appliedSeq":1,"observerBuildId":null}"#),
    ("unknown state", statusJSON(state: "future-state")),
    ("unknown engine", statusJSON(engine: "future-engine")),
    ("unknown reason", statusJSON(state: "degraded", reason: "\"future-reason\"")),
    ("ready existing engine", statusJSON(engine: "existing")),
    ("ready with reason", statusJSON(reason: "\"timeout\"")),
    ("ready unequal sequence", statusJSON(applied: "41")),
    ("ready negative sequence", statusJSON(captured: "-1", applied: "-1")),
    ("ready fractional sequence", statusJSON(captured: "1.5", applied: "1.5")),
    ("ready unsafe sequence", statusJSON(captured: "9007199254740992", applied: "9007199254740992")),
    ("ready null sequence", statusJSON(applied: "null")),
    ("ready missing reason", #"{"terminalScreenEngine":"ghostty-vt","state":"ready","capturedSeq":1,"appliedSeq":1,"observerBuildId":null}"#),
    ("ready missing build", #"{"terminalScreenEngine":"ghostty-vt","state":"ready","reason":null,"capturedSeq":1,"appliedSeq":1}"#),
    ("wrong shape", "[]")
]
for (name, json) in malformed {
    let observation = try decodeObservation(json)
    check(observation.state == .unsupported && observation.reason == .protocol && !observation.isReady,
          "\(name): malformed/unknown host state must be unsupported/protocol")
    print("PASS \(name) -> unsupported/protocol")
}

for seq in ["0", "42", "9007199254740991"] {
    let observation = try decodeObservation(statusJSON(captured: seq, applied: seq))
    check(observation.isReady && observation.state == .ready && observation.reason == nil,
          "Valid ghostty-vt ready report must be accepted")
    check(observation.observerBuildId == "observer-test", "Observer identity was lost")
}
let nullBuild = try decodeObservation(statusJSON(build: "null"))
check(nullBuild.isReady && nullBuild.observerBuildId == nil, "Nullable build identity must decode")
print("PASS valid ready status, sequence boundaries and nullable observer build")

for state in ["disabled", "starting", "catching-up", "degraded", "unsupported"] {
    let observation = try decodeObservation(statusJSON(state: state, applied: "40"))
    check(observation.state.rawValue == state && !observation.isReady, "\(state) must not imply ready")
}
for reason in ["helper-missing", "identity-mismatch", "overflow", "timeout", "crash", "protocol", "size-limit", "legacy-host"] {
    let observation = try decodeObservation(statusJSON(state: "degraded", reason: "\"\(reason)\""))
    check(observation.reason?.rawValue == reason && !observation.isReady, "\(reason) must be retained")
}
print("PASS all public non-ready states and reasons")

let runtimeList = """
[{"id":"old"},
 {"id":"missing","terminalScreenEngine":"ghostty-vt"},
 {"id":"unknown","screenObservation":\(statusJSON(state: "future-state"))},
 {"id":"invalid","screenObservation":\(statusJSON(applied: "1"))},
 {"id":"valid","terminalScreenEngine":"ghostty-vt","screenObservation":\(statusJSON())}]
"""
let summaries = try decoder.decode([LiveTerminalSummary].self, from: Data(runtimeList.utf8))
check(summaries.map { ($0.screenObservation ?? .legacyHost).isReady } == [false, false, false, false, true],
      "Mixed-host terminal list must decode without inventing readiness")
let spawn = try decoder.decode(SpawnedTerminal.self, from: Data(
    #"{"id":"spawned","terminalScreenEngine":"ghostty-vt"}"#.utf8))
let attach = try decoder.decode(TerminalAttachResult.self, from: Data(
    #"{"id":"attached","terminalScreenEngine":"ghostty-vt"}"#.utf8))
check(spawn.terminalScreenEngine == .ghosttyVT && attach.terminalScreenEngine == .ghosttyVT,
      "Spawn/attach must preserve runtime engine metadata")
let legacyAttach = try decoder.decode(TerminalAttachResult.self, from: Data(#"{"id":"old"}"#.utf8))
check(legacyAttach.terminalScreenEngine == nil, "Legacy attach should not invent engine metadata")
print("PASS mixed-host terminal list and spawn/attach runtime metadata")
"""#

let scratch = ProcessInfo.processInfo.environment["QIN_CODE_SCRATCH_DIR"]
    .map { URL(fileURLWithPath: $0, isDirectory: true) } ?? FileManager.default.temporaryDirectory
let directory = scratch.appendingPathComponent("ios-engine-model-test-\(UUID().uuidString)", isDirectory: true)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: directory) }
let testSource = directory.appendingPathComponent("main.swift")
try (projectModels + observationModel + socketModels + assertions).write(to: testSource, atomically: true, encoding: .utf8)
let process = Process()
process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
process.arguments = ["swift", testSource.path]
try process.run()
process.waitUntilExit()
guard process.terminationReason == .exit && process.terminationStatus == 0 else {
    fatalError("Production project model checks failed (status \(process.terminationStatus))")
}
