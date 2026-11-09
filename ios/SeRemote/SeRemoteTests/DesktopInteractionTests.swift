import Foundation
import CoreGraphics
import Testing
@testable import SeRemote

@Suite("Desktop local interaction policy")
struct DesktopInteractionTests {
    private let bounds = CGRect(x: 0, y: 0, width: 400, height: 400)
    private let wide = CGSize(width: 1920, height: 1080)

    @Test func letterboxIsNotRemoteInput() {
        let viewport = DesktopLocalViewport()
        #expect(viewport.normalized(CGPoint(x: 200, y: 10), in: bounds, imageSize: wide) == nil)
        #expect(viewport.normalized(CGPoint(x: 200, y: 200), in: bounds, imageSize: wide) == CGPoint(x: 0.5, y: 0.5))
        #expect(viewport.normalized(CGPoint(x: 0, y: 87.5), in: bounds, imageSize: wide) == .zero)
        #expect(viewport.normalized(CGPoint(x: 400, y: 312.5), in: bounds, imageSize: wide) == CGPoint(x: 1, y: 1))
    }

    @Test func retinaResolutionDoesNotChangeNormalizedCoordinates() {
        let viewport = DesktopLocalViewport()
        let point = CGPoint(x: 100, y: 200)
        let normal = viewport.normalized(point, in: bounds, imageSize: wide)
        let retina = viewport.normalized(point, in: bounds, imageSize: CGSize(width: 3840, height: 2160))
        #expect(normal == retina)
    }

    @Test func zoomKeepsAnchorAndClampsPan() {
        var viewport = DesktopLocalViewport()
        let anchor = CGPoint(x: 300, y: 200)
        let before = viewport.normalized(anchor, in: bounds, imageSize: wide)
        viewport.magnify(by: 2, around: anchor, in: bounds, imageSize: wide)
        #expect(viewport.normalized(anchor, in: bounds, imageSize: wide) == before)
        viewport.pan = CGSize(width: 10000, height: -10000)
        viewport.constrain(in: bounds, imageSize: wide)
        #expect(viewport.pan == CGSize(width: 200, height: -25))
        viewport.magnify(by: 0.01, around: anchor, in: bounds, imageSize: wide)
        #expect(viewport.zoom == 1)
        #expect(viewport.pan == .zero)
    }

    @Test func rejectsInvalidGeometryAndLimitsZoom() {
        var viewport = DesktopLocalViewport()
        #expect(viewport.normalized(.zero, in: .zero, imageSize: wide) == nil)
        #expect(viewport.normalized(.zero, in: bounds, imageSize: .zero) == nil)
        #expect(viewport.normalized(CGPoint(x: CGFloat.nan, y: 0), in: bounds, imageSize: wide) == nil)
        viewport.magnify(by: .infinity, around: .zero, in: bounds, imageSize: wide)
        #expect(viewport.zoom == 1)
        viewport.magnify(by: 100, around: CGPoint(x: 200, y: 200), in: bounds, imageSize: wide)
        #expect(viewport.zoom == 5)
    }

    @Test func displayViewportsAreIndependent() {
        var displays = ["left": DesktopLocalViewport(), "right": DesktopLocalViewport()]
        displays["left"]?.magnify(by: 3, around: CGPoint(x: 200, y: 200), in: bounds, imageSize: wide)
        #expect(displays["left"]?.zoom == 3)
        #expect(displays["right"]?.zoom == 1)
    }

    @Test func everyControlGateMustBeOpen() {
        for bits in 0..<16 {
            let allowed = DesktopInteractionPolicy.canControl(deviceAllowed: bits & 1 != 0,
                hostAllowed: bits & 2 != 0, active: bits & 4 != 0, streaming: bits & 8 != 0)
            #expect(allowed == (bits == 15))
        }
    }

    @Test func relativeTrackpadMovementClampsToSelectedDisplay() {
        let result = DesktopInteractionPolicy.relativePointer(CGPoint(x: 0.5, y: 0.5),
            delta: CGSize(width: 1000, height: -1000), renderedSize: CGSize(width: 400, height: 225))
        #expect(result == CGPoint(x: 1, y: 0))
        #expect(DesktopInteractionPolicy.relativePointer(result, delta: CGSize(width: 10, height: 10), renderedSize: .zero) == result)
    }

    @Test func stickyShiftUsesKeysButPreservesComposedText() {
        #expect(DesktopKeyboardPolicy.committedKeyCode("1", hasModifiers: true) == "1")
        #expect(DesktopKeyboardPolicy.committedKeyCode("\n", hasModifiers: true) == "Enter")
        #expect(DesktopKeyboardPolicy.committedKeyCode("a", hasModifiers: true) == "a")
        #expect(DesktopKeyboardPolicy.committedKeyCode("a", hasModifiers: false) == nil)
        #expect(DesktopKeyboardPolicy.committedKeyCode("你好", hasModifiers: true) == nil)
        #expect(DesktopKeyboardPolicy.committedKeyCode("e\u{0301}", hasModifiers: true) == nil)
    }

    @Test func modifiedPunctuationHasHardwareCommands() {
        let inputs = DesktopKeyboardPolicy.hardwareShortcutInputs
        for character in "[]\\;,./'`-=\"{}|:<>?~!@#$%^&*()_+ " {
            #expect(inputs.contains(character))
            #expect(DesktopKeyboardPolicy.hostKeyCode(String(character)) != nil)
        }
        #expect(Set(inputs).count == inputs.count)
    }

    @Test func activeDragClampsWhileInitialTapRejectsLetterbox() {
        let viewport = DesktopLocalViewport()
        let outside = CGPoint(x: 700, y: -100)
        #expect(viewport.normalized(outside, in: bounds, imageSize: wide) == nil)
        #expect(viewport.clampedNormalized(outside, in: bounds, imageSize: wide) == CGPoint(x: 1, y: 0))
        #expect(viewport.clampedNormalized(CGPoint(x: -20, y: 600), in: bounds, imageSize: wide) == CGPoint(x: 0, y: 1))
        #expect(viewport.clampedNormalized(CGPoint(x: 200, y: 200), in: bounds, imageSize: wide) == CGPoint(x: 0.5, y: 0.5))
        #expect(viewport.clampedNormalized(CGPoint(x: CGFloat.infinity, y: 0), in: bounds, imageSize: wide) == nil)
        #expect(viewport.clampedNormalized(.zero, in: bounds, imageSize: .zero) == nil)
    }

    @Test func keyboardCodesMatchHostSingleScalarProtocol() {
        #expect(DesktopKeyboardPolicy.hostKeyCode("A") == "a")
        #expect(DesktopKeyboardPolicy.hostKeyCode("1") == "1")
        #expect(DesktopKeyboardPolicy.hostKeyCode(";") == ";")
        #expect(DesktopKeyboardPolicy.hostKeyCode("中") == "中")
        #expect(DesktopKeyboardPolicy.hostKeyCode(" ") == "Space")
        #expect(DesktopKeyboardPolicy.hostKeyCode("\n") == "Enter")
        #expect(DesktopKeyboardPolicy.hostKeyCode("\t") == "Tab")
        #expect(DesktopKeyboardPolicy.hostKeyCode("KeyA") == nil)
        #expect(DesktopKeyboardPolicy.hostKeyCode("Digit1") == nil)
        #expect(DesktopKeyboardPolicy.hostKeyCode("\u{0000}") == nil)
    }
}
