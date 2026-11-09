import Foundation
import CoreGraphics
import Testing
#if canImport(UIKit)
import UIKit
#endif
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

    @Test func committedEnterAndTabAreKeysWithoutModifiers() {
        for hasModifiers in [false, true] {
            #expect(DesktopKeyboardPolicy.committedKeyCode("\n", hasModifiers: hasModifiers) == "Enter")
            #expect(DesktopKeyboardPolicy.committedKeyCode("\r", hasModifiers: hasModifiers) == "Enter")
            #expect(DesktopKeyboardPolicy.committedKeyCode("\t", hasModifiers: hasModifiers) == "Tab")
        }
    }

    @Test func ordinaryAndIMECommitsRemainText() {
        for text in ["a", "1", ";", " ", "中", "你好", "e\u{0301}", "👩‍💻", "line\nline", "\r\n", "\ttext"] {
            #expect(DesktopKeyboardPolicy.committedKeyCode(text, hasModifiers: false) == nil)
        }
        for text in ["你好", "e\u{0301}", "👩‍💻", "line\nline", "\r\n", "\ttext"] {
            #expect(DesktopKeyboardPolicy.committedKeyCode(text, hasModifiers: true) == nil)
        }
    }

    @Test func hardwareNavigationUsesContractKeyNames() {
        for (usage, code) in [(74, "Home"), (75, "PageUp"), (76, "Delete"), (77, "End"), (78, "PageDown")] {
            #expect(DesktopKeyboardPolicy.hardwareKeyCode(usage) == code)
            #expect(DesktopKeyboardPolicy.consumesHardwareUsage(usage))
        }
    }

    @Test func onlyF1ThroughF12HaveWireCodes() {
        for usage in 58...69 {
            #expect(DesktopKeyboardPolicy.hardwareKeyCode(usage) == "F\(usage - 57)")
            #expect(DesktopKeyboardPolicy.consumesHardwareUsage(usage))
        }
        for usage in 104...115 {
            #expect(DesktopKeyboardPolicy.hardwareKeyCode(usage) == nil)
            #expect(DesktopKeyboardPolicy.consumesHardwareUsage(usage))
        }
    }

    @Test func ordinaryHardwareKeysStayOnUIKitTextPath() {
        for usage in [0, 4, 30, 40, 42, 43, 44, 70, 73, 79, 80, 81, 82, 103, 116, 224] {
            #expect(DesktopKeyboardPolicy.hardwareKeyCode(usage) == nil)
            #expect(!DesktopKeyboardPolicy.consumesHardwareUsage(usage))
        }
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

    #if canImport(UIKit)
    @MainActor @Test func hardwareKeysEmitBalancedPairsWithPhysicalModifiers() {
        let view = DesktopKeyboardTextView()
        view.inputEnabled = true
        var messages: [DesktopInputMessage] = []
        view.onInput = { messages.append($0) }
        let modifiers = DesktopModifiers(shift: true, control: true, alt: true, meta: true)
        for (usage, code) in [(74, "Home"), (75, "PageUp"), (76, "Delete"), (77, "End"), (78, "PageDown"), (58, "F1"), (69, "F12")] {
            messages.removeAll()
            let key = DesktopTestHardwareKey(usage: usage, modifiers: [.shift, .control, .alternate, .command])
            let press = DesktopTestHardwarePress(key: key)
            view.pressesBegan([press], with: nil)
            #expect(messages == [.key(code: code, down: true, modifiers: modifiers),
                                 .key(code: code, down: false, modifiers: modifiers)])
            view.pressesEnded([press], with: nil)
            #expect(messages.count == 2)
        }
    }

    @MainActor @Test func hardwareNavigationMergesStickyAndPhysicalModifiers() throws {
        let view = DesktopKeyboardTextView()
        view.inputEnabled = true
        func controlButton(in root: UIView) -> UIButton? {
            if let button = root as? UIButton,
               (button.configuration?.title ?? button.currentTitle) == "Ctrl" { return button }
            for child in root.subviews {
                if let button = controlButton(in: child) { return button }
            }
            return nil
        }
        let accessory = try #require(view.inputAccessoryView)
        let control = try #require(controlButton(in: accessory))
        control.sendActions(for: .touchUpInside)
        var messages: [DesktopInputMessage] = []
        view.onInput = { messages.append($0) }
        let press = DesktopTestHardwarePress(key: DesktopTestHardwareKey(usage: 74, modifiers: [.shift]))
        view.pressesBegan([press], with: nil)
        let modifiers = DesktopModifiers(shift: true, control: true)
        #expect(messages == [.key(code: "Home", down: true, modifiers: modifiers),
                             .key(code: "Home", down: false, modifiers: modifiers)])
        view.inputEnabled = false
        view.pressesCancelled([press], with: nil)
        #expect(messages.count == 2)
    }

    @MainActor @Test func unsupportedHardwareFunctionKeysNeverProduceRemoteInput() {
        let view = DesktopKeyboardTextView()
        view.inputEnabled = true
        let initialText = view.text
        var messages: [DesktopInputMessage] = []
        view.onInput = { messages.append($0) }
        for usage in 104...115 {
            let press = DesktopTestHardwarePress(key: DesktopTestHardwareKey(usage: usage, modifiers: [.command]))
            view.pressesBegan([press], with: nil)
            view.pressesEnded([press], with: nil)
            view.pressesBegan([press], with: nil)
            view.pressesCancelled([press], with: nil)
        }
        #expect(messages.isEmpty)
        #expect(view.text == initialText)
    }

    @MainActor @Test func committedControlsEmitKeysAndIMEStillEmitsText() {
        let view = DesktopKeyboardTextView()
        view.inputEnabled = true
        var messages: [DesktopInputMessage] = []
        view.onInput = { messages.append($0) }
        for (text, code) in [("\n", "Enter"), ("\r", "Enter"), ("\t", "Tab")] {
            messages.removeAll()
            view.text = text
            view.textViewDidChange(view)
            #expect(messages == [.key(code: code, down: true, modifiers: DesktopModifiers()),
                                 .key(code: code, down: false, modifiers: DesktopModifiers())])
        }
        for text in ["a", " ", "你好", "e\u{0301}", "👩‍💻"] {
            messages.removeAll()
            view.text = text
            view.textViewDidChange(view)
            #expect(messages == [.text(text)])
        }
    }
    #endif

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

#if canImport(UIKit)
@MainActor private final class DesktopTestHardwareKey: UIKey {
    private let usage: UIKeyboardHIDUsage
    private let flags: UIKeyModifierFlags

    init(usage: Int, modifiers: UIKeyModifierFlags) {
        self.usage = UIKeyboardHIDUsage(rawValue: usage)!
        self.flags = modifiers
        super.init()
    }

    required init?(coder: NSCoder) { return nil }

    override var keyCode: UIKeyboardHIDUsage { usage }
    override var modifierFlags: UIKeyModifierFlags { flags }
}

// UIPress is nonisolated in the iOS 27 SDK, unlike the UIKey fixture above.
nonisolated private final class DesktopTestHardwarePress: UIPress {
    private let hardwareKey: UIKey

    init(key: UIKey) {
        hardwareKey = key
        super.init()
    }

    override var key: UIKey? { hardwareKey }
}
#endif
