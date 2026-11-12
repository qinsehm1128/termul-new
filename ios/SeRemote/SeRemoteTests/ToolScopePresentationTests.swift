import Foundation
import Testing
@testable import SeRemote

/// Resolve a catalog key the same way the app does, so label assertions stay
/// locale-independent.
private func L(_ key: String) -> String {
    String(localized: String.LocalizationValue(stringLiteral: key))
}

@Suite("Tool scope presentation")
struct ToolScopePresentationTests {
    @Test func scopeFollowsTheTerminalProject() {
        let scope = ToolScopePresentation.scope(
            terminalProjectID: "p1", projectName: "Termul", terminalCwd: "/work/termul/src")
        #expect(scope.isEnabled)
        #expect(scope.projectID == "p1")
        #expect(scope.projectLabel == "Termul")
        #expect(scope.scopeDetail == "/work/termul/src")
        #expect(scope.disabledReason == L("Open a terminal to scope the tools to its project."))
    }

    @Test func noTerminalDisablesEveryTool() {
        let scope = ToolScopePresentation.scope(terminalProjectID: nil, projectName: nil, terminalCwd: nil)
        #expect(!scope.isEnabled)
        #expect(scope.projectLabel == L("No terminal"))
        #expect(scope.scopeDetail == nil)
        #expect(ToolDestination.hubCases.count == 5)
        #expect(!scope.disabledReason.isEmpty)
    }

    @Test func emptyStringsCountAsNoScope() {
        let scope = ToolScopePresentation.scope(terminalProjectID: "", projectName: "", terminalCwd: "")
        #expect(!scope.isEnabled)
        #expect(scope.scopeDetail == nil)
        #expect(scope.projectLabel == L("No terminal"))
    }

    @Test func terminalDirectoryAloneStillScopesTheTools() {
        let scope = ToolScopePresentation.scope(terminalProjectID: nil, projectName: nil, terminalCwd: "/work/termul")
        #expect(scope.isEnabled)
        // No project name: the directory names the scope, never a host id.
        #expect(scope.projectLabel == "termul")
        #expect(scope.scopeDetail == "/work/termul")
    }

    @Test func projectNameWinsOverTheDirectory() {
        let scope = ToolScopePresentation.scope(
            terminalProjectID: "p1", projectName: "Termul", terminalCwd: "/work/termul/packages/app")
        #expect(scope.projectLabel == "Termul")
        #expect(scope.scopeDetail == "/work/termul/packages/app")
    }

    @Test func noneIsTheEmptyScope() {
        #expect(ToolScope.none == ToolScopePresentation.scope(
            terminalProjectID: nil, projectName: nil, terminalCwd: nil))
    }

    @Test func hubTilesMatchAllCasesAndDropMergedDestinations() {
        #expect(ToolDestination.hubCases == ToolDestination.allCases)
        #expect(ToolDestination.allCases.map(\.rawValue) == ["git", "scheduledTasks", "search", "screenText", "plugins"])
        #expect(ToolDestination.hubCases.map(\.title) == [
            L("Git"), L("Scheduled tasks"), L("Search"), L("Host screen text"), L("Plugins"),
        ])
        #expect(ToolDestination.hubCases.map(\.icon) == [.git, .clock, .search, .textview, .plugins])
    }

    @Test func everyDestinationTalksAboutTheScopeNotAWorkspaceTab() {
        // The hub no longer has a quick-terminal or desktop destination: those
        // live in the terminal landing and the screen page respectively.
        for destination in ToolDestination.allCases {
            #expect(destination.rawValue != "quickTerminals")
            #expect(destination.rawValue != "desktop")
            #expect(destination.rawValue != "file")
        }
    }
}
