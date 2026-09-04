import SwiftUI

enum NotificationHookClient: String, Identifiable, CaseIterable {
    case claude, codex, grok

    var id: String { rawValue }

    var title: String {
        switch self {
        case .claude: "Claude Code"
        case .codex: "Codex CLI"
        case .grok: "Grok Build"
        }
    }

    var systemImage: String {
        switch self {
        case .claude: "terminal"
        case .codex: "terminal.fill"
        case .grok: "terminal"
        }
    }
}

struct NotificationsPane: View {
    @ObservedObject var model: AppModel
    @State private var clientPendingRemoval: NotificationHookClient?
    @State private var clientsExpanded = true

    var body: some View {
        SettingsPage(title: SettingsSection.notifications.title, subtitle: SettingsSection.notifications.subtitle) {
            authorizationPanel
            hookPanel
            categoryPanel
            soundPanel
        }
        .confirmationDialog(
            clientPendingRemoval.map { "移除 \($0.title) 通知配置？" } ?? "移除通知配置？",
            isPresented: Binding(
                get: { clientPendingRemoval != nil },
                set: { if !$0 { clientPendingRemoval = nil } }
            )
        ) {
            Button("移除配置", role: .destructive) {
                if let client = clientPendingRemoval {
                    model.setNotificationClient(client, enabled: false)
                }
                clientPendingRemoval = nil
            }
            Button("取消", role: .cancel) { clientPendingRemoval = nil }
        } message: {
            Text("只删除 Sumpter 写入的通知 Hook，其它自定义 Hook 会保留。")
        }
    }

