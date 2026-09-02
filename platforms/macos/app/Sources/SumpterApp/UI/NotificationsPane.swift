import SwiftUI

struct NotificationsPane: View {
    @ObservedObject var model: AppModel

    var body: some View {
        SettingsPage(title: SettingsSection.notifications.title, subtitle: SettingsSection.notifications.subtitle) {
            authorizationPanel
            hookPanel
            categoryPanel
            soundPanel
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

    /// Claude/Codex 任一接入已配置即视为统一通知已启用。
    private var hookEnabled: Bool {
        model.notificationsEnabled
    }

    private var hookPanel: some View {
        SectionPanel(title: "通知接入", hint: "一套开关同时管理 Claude Code 与 Codex CLI。两者的协议事件名不同，但都会归入下面同一组通知类别；Sumpter 只转发客户端已经产生的事件。") {
            VStack(alignment: .leading, spacing: 12) {
                SumpterWrappingLayout(horizontalSpacing: 10, verticalSpacing: 8) {
                    Toggle("启用 Claude Code / Codex CLI 通知", isOn: Binding(
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
                DisclosureGroup("各客户端接入状态") {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Label("Claude Code", systemImage: "terminal")
                            Spacer()
                            StatusBadge(
                                text: model.claudeNotificationArguments.isEmpty ? "未配置" : "已配置",
                                systemImage: model.claudeNotificationArguments.isEmpty ? "minus.circle" : "checkmark.circle",
                                color: model.claudeNotificationArguments.isEmpty ? .secondary : .green
                            )
                        }
                        HStack {
                            Label("Codex CLI", systemImage: "terminal.fill")
                            Spacer()
                            StatusBadge(
                                text: model.codexNotificationHookStatus.title,
                                systemImage: codexStatusImage,
                                color: codexStatusColor
                            )
                        }
                        Text("Codex 配置文件：\(model.codexNotificationHookPath)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                        switch model.codexNotificationHookStatus {
                        case .pendingTrust:
                            Text("首次使用请在 Codex CLI 输入 /hooks，信任 Sumpter 的通知 Hook；收到一次真实通知后会显示“已验证”。")
                                .font(.caption)
                                .foregroundStyle(.orange)
                        case .legacyConflict:
                            Text("检测到无法安全判断的 legacy notify。为避免误删，请先在 config.toml 中手动处理后再启用。")
                                .font(.caption)
                                .foregroundStyle(.red)
                        case .writeFailed:
                            Text("Codex 写入失败；请检查 CODEX_HOME、文件权限和 hooks.json 格式。")
                                .font(.caption)
                                .foregroundStyle(.red)
                        case .notConfigured, .verified:
                            EmptyView()
                        }
                    }
                    .padding(.top, 6)
                }
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
        SectionPanel(title: "统一通知类别", hint: "这些开关同时适用于 Claude Code 与 Codex CLI；关闭普通状态不会影响失败、完成或需要你操作的事件。") {
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
