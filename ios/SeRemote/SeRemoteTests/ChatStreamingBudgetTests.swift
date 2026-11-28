import Foundation
import Testing
@testable import SeRemote

/// A streaming reply must not cost one full transcript mutation (and one
/// Markdown parse) per token: chunks are applied once per frame budget, and a
/// transcript that outgrew its budget is trimmed when a turn settles.
@Suite("Chat streaming budget")
@MainActor
struct ChatStreamingBudgetTests {
    private func chunk(_ text: String, role: String = "agent") throws -> Data {
        try JSONSerialization.data(withJSONObject: [
            "role": role,
            "content": ["type": "text", "text": text],
        ])
    }

    private func connectedChat() -> (ChatStore, AcpSocket) {
        let chat = ChatStore()
        let socket = AcpSocket()
        chat.attach(socket: socket)
        return (chat, socket)
    }

    @Test("A token burst applies as one ordered mutation")
    func coalescesTokenBurst() throws {
        let (chat, socket) = connectedChat()
        socket.onEvent?("message_chunk", "s1", 1, try chunk("Hel"))
        socket.onEvent?("message_chunk", "s1", 2, try chunk("lo"))
        socket.onEvent?("message_chunk", "s1", 3, try chunk("!", role: "thought"))

        #expect(chat.messages.isEmpty, "nothing reaches the transcript inside the frame budget")

        chat.flushPendingChunks()

        #expect(chat.messages.count == 2)
        #expect(chat.messages.first?.text == "Hello")
        #expect(chat.messages.first?.seq == 2, "the segment keeps the newest host cursor")
        #expect(chat.messages.last?.role == .thought)
        #expect(chat.messages.last?.text == "!")

        chat.flushPendingChunks()
        #expect(chat.messages.count == 2, "flushing twice never duplicates text")
        #expect(chat.messages.map(\.text) == ["Hello", "!"])
    }

    @Test("A tool call flushes buffered text before it")
    func eventBoundaryFlushesText() throws {
        let (chat, socket) = connectedChat()
        socket.onEvent?("message_chunk", "s1", 1, try chunk("Reading"))
        let tool = try JSONSerialization.data(withJSONObject: [
            "toolCall": ["toolCallId": "t1", "title": "read", "status": "pending"],
        ])
        socket.onEvent?("tool_call", "s1", 2, tool)

        #expect(chat.messages.map(\.text) == ["Reading"])
        #expect(chat.tools.map(\.id) == ["t1"])
    }

    @Test("The working flag flips once per turn, not per token")
    func streamingFlagIsCoarse() throws {
        let (chat, socket) = connectedChat()
        #expect(!chat.isAgentWorking)

        socket.onEvent?("message_chunk", "s1", 1, try chunk("Hel"))
        #expect(!chat.isStreaming, "buffered text is not a transcript change yet")

        chat.flushPendingChunks()
        #expect(chat.isStreaming)
        #expect(chat.isAgentWorking)

        socket.onEvent?("prompt_complete", "s1", 2, Data("{}".utf8))
        #expect(!chat.isStreaming)
        #expect(!chat.isAgentWorking)
    }

    @Test("A settled turn bounds a transcript that outgrew the message budget")
    func settledTurnBoundsTranscript() throws {
        let (chat, socket) = connectedChat()
        let overflow = 120
        chat.messages = (0..<(ChatStore.maximumTranscriptMessages + overflow)).map { index in
            ChatMessage(
                id: "m\(index)",
                role: .agent,
                text: String(repeating: "x", count: 120) + "-\(index)",
                streaming: false,
                seq: UInt64(index + 1)
            )
        }
        socket.onEvent?("prompt_complete", "s1", 100_000, Data("{}".utf8))

        #expect(chat.messages.count == ChatStore.maximumTranscriptMessages)
        #expect(chat.messages.first?.id == "m\(overflow)")
        #expect(chat.messages.last?.id == "m\(ChatStore.maximumTranscriptMessages + overflow - 1)")
    }

    @Test("A byte budget drops old turns even under the count limit")
    func byteBudgetDropsOldTurns() throws {
        let (chat, socket) = connectedChat()
        let perMessage = ChatStore.maximumTranscriptBytes / 8
        chat.messages = (0..<10).map { index in
            ChatMessage(
                id: "m\(index)",
                role: .agent,
                text: String(repeating: "y", count: perMessage) + "-\(index)",
                streaming: false,
                seq: UInt64(index + 1)
            )
        }
        socket.onEvent?("prompt_complete", "s1", 100_000, Data("{}".utf8))

        #expect(chat.messages.count < 10, "10 x 1 MiB exceeds the byte budget")
        #expect(chat.messages.last?.id == "m9", "the newest turn always survives")
    }
}
