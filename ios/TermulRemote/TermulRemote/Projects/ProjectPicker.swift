import SwiftUI

struct ProjectPicker: View {
    @Bindable var session: WorkspaceSession
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List(session.projects.projects.filter { !$0.isArchived }) { project in
                Button {
                    Task {
                        await session.selectProject(project)
                        dismiss()
                    }
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(project.name)
                                .foregroundStyle(.primary)
                            if let path = project.path {
                                Text(path)
                                    .font(.footnote)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }
                        }
                        Spacer()
                        if session.projects.active?.id == project.id {
                            Image(systemName: "checkmark")
                                .foregroundStyle(TermulTheme.accent)
                        }
                    }
                }
            }
            .navigationTitle(String(localized: "Projects"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .overlay {
                if session.projects.projects.isEmpty {
                    ContentUnavailableView(
                        "No projects",
                        systemImage: "square.stack",
                        description: Text("Open a project on the desktop, then reconnect.")
                    )
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}
