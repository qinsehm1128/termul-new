import SwiftUI

struct AgentConfigSheet: View {
    @Bindable var session: WorkspaceSession
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        NavigationStack {
            List {
                if let error = session.chat.errorMessage, !error.isEmpty {
                    Section {
                        SeErrorBanner(message: error)
                    }
                    .listRowBackground(SeTheme.canvas)
                }
                Section {
                    ForEach(session.chat.catalogAgents) { agent in
                        choice(
                            title: agent.name,
                            detail: statusLabel(agent),
                            isSelected: isCurrent(agent),
                            isEnabled: (session.chat.canSelect(agent) || isCurrent(agent)) && !session.chat.isSwitchingAgent
                        ) {
                            Task { await session.chat.switchAgent(to: agent) }
                        }
                    }
                } header: {
                    SeSectionHeader(title: String(localized: "ACP"))
                }
                if let modes = session.chat.modes?.availableModes, !modes.isEmpty {
                    Section {
                        ForEach(modes) { mode in
                            choice(
                                title: mode.name,
                                detail: mode.description,
                                isSelected: mode.id == session.chat.modes?.currentModeId
                            ) {
                                Task { await session.chat.setMode(mode.id) }
                            }
                        }
                    } header: {
                        SeSectionHeader(title: String(localized: "Mode"))
                    }
                }
                if let models = session.chat.models?.availableModels, !models.isEmpty {
                    Section {
                        ForEach(models) { model in
                            choice(
                                title: model.name,
                                detail: model.description,
                                isSelected: model.modelId == session.chat.models?.currentModelId
                            ) {
                                Task { await session.chat.setModel(model.modelId) }
                            }
                        }
                    } header: {
                        SeSectionHeader(title: String(localized: "Model"))
                    }
                }
                ForEach(visibleConfigOptions) { option in
                    if let values = option.options, !values.isEmpty {
                        Section {
                            ForEach(values) { value in
                                choice(
                                    title: value.name,
                                    detail: value.description,
                                    isSelected: value.value == option.currentValue
                                ) {
                                    Task { await session.chat.setConfig(optionId: option.id, valueId: value.value) }
                                }
                            }
                        } header: {
                            SeSectionHeader(title: option.name)
                        }
                    }
                }
                if !hasComposerControls {
                    Section {
                        HStack(spacing: 4) {
                            Text("Options come from the host agent.")
                                .font(SeFont.support)
                                .foregroundStyle(SeTheme.secondaryInk)
                            SeInfoTip(
                                text: String(localized: "If they stay empty, keep this chat open on the Mac once so the agent can advertise them, then pull to refresh."),
                                size: 24
                            )
                            Spacer(minLength: 0)
                        }
                        .listRowBackground(SeTheme.surface)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .scrollContentBackground(.hidden)
            .background(SeTheme.canvas)
            .tint(SeTheme.accent)
            .navigationTitle(session.chat.agentLabel)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { session.chat.showAgentSheet = false }
                        .frame(minWidth: SeSpace.hitTarget, minHeight: SeSpace.hitTarget)
                }
            }
            .overlay {
                if session.chat.isSwitchingAgent {
                    GeometryReader { geometry in
                        ScrollView {
                            VStack(spacing: SeSpace.sm) {
                                ProgressView()
                                    .controlSize(.large)
                                    .tint(SeTheme.live)
                                Text(session.chat.switchingStatus ?? String(localized: "Starting the agent on the Mac. First launch can take a few minutes."))
                                    .font(SeFont.support)
                                    .foregroundStyle(SeTheme.secondaryInk)
                                    .multilineTextAlignment(.center)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            .padding(SeSpace.xl)
                            .frame(maxWidth: .infinity, minHeight: geometry.size.height)
                        }
                        .background(SeTheme.canvas)
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(Text(session.chat.switchingStatus ?? String(localized: "Starting the agent")))
                }
            }
            .task {
                await session.chat.refreshComposerControls()
            }
        }
    }

    private func choice(
        title: String,
        detail: String?,
        isSelected: Bool,
        isEnabled: Bool = true,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: SeSpace.xs) {
                // Keep names multiline; only the short selection badge is a chip.
                // Large text moves the badge below rather than squeezing the name.
                let layout = dynamicTypeSize.isAccessibilitySize
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: SeSpace.xs))
                    : AnyLayout(HStackLayout(alignment: .top, spacing: SeSpace.sm))
                layout {
                    Text(title)
                        .font(SeFont.rowTitle)
                        .foregroundStyle(SeTheme.ink)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .fixedSize(horizontal: false, vertical: true)
                    if isSelected {
                        SeChip(title: String(localized: "Selected"), systemImage: "checkmark", isSelected: true)
                            .fixedSize()
                    }
                }
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(SeFont.support)
                        .foregroundStyle(SeTheme.secondaryInk)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.vertical, SeSpace.xs)
            .frame(maxWidth: .infinity, minHeight: SeSpace.hitTarget, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(SePressableRowStyle())
        .disabled(!isEnabled)
        .opacity(isEnabled ? 1 : 0.5)
        .listRowBackground(SeTheme.surface)
        .listRowSeparatorTint(SeTheme.stroke)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(title))
        .accessibilityValue(Text([detail, isSelected ? String(localized: "Selected") : nil].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: ", ")))
        .accessibilityAddTraits(isSelected ? .isSelected : [])
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
