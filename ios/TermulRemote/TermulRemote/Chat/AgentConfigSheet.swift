import SwiftUI

struct AgentConfigSheet: View {
    @Bindable var session: WorkspaceSession

    var body: some View {
        NavigationStack {
            List {
                if let error = session.chat.errorMessage, !error.isEmpty {
                    Section {
                        Text(error)
                            .foregroundStyle(.red)
                    }
                }
                Section("ACP") {
                    ForEach(session.chat.catalogAgents) { agent in
                        Button {
                            Task { await session.chat.switchAgent(to: agent) }
                        } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(agent.name)
                                    Text(statusLabel(agent))
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                                if isCurrent(agent) {
                                    Image(systemName: "checkmark")
                                        .foregroundStyle(SeTheme.accent)
                                }
                            }
                        }
                        .disabled((!session.chat.canSelect(agent) && !isCurrent(agent)) || session.chat.isSwitchingAgent)
                    }
                }
                if let modes = session.chat.modes?.availableModes, !modes.isEmpty {
                    Section("Mode") {
                        ForEach(modes) { mode in
                            Button {
                                Task { await session.chat.setMode(mode.id) }
                            } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(mode.name)
                                        if let description = mode.description, !description.isEmpty {
                                            Text(description)
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                        }
                                    }
                                    Spacer()
                                    if mode.id == session.chat.modes?.currentModeId {
                                        Image(systemName: "checkmark")
                                            .foregroundStyle(SeTheme.accent)
                                    }
                                }
                            }
                        }
                    }
                }
                if let models = session.chat.models?.availableModels, !models.isEmpty {
                    Section("Model") {
                        ForEach(models) { model in
                            Button {
                                Task { await session.chat.setModel(model.modelId) }
                            } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(model.name)
                                        if let description = model.description, !description.isEmpty {
                                            Text(description)
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                        }
                                    }
                                    Spacer()
                                    if model.modelId == session.chat.models?.currentModelId {
                                        Image(systemName: "checkmark")
                                            .foregroundStyle(SeTheme.accent)
                                    }
                                }
                            }
                        }
                    }
                }
                ForEach(visibleConfigOptions) { option in
                    if let values = option.options, !values.isEmpty {
                        Section(option.name) {
                            ForEach(values) { value in
                                Button {
                                    Task { await session.chat.setConfig(optionId: option.id, valueId: value.value) }
                                } label: {
                                    HStack {
                                        VStack(alignment: .leading, spacing: 2) {
                                            Text(value.name)
                                            if let description = value.description, !description.isEmpty {
                                                Text(description)
                                                    .font(.caption)
                                                    .foregroundStyle(.secondary)
                                            }
                                        }
                                        Spacer()
                                        if value.value == option.currentValue {
                                            Image(systemName: "checkmark")
                                                .foregroundStyle(SeTheme.accent)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if !hasComposerControls {
                    Section {
                        Text("Mode, model, and thinking come from the host agent after the session is live. If they stay empty, keep this chat open on the Mac once so the agent can advertise them, then pull to refresh.")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .navigationTitle(session.chat.agentLabel)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { session.chat.showAgentSheet = false }
                }
            }
            .overlay {
                if session.chat.isSwitchingAgent {
                    VStack(spacing: 12) {
                        ProgressView()
                            .controlSize(.large)
                        Text(session.chat.switchingStatus ?? String(localized: "Starting the agent on the Mac. First launch can take a few minutes."))
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal, 24)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(.ultraThinMaterial)
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(Text(session.chat.switchingStatus ?? String(localized: "Starting the agent")))
                }
            }
            .task {
                await session.chat.refreshComposerControls()
            }
        }
    }

    private var hasComposerControls: Bool {
        !(session.chat.modes?.availableModes?.isEmpty ?? true)
            || !(session.chat.models?.availableModels?.isEmpty ?? true)
            || visibleConfigOptions.contains { !($0.options?.isEmpty ?? true) }
    }

    private var visibleConfigOptions: [SessionConfigOption] {
        session.chat.configOptions.filter { option in
            if option.category == "model", !(session.chat.models?.availableModels?.isEmpty ?? true) {
                return false
            }
            if option.category == "mode", !(session.chat.modes?.availableModes?.isEmpty ?? true) {
                return false
            }
            return !(option.options?.isEmpty ?? true)
        }
    }

    private func isCurrent(_ agent: CatalogAgent) -> Bool {
        AcpSpawnConfig.isCurrent(
            agent,
            activeAgentId: session.chat.activeAgentId,
            binding: session.chat.currentBinding
        )
    }

    private func statusLabel(_ agent: CatalogAgent) -> String {
        if isCurrent(agent) {
            return String(localized: "Current")
        }
        if AcpSpawnConfig.isLive(agent, liveAgentIds: session.chat.liveAgentIds) {
            return String(localized: "Running")
        }
        if session.chat.canSelect(agent) {
            return String(localized: "Ready")
        }
        switch agent.status {
        case "install-required":
            return String(localized: "Install on desktop")
        case "needs-runtime":
            return String(localized: "Needs a runtime")
        case "manual-install":
            return String(localized: "Manual install")
        default:
            return String(localized: "Unavailable")
        }
    }
}
