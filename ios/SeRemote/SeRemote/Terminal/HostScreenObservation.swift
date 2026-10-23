import Foundation

/// A read-only, point-in-time host report. This is deliberately independent of
/// the project's engine preference and of the phone's SwiftTerm renderer.
struct HostScreenObservation: Decodable, Hashable, Sendable {
    enum State: String, Decodable, Sendable {
        case disabled, starting, ready, degraded, unsupported
        case catchingUp = "catching-up"
    }

    enum Reason: String, Decodable, Sendable {
        case helperMissing = "helper-missing"
        case identityMismatch = "identity-mismatch"
        case overflow, timeout, crash, `protocol`
        case sizeLimit = "size-limit"
        case legacyHost = "legacy-host"
    }

    let terminalScreenEngine: HostTerminalScreenEngine
    let state: State
    let reason: Reason?
    let capturedSeq: Int
    let appliedSeq: Int
    let observerBuildId: String?

    static let legacyHost = unavailable(reason: .legacyHost)

    static func unavailable(
        reason: Reason,
        engine: HostTerminalScreenEngine = .existing
    ) -> Self {
        Self(terminalScreenEngine: engine, state: .unsupported, reason: reason,
             capturedSeq: 0, appliedSeq: 0, observerBuildId: nil)
    }

    var isReady: Bool {
        terminalScreenEngine == .ghosttyVT && state == .ready && reason == nil &&
            Self.validSequence(capturedSeq) && Self.validSequence(appliedSeq) &&
            capturedSeq == appliedSeq
    }

    private static func validSequence(_ value: Int) -> Bool {
        // The host JSON contract uses JavaScript-safe nonnegative integers.
        value >= 0 && value <= 9_007_199_254_740_991
    }

    private enum CodingKeys: String, CodingKey {
        case terminalScreenEngine, state, reason, capturedSeq, appliedSeq, observerBuildId
    }

    private init(
        terminalScreenEngine: HostTerminalScreenEngine, state: State, reason: Reason?,
        capturedSeq: Int, appliedSeq: Int, observerBuildId: String?
    ) {
        self.terminalScreenEngine = terminalScreenEngine
        self.state = state
        self.reason = reason
        self.capturedSeq = capturedSeq
        self.appliedSeq = appliedSeq
        self.observerBuildId = observerBuildId
    }

    init(from decoder: Decoder) throws {
        // Fail closed without breaking the rest of a mixed-version terminal
        // list. In particular, the preference enum's unknown -> existing
        // fallback must not make an unknown runtime report look supported.
        var engine: HostTerminalScreenEngine = .existing
        do {
            let fields = try decoder.container(keyedBy: CodingKeys.self)
            let rawEngine = try fields.decode(String.self, forKey: .terminalScreenEngine)
            guard let knownEngine = HostTerminalScreenEngine(rawValue: rawEngine) else {
                self = .unavailable(reason: .protocol)
                return
            }
            engine = knownEngine
            let state = try fields.decode(State.self, forKey: .state)
            guard fields.contains(.reason), fields.contains(.observerBuildId) else {
                self = .unavailable(reason: .protocol, engine: engine)
                return
            }
            let reason = try fields.decodeIfPresent(Reason.self, forKey: .reason)
            let capturedSeq = try fields.decode(Int.self, forKey: .capturedSeq)
            let appliedSeq = try fields.decode(Int.self, forKey: .appliedSeq)
            let buildId = try fields.decodeIfPresent(String.self, forKey: .observerBuildId)
            guard Self.validSequence(capturedSeq), Self.validSequence(appliedSeq),
                  state != .ready || (engine == .ghosttyVT && reason == nil && capturedSeq == appliedSeq)
            else {
                self = .unavailable(reason: .protocol, engine: engine)
                return
            }
            self.init(terminalScreenEngine: engine, state: state, reason: reason,
                      capturedSeq: capturedSeq, appliedSeq: appliedSeq, observerBuildId: buildId)
        } catch {
            self = .unavailable(reason: .protocol, engine: engine)
        }
    }
}
