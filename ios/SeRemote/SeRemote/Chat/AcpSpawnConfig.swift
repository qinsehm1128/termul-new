import Foundation

struct CatalogHost: Decodable, Sendable {
    var os: String?
    var arch: String?
    var runtimes: CatalogRuntimes?
}

struct CatalogRuntimes: Decodable, Sendable {
    var npx: Bool?
    var uvx: Bool?
}

struct AcpLaunchConfig: Sendable {
    var configId: String
    var name: String
    var command: String
    var args: [String]
    var env: [String: String]
}

enum AcpSpawnConfig {
    static func registryConfigId(_ agentId: String) -> String {
        "acp-registry:\(agentId)"
    }

    static func matches(agentId: String, catalogId: String) -> Bool {
        agentId == catalogId
            || agentId.hasPrefix("\(catalogId)-")
            || agentId.hasPrefix("acp-registry:\(catalogId)")
    }

    /// Host namespaces look like `config:acp-registry:cursor` or `config:cursor`.
    static func catalogId(fromNamespace namespace: String?) -> String? {
        guard let raw = namespace?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else {
            return nil
        }
        let stripped = raw.hasPrefix("config:") ? String(raw.dropFirst("config:".count)) : raw
        let catalog = stripped.hasPrefix("acp-registry:")
            ? String(stripped.dropFirst("acp-registry:".count))
            : stripped
        return catalog.isEmpty ? nil : catalog
    }

    static func matchesBinding(_ agent: CatalogAgent, binding: AgentSessionBinding) -> Bool {
        if let running = agent.runningAgentId, running == binding.runtimeAgentId {
            return true
        }
        if matches(agentId: binding.runtimeAgentId, catalogId: agent.id) {
            return true
        }
        if let catalogId = catalogId(fromNamespace: binding.stableAgentNamespace) {
            return catalogId == agent.id
        }
        return false
    }

    static func isLive(_ agent: CatalogAgent, liveAgentIds: [String]) -> Bool {
        if let running = agent.runningAgentId, !running.isEmpty {
            return true
        }
        return liveAgentIds.contains { matches(agentId: $0, catalogId: agent.id) }
    }

    /// Prefer a process that is still running. Bindings keep the spawn UUID from
    /// the process that first created the session; after a Mac restart that UUID
    /// is gone and `runningAgentId` / catalog overlay is the live handle.
    static func resolveLiveAgentId(
        preferred: String?,
        binding: AgentSessionBinding?,
        catalog: [CatalogAgent],
        liveAgentIds: [String]
    ) -> String? {
        if let preferred, liveAgentIds.contains(preferred) {
            return preferred
        }
        if let runtime = binding?.runtimeAgentId, liveAgentIds.contains(runtime) {
            return runtime
        }
        if let binding, let agent = catalog.first(where: { matchesBinding($0, binding: binding) }) {
            if let running = agent.runningAgentId, !running.isEmpty {
                return running
            }
            if let match = liveAgentIds.first(where: { matches(agentId: $0, catalogId: agent.id) }) {
                return match
            }
        }
        if let preferred, let agent = catalog.first(where: {
            isCurrent($0, activeAgentId: preferred, binding: binding)
        }) {
            if let running = agent.runningAgentId, !running.isEmpty {
                return running
            }
            if let match = liveAgentIds.first(where: { matches(agentId: $0, catalogId: agent.id) }) {
                return match
            }
        }
        return nil
    }

    static func isCurrent(
        _ agent: CatalogAgent,
        activeAgentId: String?,
        binding: AgentSessionBinding? = nil
    ) -> Bool {
        if let running = agent.runningAgentId, let activeAgentId, running == activeAgentId {
            return true
        }
        if let activeAgentId, !activeAgentId.isEmpty, matches(agentId: activeAgentId, catalogId: agent.id) {
            return true
        }
        if let binding {
            return matchesBinding(agent, binding: binding)
        }
        return false
    }

    static func canSelect(_ agent: CatalogAgent, liveAgentIds: [String], host: CatalogHost?) -> Bool {
        if isLive(agent, liveAgentIds: liveAgentIds) {
            return true
        }
        if let installed = agent.installed, !installed.command.isEmpty {
            return true
        }
        guard agent.status == "ready" else { return false }
        return derive(agent, host: host) != nil
    }

    static func derive(_ agent: CatalogAgent, host: CatalogHost?) -> AcpLaunchConfig? {
        if let installed = agent.installed, !installed.command.isEmpty {
            return AcpLaunchConfig(
                configId: registryConfigId(agent.id),
                name: agent.name,
                command: installed.command,
                args: installed.args ?? [],
                env: [:]
            )
        }
        return deriveDistribution(agent, host: host)
    }

    static func platformArch(host: CatalogHost?) -> String {
        let os = host?.os == "macos" ? "darwin" : (host?.os ?? "darwin")
        let arch = host?.arch ?? "aarch64"
        return "\(os)-\(arch)"
    }

    private static func deriveDistribution(_ agent: CatalogAgent, host: CatalogHost?) -> AcpLaunchConfig? {
        guard let distribution = agent.distribution?.object else { return nil }

        if let npx = distribution["npx"]?.object, let package = npx["package"]?.string, isSafePackage(package) {
            return AcpLaunchConfig(
                configId: registryConfigId(agent.id),
                name: agent.name,
                command: "npx",
                args: ["-y", package] + (npx["args"]?.stringArray ?? []),
                env: npx["env"]?.stringMap ?? [:]
            )
        }

        if let uvx = distribution["uvx"]?.object, let package = uvx["package"]?.string, isSafePackage(package) {
            return AcpLaunchConfig(
                configId: registryConfigId(agent.id),
                name: agent.name,
                command: "uvx",
                args: [package] + (uvx["args"]?.stringArray ?? []),
                env: uvx["env"]?.stringMap ?? [:]
            )
        }

        if let binary = distribution["binary"]?.object,
           let target = binary[platformArch(host: host)]?.object,
           let command = launchCommand(from: target["cmd"]?.string) {
            return AcpLaunchConfig(
                configId: registryConfigId(agent.id),
                name: agent.name,
                command: command,
                args: target["args"]?.stringArray ?? [],
                env: target["env"]?.stringMap ?? [:]
            )
        }
        return nil
    }

    /// Prefer a PATH-safe name. Relative archive cmds such as
    /// `./dist-package/cursor-agent` become `cursor-agent`.
    static func launchCommand(from raw: String?) -> String? {
        guard let raw, !raw.isEmpty else { return nil }
        if !raw.hasPrefix("./"), !raw.hasPrefix(".\\"), !raw.contains("/"), !raw.contains("\\") {
            return raw
        }
        let basename = raw.split { $0 == "/" || $0 == "\\" }.last.map(String.init) ?? raw
        guard !basename.isEmpty, !basename.hasPrefix(".") else { return nil }
        return basename
    }

    private static func isSafePackage(_ package: String) -> Bool {
        !package.isEmpty && !package.hasPrefix("-")
    }
}
