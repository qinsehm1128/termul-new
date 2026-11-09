import Foundation
import Observation
import OSLog

@MainActor
@Observable
final class AgentChatStore {
    let terminalID: String
    private let client: AgentChatClient
    private(set) var sessionID: String?
    private(set) var agent: String?
    private(set) var cursor: UInt64 = 0
    private(set) var olderCursor: UInt64?
    private(set) var isLoadingOlder = false
    private(set) var isBrowsingOlder = false
    private var isReturningLatest = false
    private var historyEpoch: UInt64 = 0
    static let maximumMessages = 1024
    static let maximumBytes = 16 * 1024 * 1024
    private(set) var messages: [ChatMessage] = []
    private(set) var deliveries: [AgentChatDelivery] = []
    private(set) var toolCalls: [AgentChatToolCall] = []
    private var messageToolIDs: [String: String] = [:]
    private(set) var hookKind: String?
    private(set) var isReady = false
    private(set) var isLoading = true
    private(set) var isSubmitting = false
    private(set) var isControlling = false
    private(set) var errorMessage: String?
    private(set) var reason: String?
    private(set) var submissionNotice: String?
    var draft = ""
    var canView: Bool
    var canInput: Bool
    private var refreshing = false
    private var mutationError: String?

    init(terminalID: String, client: AgentChatClient, canView: Bool = true, canInput: Bool = true) {
        self.terminalID = terminalID
        self.client = client
        self.canView = canView
        self.canInput = canInput
    }

