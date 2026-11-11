import Foundation
import SwiftUI
import Testing
import UIKit
@testable import SeRemote

/// The workbench owns exactly one mounted canvas (I1). Switching destinations
/// or opening the overlay panel only changes the chrome around it, so the
/// `TerminalScreen` behind the surface must never be recreated — a remount
/// would drop the SwiftTerm scrollback and re-install the host feed.
@Suite("Terminal workbench mount")
@MainActor
struct TerminalWorkbenchMountTests {
    /// A backend that counts canvas installs. Every `installCanvasFeed` call is
    /// one `TerminalScreen.makeUIView`, so the count is the mount count.
    @MainActor
    private final class CanvasProbe: TerminalSurfaceBackend, TerminalSurfaceCanvasHost {
        private(set) var installs = 0

        var items: [TerminalSurfaceItem] {
            [TerminalSurfaceItem(id: "t1", title: "zsh", cwd: "/repo")]
        }
        var activeID: String? { "t1" }
        var activeTitle: String { "zsh" }
        var activeCwd: String? { "/repo" }
        var activeProjectID: String? { nil }
        var agentReady: Bool { false }

        func select(_ id: String) async {}
        func write(_ text: String) async {}
        func resize(cols: Int, rows: Int) {}
        func park() async {}
        func resume() async {}

        var canvasTerminalID: String? { "t1" }
        var canvasCols: Int { 80 }
        var canvasRows: Int { 24 }
        var canvasReplayGrid: TerminalReplayGrid? { nil }
        var canvasLocksToHostCols: Bool { false }

        func installCanvasFeed(_ feed: @escaping @MainActor (Data, Bool) -> Void) {
            installs += 1
        }

        func sendCanvasInput(_ text: String) {}
        func canvasResized(cols: Int, rows: Int) {}
    }

    private struct Harness: View {
        let session: WorkspaceSession
        let model: TerminalWorkbenchModel
        let active: Bool

        var body: some View {
            TerminalWorkbenchScreen(
                session: session,
                model: model,
                isActive: active,
                onBack: {},
                onExternalAction: { _ in }
            )
        }
    }

    private func makeModel(_ session: WorkspaceSession, _ probe: CanvasProbe) -> TerminalWorkbenchModel {
        TerminalWorkbenchModel(
            session: session,
            workspaceBackend: { _ in probe },
            standaloneBackend: { _, _ in probe },
            probesHook: false
        )
    }

    /// The test host app already owns a scene; reusing it keeps the window out
    /// of the deprecated `init(frame:)`/`init()` paths.
    private func makeWindow() throws -> UIWindow {
        let scene = try #require(
            UIApplication.shared.connectedScenes
                .compactMap({ $0 as? UIWindowScene })
                .first,
            "The test host has no window scene to mount the workbench in."
        )
        return UIWindow(windowScene: scene)
    }

    @Test("Toggling visibility and the panel keeps exactly one canvas mount")
    func singleCanvasMount() async throws {
        let session = WorkspaceSession(
            accessURL: URL(string: "http://127.0.0.1:1")!,
            bearer: "test"
        )
        let probe = CanvasProbe()
        let model = makeModel(session, probe)
        await model.open(
            TerminalOpenRequest(attach: .workspace(terminalID: "t1"), title: "zsh", cwd: "/repo")
        )

        let window = try makeWindow()
        window.frame = CGRect(x: 0, y: 0, width: 390, height: 844)
        let controller = UIHostingController(
            rootView: Harness(session: session, model: model, active: true)
        )
        window.rootViewController = controller
        window.isHidden = false
        defer {
            window.isHidden = true
            window.rootViewController = nil
        }

        try await Task.sleep(for: .milliseconds(250))
        #expect(probe.installs == 1)

        // Leaving the workbench and coming back is an opacity change, never a
        // rebuild of the canvas.
        controller.rootView = Harness(session: session, model: model, active: false)
        try await Task.sleep(for: .milliseconds(80))
        controller.rootView = Harness(session: session, model: model, active: true)
        try await Task.sleep(for: .milliseconds(80))
        #expect(probe.installs == 1)

        // The overlay panel is presentation only: it must not reflow or remount
        // the terminal either.
        model.panelExpanded = true
        try await Task.sleep(for: .milliseconds(80))
        model.panelExpanded = false
        try await Task.sleep(for: .milliseconds(80))

        #expect(probe.installs == 1)
    }
}
