import Foundation

@MainActor
protocol QuickTerminalServing {
    func list() async throws -> [QuickTerminalRecordWire]
    func create(_ body: CreateQuickTerminalBody) async throws -> QuickTerminalRecordWire
    func open(_ body: OpenQuickTerminalBody) async throws -> QuickTerminalOpenedWire
    func createTerminal(_ body: CreateQuickTerminalTerminalBody) async throws -> QuickTerminalOpenedWire
    func closeTerminal(_ body: CloseQuickTerminalTerminalBody) async throws -> QuickTerminalRecordWire
    func rename(_ body: QuickTerminalRenameBody) async throws -> QuickTerminalRecordWire
    func delete(_ body: QuickTerminalIdBody) async throws
    func close(_ body: QuickTerminalIdBody) async throws -> QuickTerminalRecordWire
}

@MainActor
struct QuickTerminalClient: QuickTerminalServing {
    let http: HostHTTP
    private let acknowledgementSession: URLSession

    init(http: HostHTTP, acknowledgementSession: URLSession = HostURLSession.shared) {
        self.http = http
        self.acknowledgementSession = acknowledgementSession
    }

    func list() async throws -> [QuickTerminalRecordWire] {
        try await http.get("quick-terminals")
    }

    func create(_ body: CreateQuickTerminalBody) async throws -> QuickTerminalRecordWire {
        try await http.post("quick-terminals", body: QuickTerminalRequestJSON.dictionary(body))
    }

    func open(_ body: OpenQuickTerminalBody) async throws -> QuickTerminalOpenedWire {
        try await http.post("quick-terminals/open", body: QuickTerminalRequestJSON.dictionary(body))
    }

    func createTerminal(_ body: CreateQuickTerminalTerminalBody) async throws -> QuickTerminalOpenedWire {
        try await http.post("quick-terminals/create-terminal", body: QuickTerminalRequestJSON.dictionary(body))
    }

    func closeTerminal(_ body: CloseQuickTerminalTerminalBody) async throws -> QuickTerminalRecordWire {
        try await http.post("quick-terminals/close-terminal", body: QuickTerminalRequestJSON.dictionary(body))
    }

    func rename(_ body: QuickTerminalRenameBody) async throws -> QuickTerminalRecordWire {
        try await http.post("quick-terminals/rename", body: QuickTerminalRequestJSON.dictionary(body))
    }

    func close(_ body: QuickTerminalIdBody) async throws -> QuickTerminalRecordWire {
        try await http.post("quick-terminals/close", body: QuickTerminalRequestJSON.dictionary(body))
    }

    func delete(_ body: QuickTerminalIdBody) async throws {
        // HostHTTP.post unwraps data and rejects success:true,data:null. Only this
        // acknowledgement uses a local POST transport; auth/trust remain shared.
        do {
            let request = try Self.deleteRequest(http: http, body: body)
            let (data, response) = try await acknowledgementSession.data(for: request)
            try Self.validateDeleteResponse(data: data, response: response)
        } catch let error as HostError {
            throw error
        } catch let error as DecodingError {
            throw HostError.unexpected(error.localizedDescription)
        } catch {
            throw HostError.network(error.localizedDescription)
        }
    }

    static func deleteRequest(http: HostHTTP, body: QuickTerminalIdBody) throws -> URLRequest {
        let base = http.origin.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: base + "/quick-terminals/delete") else {
            throw HostError.unexpected("Invalid quick terminal delete URL")
        }
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.httpMethod = "POST"
        request.assumesHTTP3Capable = false
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        http.credentials.apply(to: &request)
        request.httpBody = try JSONEncoder().encode(body)
        return request
    }

    static func validateDeleteResponse(data: Data, response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse else {
            throw HostError.network(String(localized: "The tunnel returned an unexpected response."))
        }
        if http.statusCode == 401 || http.statusCode == 403 {
            throw HostError.network(String(localized: "The access token was rejected. Scan the desktop QR again."))
        }
        guard (200 ..< 300).contains(http.statusCode) else {
            throw HostError.network("HTTP \(http.statusCode)")
        }
        let result = try JSONDecoder().decode(QuickTerminalAcknowledgement.self, from: data)
        guard result.success else {
            throw HostError.ipc(result.error ?? String(localized: "The host rejected the request."), result.code ?? "NETWORK_ERROR")
        }
    }
}

/// Unlike IpcResult.unwrap, an acknowledgement intentionally does not require data.
private nonisolated struct QuickTerminalAcknowledgement: Decodable {
    let success: Bool
    let error: String?
    let code: String?
}