    private var authorizationPanel: some View {
        SectionPanel(title: "系统授权", hint: "macOS 只会在首次请求时弹窗；如果已拒绝，需要去系统设置里手动开启。") {
            VStack(alignment: .leading, spacing: 12) {
                SumpterWrappingLayout(horizontalSpacing: 8, verticalSpacing: 8) {
                    StatusBadge(
                        text: authorizationTitle,
                        systemImage: authorizationSystemImage,
                        color: authorizationColor
                    )
                    Button {
                        model.refreshNotificationAuthorizationStatus()
                    } label: {
                        Label("刷新", systemImage: "arrow.clockwise")
                    }
                    Button {
                        model.requestNotificationAuthorization()
                    } label: {
                        Label("请求授权", systemImage: "bell.badge")
                    }
                    .disabled(model.notificationAuthorizationStatus == .authorized || model.notificationAuthorizationStatus == .provisional)
                    if model.notificationAuthorizationStatus == .denied {
                        Button {
                            model.openNotificationSettings()
                        } label: {
                            Label("打开系统设置", systemImage: "gear")
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if let error = model.notificationError, !error.isEmpty {
                    Text(error)
                        .font(.caption)
                        .foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("如果状态已允许但后台仍不弹，请检查系统专注模式、通知样式和Sumpter的横幅权限。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    /// Claude/Codex/Grok 任一接入已配置即视为统一通知已启用。
    private var hookEnabled: Bool {
        model.notificationsEnabled
    }

    private var hookPanel: some View {
        SectionPanel(title: "通知接入", hint: "一套开关同时管理 Claude Code、Codex CLI 与 Grok Build。协议事件名不同，但都会归入下面同一组通知类别；Sumpter 只转发客户端已经产生的事件。") {
            VStack(alignment: .leading, spacing: 12) {
                SumpterWrappingLayout(horizontalSpacing: 10, verticalSpacing: 8) {
                    Toggle("启用 Claude Code / Codex CLI / Grok Build 通知", isOn: Binding(
                        get: { hookEnabled },
                        set: { model.setNotifications(enabled: $0) }
                    ))
                    StatusBadge(
                        text: hookEnabled ? "已启用" : "未启用",
                        systemImage: hookEnabled ? "bell.fill" : "bell.slash",
                        color: hookEnabled ? .green : .secondary
                    )
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                DisclosureGroup("各客户端接入状态", isExpanded: $clientsExpanded) {
                    VStack(alignment: .leading, spacing: 14) {
                        clientInstallRow(
                            .claude,
                            configured: !model.claudeNotificationArguments.isEmpty,
                            badgeText: model.claudeNotificationArguments.isEmpty ? "未配置" : "已配置",
                            badgeImage: model.claudeNotificationArguments.isEmpty ? "minus.circle" : "checkmark.circle",
                            badgeColor: model.claudeNotificationArguments.isEmpty ? .secondary : .green,
                            detail: "写入 ~/.claude/settings.json 与 sumpter-notify.sh。"
                        )
                        Divider()
                        clientInstallRow(
                            .codex,
                            configured: !model.codexNotificationArguments.isEmpty,
                            badgeText: model.codexNotificationHookStatus.title,
                            badgeImage: codexStatusImage,
                            badgeColor: codexStatusColor,
                            detail: "Codex 配置文件：\(model.codexNotificationHookPath)"
                        )
                        switch model.codexNotificationHookStatus {
                        case .pendingTrust:
                            Text("首次使用请在 Codex CLI 输入 /hooks，信任 Sumpter 的通知 Hook；收到一次真实通知后会显示“已验证”。")
                                .font(.caption)
                                .foregroundStyle(.orange)
                                .fixedSize(horizontal: false, vertical: true)
                        case .legacyConflict:
                            Text("检测到无法安全判断的 legacy notify。为避免误删，请先在 config.toml 中手动处理后再启用。")
                                .font(.caption)
                                .foregroundStyle(.red)
                                .fixedSize(horizontal: false, vertical: true)
                        case .writeFailed:
                            Text("Codex 写入失败；请检查 CODEX_HOME、文件权限和 hooks.json 格式。")
                                .font(.caption)
                                .foregroundStyle(.red)
                                .fixedSize(horizontal: false, vertical: true)
                        case .notConfigured, .verified:
                            EmptyView()
                        }
                        Divider()
                        clientInstallRow(
                            .grok,
                            configured: model.grokNotificationsEnabled,
                            badgeText: model.grokNotificationsEnabled ? "已配置" : "未配置",
                            badgeImage: model.grokNotificationsEnabled ? "checkmark.circle" : "minus.circle",
                            badgeColor: model.grokNotificationsEnabled ? .green : .secondary,
                            detail: "Grok 配置文件：\(model.grokNotificationHookPath)"
                        )
                        Text("Grok 全局 hook 默认受信任，无需再跑 /hooks。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.top, 6)
                }
            }
        }
    }

    private func clientInstallRow(
        _ client: NotificationHookClient,
        configured: Bool,
        badgeText: String,
        badgeImage: String,
        badgeColor: Color,
        detail: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Label(client.title, systemImage: client.systemImage)
                Spacer()
                StatusBadge(
                    text: badgeText,
                    systemImage: badgeImage,
                    color: badgeColor
                )
            }
            Text(detail)
                .font(.caption)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            SumpterWrappingLayout(horizontalSpacing: 8, verticalSpacing: 8) {
                Button {
                    model.setNotificationClient(client, enabled: true)
                } label: {
                    Label(configured ? "重新安装" : "安装配置", systemImage: "hammer")
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                Button("移除配置…", role: .destructive) {
                    clientPendingRemoval = client
                }
                .controlSize(.small)
                .disabled(!configured)
            }
        }
    }

    private var codexStatusImage: String {
        switch model.codexNotificationHookStatus {
        case .notConfigured: "bell.slash"
        case .pendingTrust: "clock"
        case .verified: "checkmark.seal.fill"
        case .legacyConflict: "exclamationmark.triangle.fill"
        case .writeFailed: "xmark.octagon.fill"
        }
    }

    private var codexStatusColor: Color {
        switch model.codexNotificationHookStatus {
        case .notConfigured: .secondary
        case .pendingTrust: .orange
        case .verified: .green
        case .legacyConflict, .writeFailed: .red
        }
    }

    private var categoryPanel: some View {
        SectionPanel(title: "统一通知类别", hint: "这些开关同时适用于 Claude Code、Codex CLI 与 Grok Build；关闭普通状态不会影响失败、完成或需要你操作的事件。") {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(UnifiedNotificationCategory.allCases) { row in
                    HStack(alignment: .top) {
                        Label(row.title, systemImage: row.systemImage)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(row.hint)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Toggle(row.title, isOn: Binding(
                            get: { model.notificationCategoryEnabled(row.category) },
                            set: { model.setNotificationCategory(row.category, enabled: $0) }
                        ))
                        .labelsHidden()
                    }
                    if row != UnifiedNotificationCategory.allCases.last {
                        Divider()
                    }
                }
            }
        }
    }

    private var soundPanel: some View {
        SectionPanel(title: "声音与测试", hint: "提示音选项来自 macOS 的系统音效目录；试听声音会立即播放，发送测试通知用于验证横幅是否弹出。") {
            SumpterWrappingLayout(horizontalSpacing: 10, verticalSpacing: 8) {
                Picker("提示音", selection: Binding(
                    get: { model.notificationSoundPreference },
                    set: { model.setNotificationSoundPreference($0) }
                )) {
                    ForEach(NotificationSoundPreference.allCases) { sound in
                        Text(sound.title).tag(sound)
                    }
                }
                .frame(minWidth: 180, idealWidth: 220, maxWidth: 260)
                Button {
                    model.previewNotificationSound()
                } label: {
                    Label("试听声音", systemImage: "speaker.wave.2")
                }
                Button {
                    model.sendTestNotification()
                } label: {
                    Label("发送测试通知", systemImage: "paperplane")
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var authorizationTitle: String {
        switch model.notificationAuthorizationStatus {
        case .notDetermined: "未询问"
        case .denied: "已拒绝"
        case .authorized: "已允许"
        case .provisional: "临时允许"
        case .unknown: "未知"
        }
    }

    private var authorizationSystemImage: String {
        switch model.notificationAuthorizationStatus {
        case .authorized, .provisional: "checkmark.circle.fill"
        case .denied: "xmark.octagon.fill"
        case .notDetermined: "questionmark.circle"
        case .unknown: "exclamationmark.triangle"
        }
    }

    private var authorizationColor: Color {
        switch model.notificationAuthorizationStatus {
        case .authorized, .provisional: .green
        case .denied: .red
        case .notDetermined: .orange
        case .unknown: .secondary
        }
    }
}
