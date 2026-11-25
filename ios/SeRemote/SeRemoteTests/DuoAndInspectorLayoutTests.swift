import Foundation
import Testing
@testable import SeRemote

@Suite("iPhone Duo & collapsible inspector layout")
struct DuoAndInspectorLayoutTests {
    // MARK: - Collapsible inspector default policy across iPhone Duo / iPad / iPhone

    @Test("Wide iPad landscape opens inspector by default; iPad portrait and iPhone Duo start collapsed")
    func defaultInspectorOpenPolicy() {
        // iPad Pro 11" & 13" landscape, iPad mini landscape: roomy landscape defaults open.
        #expect(ShellPresentation.defaultInspectorOpen(width: 1194, height: 834, isRegular: true))
        #expect(ShellPresentation.defaultInspectorOpen(width: 1366, height: 1024, isRegular: true))
        #expect(ShellPresentation.defaultInspectorOpen(width: 1133, height: 744, isRegular: true))

        // iPad portrait: starts collapsed so the terminal canvas keeps room.
        #expect(!ShellPresentation.defaultInspectorOpen(width: 834, height: 1194, isRegular: true))
        #expect(!ShellPresentation.defaultInspectorOpen(width: 744, height: 1133, isRegular: true))
        #expect(!ShellPresentation.defaultInspectorOpen(width: 1024, height: 1366, isRegular: true))

        // iPhone Duo inner screen (regular x regular): both orientations start collapsed
        // so the terminal is never crushed by rail + inspector.
        #expect(!ShellPresentation.defaultInspectorOpen(width: 669, height: 951, isRegular: true))
        #expect(!ShellPresentation.defaultInspectorOpen(width: 951, height: 669, isRegular: true))

        // iPhone Duo outer screen & standard iPhone (compact): never auto-open inline inspector.
        #expect(!ShellPresentation.defaultInspectorOpen(width: 466, height: 678, isRegular: false))
        #expect(!ShellPresentation.defaultInspectorOpen(width: 678, height: 466, isRegular: false))
        #expect(!ShellPresentation.defaultInspectorOpen(width: 393, height: 852, isRegular: false))
    }

    @Test("Inline inspector is available on regular widths at or above the sidebar breakpoint")
    func usesInlineInspectorPolicy() {
        // iPhone Duo inner screen (portrait & landscape) & iPad.
        #expect(ShellPresentation.usesInlineInspector(isRegular: true, width: 669))
        #expect(ShellPresentation.usesInlineInspector(isRegular: true, width: 951))
        #expect(ShellPresentation.usesInlineInspector(isRegular: true, width: 834))
        #expect(ShellPresentation.usesInlineInspector(isRegular: true, width: 1194))

        // iPhone Duo outer screen, 50/50 split narrow width, standard iPhone.
        #expect(!ShellPresentation.usesInlineInspector(isRegular: false, width: 466))
        #expect(!ShellPresentation.usesInlineInspector(isRegular: false, width: 678))
        #expect(!ShellPresentation.usesInlineInspector(isRegular: true, width: 475))
    }

    @Test("Inspector toggle state machine opens, switches segments, and collapses cleanly")
    func inspectorToggleStateMachine() {
        // Closed -> requesting files opens files.
        let openedFiles = ShellPresentation.nextInspectorState(
            isOpen: false,
            current: .files,
            requested: .files
        )
        #expect(openedFiles.isOpen == true)
        #expect(openedFiles.segment == .files)

        // Open on files -> requesting files collapses the inspector.
        let collapsedFiles = ShellPresentation.nextInspectorState(
            isOpen: true,
            current: .files,
            requested: .files
        )
        #expect(collapsedFiles.isOpen == false)
        #expect(collapsedFiles.segment == .files)

        // Open on agent -> requesting files stays open and switches to files.
        let switchedToFiles = ShellPresentation.nextInspectorState(
            isOpen: true,
            current: .agent,
            requested: .files
        )
        #expect(switchedToFiles.isOpen == true)
        #expect(switchedToFiles.segment == .files)

        // Open on files -> requesting agent stays open and switches to agent.
        let switchedToAgent = ShellPresentation.nextInspectorState(
            isOpen: true,
            current: .files,
            requested: .agent
        )
        #expect(switchedToAgent.isOpen == true)
        #expect(switchedToAgent.segment == .agent)

        // Closed -> requesting agent opens directly on agent.
        let openedAgent = ShellPresentation.nextInspectorState(
            isOpen: false,
            current: .files,
            requested: .agent
        )
        #expect(openedAgent.isOpen == true)
        #expect(openedAgent.segment == .agent)
    }