    var canSend: Bool {
        canView && canInput && isReady && !isSubmitting && !isControlling
            && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var canControl: Bool { canView && canInput && isReady && !isSubmitting && !isControlling }

    /// One reader owns polling. Transient failures leave the transcript intact and expose a terminal fallback.
    func follow() async {
        HostLog.ui.info("agent_chat follow started")
        defer { HostLog.ui.info("agent_chat follow ended") }
        while !Task.isCancelled {
            await refresh()
            do { try await Task.sleep(for: .seconds(1)) } catch { break }
        }
    }

    func refresh() async {
        guard canView else {
            isLoading = false
            isReady = false
            olderCursor = nil
            isBrowsingOlder = false
            cursor = 0
            historyEpoch &+= 1
            messages = []
            deliveries = []
            toolCalls = []
            messageToolIDs = [:]
            reason = String(localized: "This link does not allow viewing terminal conversations.")
            return
        }
        guard !refreshing, !isBrowsingOlder, !isLoadingOlder else { return }
        let epoch = historyEpoch
        refreshing = true
        defer { refreshing = false; isLoading = false }
        do {
            let snapshot = try await client.snapshot(terminalID, cursor)
            guard !Task.isCancelled, canView, epoch == historyEpoch, !isBrowsingOlder else { return }
            apply(snapshot)
        } catch is CancellationError {
            return
        } catch {
            isReady = false
            errorMessage = String(localized: "Conversation unavailable. Return to the terminal or retry.")
            // Never log the response body, prompt, terminal id, or credentials.
            HostLog.ui.error("agent_chat snapshot failed")
        }
    }

    func apply(_ snapshot: AgentChatSnapshot) {
        guard snapshot.terminalID == terminalID, canView else { return }
        // Terminal cursors are monotonic; only an explicit reset or session change replaces history.
        let reset = snapshot.reset || sessionID != snapshot.sessionID
        guard reset || snapshot.cursor >= cursor else { return }
        if reset {
            isBrowsingOlder = false
            historyEpoch &+= 1
            olderCursor = snapshot.olderCursor
            messages = []
            toolCalls = []
            messageToolIDs = [:]
            deliveries = []
            submissionNotice = nil
            mutationError = nil
        }
        sessionID = snapshot.sessionID
        agent = snapshot.agent
        cursor = snapshot.cursor
        hookKind = snapshot.hookState?.kind
        var indices = Dictionary(messages.enumerated().map { ($0.element.id, $0.offset) }, uniquingKeysWith: { _, last in last })
        for message in snapshot.messages {
            if let index = indices[message.id] {
                messages[index] = message
            } else {
                indices[message.id] = messages.count
                messages.append(message)
            }
        }
        messageToolIDs.merge(snapshot.messageToolIDs, uniquingKeysWith: { _, next in next })
        mergeTools(snapshot.toolCalls, olderPage: false)
        boundHistory()
        deliveries = snapshot.deliveries
        reason = snapshot.reason
        isReady = snapshot.state == "ready"
        isLoading = false
        errorMessage = mutationError
    }

    /// Older pages preserve the live cursor, except an explicit backend reset-fence response.
    func loadOlder() async {
        guard canView, !isLoadingOlder, !isReturningLatest, let before = olderCursor else { return }
        isLoadingOlder = true
        let wasBrowsing = isBrowsingOlder
        if !isBrowsingOlder { historyEpoch &+= 1 }
        isBrowsingOlder = true
        defer { isLoadingOlder = false }
        let epoch = historyEpoch
        let requestedSession = sessionID
        do {
            let page = try await client.snapshot(terminalID, nil, before: before)
            guard !Task.isCancelled, canView, epoch == historyEpoch, requestedSession == sessionID else { return }
            guard page.terminalID == terminalID, page.reset || page.sessionID == sessionID else {
                await returnToLatest()
                return
            }
            // A stale before crossing the backend reset fence is a complete recent-window replacement.
            if page.reset { apply(page) } else { applyOlder(page) }
            errorMessage = mutationError
        } catch is CancellationError {
            return
        } catch {
            guard epoch == historyEpoch else { return }
            isBrowsingOlder = wasBrowsing
            errorMessage = String(localized: "Older messages unavailable. Retry or use the terminal.")
            HostLog.ui.error("agent_chat older page failed")
        }
    }

    func applyOlder(_ page: AgentChatSnapshot) {
        guard canView, page.terminalID == terminalID, page.sessionID == sessionID, !page.reset else { return }
        isBrowsingOlder = true
        let existing = Set(messages.map(\.id))
        var seen = existing
        let older = page.messages.filter { seen.insert($0.id).inserted }
        messages = older + messages
        messageToolIDs.merge(page.messageToolIDs, uniquingKeysWith: { current, _ in current })
        mergeTools(page.toolCalls, olderPage: true)
        olderCursor = page.olderCursor
        boundHistory(keepOlder: true)
    }

    private func mergeTools(_ incoming: [AgentChatToolCall], olderPage: Bool) {
        var indices = Dictionary(toolCalls.enumerated().map { ($0.element.id, $0.offset) }, uniquingKeysWith: { _, last in last })
        for tool in incoming {
            if let index = indices[tool.id] {
                let current = toolCalls[index]
                if (tool.seq ?? 0) > (current.seq ?? 0) || (!olderPage && (tool.seq ?? 0) == (current.seq ?? 0)) {
                    toolCalls[index] = tool
                }
            } else {
                indices[tool.id] = toolCalls.count
                toolCalls.append(tool)
            }
        }
    }

    /// Bound full message/tool units. A marker owns its tool even when the latest tool seq is newer.
    private func boundHistory(keepOlder: Bool = false) {
        let allTools = toolCalls
        let hasMarkers = !messageToolIDs.isEmpty
        // Include retained string and record metadata, not just visible message text.
        let messageSizes = Dictionary(messages.map { ($0.id, $0.text.utf8.count + $0.id.utf8.count + 64) }, uniquingKeysWith: { _, last in last })
        let toolSizes = Dictionary(allTools.map { ($0.id, $0.id.utf8.count + $0.title.utf8.count + $0.status.utf8.count + $0.detail.utf8.count + 64) }, uniquingKeysWith: { _, last in last })
        func reachableTools() -> [AgentChatToolCall] {
            if hasMarkers {
                let refs = Set(messages.compactMap { messageToolIDs[$0.id] })
                return allTools.filter { refs.contains($0.id) }
            }
            // Compatibility with older servers/fixtures without explicit tool marker metadata.
            let seqs = messages.compactMap(\.seq)
            if let first = seqs.min(), let last = seqs.max() {
                return allTools.filter { tool in tool.seq.map { $0 >= first && $0 <= last } ?? true }
            }
            return allTools
        }
        func retainedBytes() -> Int {
            messages.reduce(0) { $0 + (messageSizes[$1.id] ?? 0) } +
                toolCalls.reduce(0) { $0 + (toolSizes[$1.id] ?? 0) }
        }
        toolCalls = reachableTools()
        var evicted = false
        while messages.count > 1 && (messages.count > Self.maximumMessages || retainedBytes() > Self.maximumBytes) {
            if keepOlder { messages.removeLast() } else { messages.removeFirst() }
            evicted = true
            toolCalls = reachableTools()
        }
        // Legacy tool-only values lack marker ownership: bound them independently, with one full pair allowance.
        while toolCalls.count > 1 && (toolCalls.count > Self.maximumMessages || retainedBytes() > Self.maximumBytes) {
            if keepOlder { toolCalls.removeLast() } else { toolCalls.removeFirst() }
        }
        let retainedIDs = Set(messages.map(\.id))
        messageToolIDs = messageToolIDs.filter { retainedIDs.contains($0.key) }
        if evicted && !keepOlder { olderCursor = messages.first?.seq ?? olderCursor }
    }

    /// Resume polling from a fresh bounded recent window, not the older page's live cursor.
    func returnToLatest() async {
        guard canView, !isReturningLatest else { return }
        isReturningLatest = true
        historyEpoch &+= 1
        let epoch = historyEpoch
        defer { isReturningLatest = false }
        do {
            let snapshot = try await client.snapshot(terminalID)
            guard !Task.isCancelled, canView, epoch == historyEpoch, snapshot.terminalID == terminalID else { return }
            messages = []
            olderCursor = snapshot.olderCursor
            isBrowsingOlder = false
            sessionID = nil
            cursor = 0
            deliveries = []
            toolCalls = []
            messageToolIDs = [:]
            apply(snapshot)
            errorMessage = mutationError
        } catch is CancellationError {
            return
        } catch {
            errorMessage = String(localized: "Conversation unavailable. Retry or use the terminal.")
        }
    }

    /// Never retries POST. Only a user action can create a second delivery.
    func send() async {
        guard canSend else { return }
        let text = draft
        let sendingSession = sessionID
        isSubmitting = true
        mutationError = nil
        errorMessage = nil
        defer { isSubmitting = false }
        HostLog.ui.info("agent_chat send requested")
        do {
            try await client.send(terminalID, text)
            guard sessionID == sendingSession else { return }
            if draft == text { draft = "" }
            submissionNotice = String(localized: "Sent — awaiting transcript confirmation")
            await refresh()
        } catch {
            submissionNotice = String(localized: "Unconfirmed — not resent automatically")
            mutationError = String(localized: "Send could not be confirmed. Check the terminal before sending again.")
            errorMessage = mutationError
            HostLog.ui.error("agent_chat send unconfirmed; no retry")
        }
    }

    func control(_ action: AgentChatControl) async {
        guard canControl else { return }
        isControlling = true
        mutationError = nil
        errorMessage = nil
        defer { isControlling = false }
        HostLog.ui.info("agent_chat control requested: \(action.rawValue, privacy: .public)")
        do {
            try await client.control(terminalID, action)
            await refresh()
        } catch {
            mutationError = String(localized: "Control could not be confirmed. Use the terminal instead.")
            errorMessage = mutationError
            HostLog.ui.error("agent_chat control failed; no retry")
        }
    }
}
