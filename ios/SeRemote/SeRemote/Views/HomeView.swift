import SwiftUI

/// The pre-connection home: the prototype's 夜间作业 page reduced to the data
/// this phone actually has — saved hosts and real open/connection timestamps.
@MainActor
struct HomeView: View {
    @Bindable var store: ConnectionStore
    @Bindable var settings: AppSettings

    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.sePalette) private var palette
    @Environment(\.seThemeMode) private var mode

    @State private var showPairing = false
    @State private var showSettings = false
    @State private var pendingLink: RemoteLink?
    @State private var renamingLink: RemoteLink?
    @State private var renameTitle = ""
    @State private var deletingLink: RemoteLink?
    @State private var actionLink: RemoteLink?
    @State private var activity = ShellPairingActivity()

    private var recentHost: RemoteLink? {
        var best: (link: RemoteLink, date: Date)?
        for link in store.savedLinks {
            let date = activityDate(for: link)
            if best == nil || date > best!.date {
                best = (link, date)
            }
        }
        return best?.link
    }

    private func activityDate(for link: RemoteLink) -> Date {
        if let connected = activity.connectedDate(for: link) { return connected }
        if let opened = activity.date(for: link) { return opened }
        return link.createdAt
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            ScrollView(.vertical, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    pageTop
                    if let recentHost {
                        resumeRow(recentHost)
                            .padding(.horizontal, 18)
                            .padding(.top, 12)
                    }

                    SeGroupLabel(title: "最近主机", count: String(format: "%02d", store.savedLinks.count))

                    if store.savedLinks.isEmpty {
                        emptyState
                            .padding(.horizontal, 18)
                    } else {
                        VStack(spacing: 0) {
                            ForEach(Array(store.savedLinks.enumerated()), id: \.element.id) { index, link in
                                SeHostRowView(
                                    index: String(format: "%02d", index + 1),
                                    name: link.title,
                                    meta: meta(for: link),
                                    latency: nil,
                                    online: store.activeLink?.id == link.id,
                                    onTap: { open(link) },
                                    onMore: { actionLink = link }
                                )
                                .contextMenu {
                                    Button("打开") { open(link) }
                                    Button("重命名") { beginRename(link) }
                                    Button("解除", role: .destructive) { deletingLink = link }
                                }
                            }
                        }
                        .padding(.horizontal, 18)
                    }

                    SeLinkRow(title: "用配对链接添加主机") {
                        store.dismissError()
                        showPairing = true
                    }
                    .padding(.horizontal, 18)
                    .padding(.top, 4)

                    if let error = store.errorMessage {
                        SeErrorBanner(message: error)
                            .padding(.horizontal, 18)
                            .padding(.top, 8)
                    }

                    Text("配对需要在你的主机上确认。屏幕之外，没有别的入口。")
                        .font(SeNocturneType.font(10))
                        .foregroundStyle(palette.inkTertiary)
                        .padding(.horizontal, 18)
                        .padding(.top, 14)
                        .padding(.bottom, 120)
                }
                .frame(maxWidth: 680)
                .frame(maxWidth: .infinity)
            }

            dock
        }
        .background(palette.canvas.ignoresSafeArea())
        .foregroundStyle(palette.ink)
        .onAppear { activity.reload() }
        .sheet(isPresented: $showPairing, onDismiss: connectPendingLink) {
            ShellPairingView { link in
                pendingLink = link
                showPairing = false
            }
            .presentationDetents([.large])
            .presentationDragIndicator(.hidden)
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(settings: settings, store: store)
        }
        .alert("重命名", isPresented: Binding(
            get: { renamingLink != nil },
            set: { if !$0 { renamingLink = nil } }
        ), presenting: renamingLink) { link in
            TextField("名称", text: $renameTitle)
            Button("取消", role: .cancel) { renamingLink = nil }
            Button("保存") {
                store.rename(link, title: renameTitle)
                renamingLink = nil
            }
            .disabled(renameTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
        .confirmationDialog("主机操作", isPresented: Binding(
            get: { actionLink != nil },
            set: { if !$0 { actionLink = nil } }
        ), titleVisibility: .visible, presenting: actionLink) { link in
            Button("打开") { actionLink = nil; open(link) }
            Button("重命名") { actionLink = nil; beginRename(link) }
            Button("解除", role: .destructive) { actionLink = nil; deletingLink = link }
            Button("取消", role: .cancel) { actionLink = nil }
        }
        .confirmationDialog("解除这台主机？", isPresented: Binding(
            get: { deletingLink != nil },
            set: { if !$0 { deletingLink = nil } }
        ), titleVisibility: .visible, presenting: deletingLink) { link in
            Button("解除", role: .destructive) {
                activity.forget(link)
                store.forget(link)
                deletingLink = nil
            }
            Button("取消", role: .cancel) { deletingLink = nil }
        } message: { _ in
            Text("这会从本机删除主机和已保存的访问密钥。不会改变你的桌面。")
        }
        .tint(palette.accent)
    }

    // MARK: - Page

    private var pageTop: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                SeEyebrow(text: today)
                Spacer(minLength: 0)
                SeIconButton(icon: .refresh, accessibilityLabel: "刷新", size: 34) {
                    activity.reload()
                }
                SeIconButton(icon: mode == .night ? .moon : .sun, accessibilityLabel: "切换主题", size: 34) {
                    settings.appearance = mode.toggled.appearance
                }
                SeIconButton(icon: .gear, accessibilityLabel: "设置", size: 34) {
                    showSettings = true
                }
            }
            SeDisplayText(text: "夜间作业")
                .padding(.top, 4)
            lede
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
    }

    private var lede: some View {
        HStack(spacing: 4) {
            Text("\(store.savedLinks.count) 台").fontWeight(.semibold).foregroundStyle(palette.ink)
            Text("主机").foregroundStyle(palette.inkSecondary)
            Text("·").foregroundStyle(palette.inkTertiary)
            Text(store.savedLinks.isEmpty ? "还没有连接" : "等你连接")
                .foregroundStyle(palette.inkSecondary)
        }
        .font(SeNocturneType.font(12))
    }

    private func resumeRow(_ link: RemoteLink) -> some View {
        SeResumeRow(
            title: link.title,
            meta: resumeMeta(link),
            actionTitle: "继续 →",
            onTap: { open(link) }
        )
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("还没有保存的主机")
                .font(SeNocturneType.font(13, weight: .semibold))
                .foregroundStyle(palette.ink)
            Text("在 Mac 上打开 SeRemote，扫描二维码或粘贴访问链接。")
                .font(SeNocturneType.font(11))
                .foregroundStyle(palette.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(mode.hairline, lineWidth: 1)
        }
    }

    private var dock: some View {
        HStack(spacing: 8) {
            SeSolidButton(title: "配对新主机", icon: .plus) {
                store.dismissError()
                showPairing = true
            }
            .accessibilityIdentifier("home.pair")
            SeOutlineButton(title: "扫码", icon: .scan, square: true) {
                store.dismissError()
                showPairing = true
            }
            SeOutlineButton(title: "粘贴链接", icon: .link, square: true) {
                store.dismissError()
                showPairing = true
            }
        }
        .padding(.horizontal, 18)
        .padding(.top, 20)
        .padding(.bottom, 28)
        .background(
            LinearGradient(
                stops: [
                    .init(color: palette.canvas.opacity(0), location: 0),
                    .init(color: palette.canvas.opacity(0.86), location: 0.45),
                    .init(color: palette.canvas, location: 0.72)
                ],
                startPoint: .top, endPoint: .bottom
            )
            .ignoresSafeArea(edges: .bottom)
        )
    }

    // MARK: - Data

    private var today: String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_Hans_CN")
        formatter.dateFormat = "EEEE · M 月 d 日"
        return formatter.string(from: .now)
    }

    private func meta(for link: RemoteLink) -> String {
        if let date = activity.connectedDate(for: link) {
            return "上次连接 " + relative(date)
        }
        if let date = activity.date(for: link) {
            return "上次打开 " + relative(date)
        }
        return "保存于 " + link.createdAt.formatted(date: .numeric, time: .omitted)
    }

    private func resumeMeta(_ link: RemoteLink) -> String {
        if let date = activity.connectedDate(for: link) { return "上次连接 · " + relative(date) }
        if let date = activity.date(for: link) { return "上次打开 · " + relative(date) }
        return "已保存"
    }

    private func relative(_ date: Date) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.locale = Locale(identifier: "zh_Hans_CN")
        formatter.unitsStyle = .short
        return formatter.localizedString(for: date, relativeTo: .now)
    }

    // MARK: - Actions

    private func beginRename(_ link: RemoteLink) {
        renameTitle = link.title
        renamingLink = link
    }

    private func open(_ link: RemoteLink) {
        activity.recordOpen(link)
        store.connect(link: link)
    }

    private func connectPendingLink() {
        guard let link = pendingLink else { return }
        pendingLink = nil
        open(link)
    }
}