    @Test("Inspector width budget stays within SeLayout bounds and protects terminal canvas room")
    func inspectorWidthBudget() {
        #expect(ShellPresentation.inspectorMinWidth == SeLayout.sidebarWidth.lowerBound)
        #expect(ShellPresentation.inspectorIdealWidth == SeLayout.sidebarWidth.upperBound)
        #expect(ShellPresentation.inspectorAutoOpenMinimumWidth == Double(SeLayout.inspectorMinimumWidth))

        let wide = ShellPresentation.inspectorColumnWidth(availableWidth: 1194, railWidth: 250)
        #expect(wide == ShellPresentation.inspectorIdealWidth)

        let tight = ShellPresentation.inspectorColumnWidth(availableWidth: 744, railWidth: 76)
        #expect(tight >= ShellPresentation.inspectorMinWidth)
        #expect(tight <= ShellPresentation.inspectorIdealWidth)
    }

    // MARK: - iPhone Duo HIG layout rules

    @Test("Tool card grids always use even column counts to avoid straddling the fold crease")
    func evenGridColumns() {
        for (width, regular) in [
            (393.0, false),
            (466.0, false),
            (678.0, false),
            (669.0, true),
            (951.0, true),
            (1024.0, true),
            (1194.0, true),
            (1366.0, true),
        ] {
            let count = ShellPresentation.evenGridColumnCount(width: width, isRegular: regular)
            #expect(count > 0 && count.isMultiple(of: 2), "width=\(width) regular=\(regular) -> \(count)")
        }
        #expect(ShellPresentation.evenGridColumnCount(width: 669, isRegular: true) == 2)
        #expect(ShellPresentation.evenGridColumnCount(width: 951, isRegular: true) == 2)
        #expect(ShellPresentation.evenGridColumnCount(width: 1194, isRegular: true) == 4)
    }

    @Test("Desktop interaction default follows horizontal size class instead of device idiom")
    func desktopInteractionFollowsSizeClass() {
        #expect(ShellPresentation.defaultDesktopInteractionMode(isRegular: true) == .direct)
        #expect(ShellPresentation.defaultDesktopInteractionMode(isRegular: false) == .trackpad)
    }

    @Test("Orientation change preserves explicit user collapse and auto-folds forced opens on narrow postures")
    func inspectorOrientationChangePolicy() {
        // Explicit user collapse stays collapsed even when rotating to wide landscape.
        #expect(
            ShellPresentation.inspectorOverrideAfterOrientationChange(
                currentOverride: false,
                newDefaultOpen: true
            ) == false
        )
        #expect(
            ShellPresentation.inspectorOverrideAfterOrientationChange(
                currentOverride: false,
                newDefaultOpen: false
            ) == false
        )
        // Explicit user open clears when rotating into a narrower posture so the canvas isn't crushed.
        #expect(
            ShellPresentation.inspectorOverrideAfterOrientationChange(
                currentOverride: true,
                newDefaultOpen: false
            ) == nil
        )
        // Explicit user open stays open on roomy landscape.
        #expect(
            ShellPresentation.inspectorOverrideAfterOrientationChange(
                currentOverride: true,
                newDefaultOpen: true
            ) == true
        )
        // No override keeps following default.
        #expect(
            ShellPresentation.inspectorOverrideAfterOrientationChange(
                currentOverride: nil,
                newDefaultOpen: true
            ) == nil
        )
    }

    @Test("Every navigation destination exposes a non-empty SF Symbol for native TabView and Duo vertical bar")
    func destinationSystemImages() {
        for destination in SidebarDestination.allCases {
            #expect(!destination.systemImage.isEmpty, "\(destination.rawValue)")
        }
        #expect(Set(SidebarDestination.allCases.map(\.systemImage)).count == SidebarDestination.allCases.count)
        #expect(SidebarDestination.tools.systemImage != "folder")
    }
}
