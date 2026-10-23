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
"""#

let scratch = ProcessInfo.processInfo.environment["QIN_CODE_SCRATCH_DIR"]
    .map { URL(fileURLWithPath: $0, isDirectory: true) } ?? FileManager.default.temporaryDirectory
let directory = scratch.appendingPathComponent("ios-engine-model-test-\(UUID().uuidString)", isDirectory: true)
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
defer { try? FileManager.default.removeItem(at: directory) }
let testSource = directory.appendingPathComponent("main.swift")
try (projectModels + assertions).write(to: testSource, atomically: true, encoding: .utf8)
let process = Process()
process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
process.arguments = ["swift", testSource.path]
try process.run()
process.waitUntilExit()
guard process.terminationReason == .exit && process.terminationStatus == 0 else {
    fatalError("Production project model checks failed (status \(process.terminationStatus))")
}
