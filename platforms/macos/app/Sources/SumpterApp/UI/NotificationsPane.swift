import SwiftUI

struct NotificationsPane: View {
    @ObservedObject var model: AppModel

    private let eventRows: [(argument: String, title: String, systemImage: String)] = [
        ("notification", "Claude 状态与行动事件", "person.crop.circle.badge.exclamationmark"),
        ("stop", "回合结束", "checkmark.circle"),
        ("subagent_stop", "子任务结束", "square.stack.3d.up"),
        ("stop_failure", "回合异常 / 最终失败", "xmark.octagon")
    ]

    private let categoryRows: [(category: String, title: String, hint: String, systemImage: String)] = [
        ("action_required", "需要我处理", "权限、输入、选择、确认", "hand.raised"),
        ("status", "普通状态提示", "认证、计算机控制等状态变化（默认关闭）", "info.circle"),
        ("turn_completed", "普通回合完成", "主会话正常结束", "checkmark.circle"),
        ("subtask_completed", "子任务完成", "Agent / 子代理任务结束", "square.stack.3d.up"),
        ("turn_failed", "最终失败", "仅 StopFailure 或已关联的最终失败", "exclamationmark.triangle")
    ]

    var body: some View {
        SettingsPage(title: SettingsSection.notifications.title, subtitle: SettingsSection.notifications.subtitle) {
            authorizationPanel
            hookPanel
            typesPanel
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

    /// hook 总开关状态(settings.json 里有任一事件参数即视为启用)。
    private var hookEnabled: Bool {
        !model.claudeNotificationArguments.isEmpty || model.claudeNotificationsEnabled
    }

    private var hookPanel: some View {
        SectionPanel(title: "Claude Code Hook", hint: "把 Claude Code 事件写入 ~/.claude/settings.json，通过本地代理转成系统通知。代理只转发客户端已经产生的事件，不根据单次上游 502 猜测失败；同一会话的通知在通知中心堆叠成组。") {
            VStack(alignment: .leading, spacing: 12) {
                SumpterWrappingLayout(horizontalSpacing: 10, verticalSpacing: 8) {
                    Toggle("启用 Claude Code 通知", isOn: Binding(
                        get: { hookEnabled },
                        set: { model.setClaudeNotifications(enabled: $0) }
                    ))
                    StatusBadge(
                        text: model.claudeNotificationArguments.isEmpty ? "未启用" : "已启用",
                        systemImage: model.claudeNotificationArguments.isEmpty ? "bell.slash" : "bell.fill",
                        color: model.claudeNotificationArguments.isEmpty ? .secondary : .green
                    )
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private var typesPanel: some View {
        SectionPanel(title: "Hook 事件来源", hint: hookEnabled ? nil : "先启用上方的 Claude Code 通知总开关。") {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(eventRows, id: \.argument) { row in
                    HStack {
                        Label(row.title, systemImage: row.systemImage)
                        Spacer()
                        Toggle(row.title, isOn: Binding(
                            get: { model.claudeNotificationArguments.contains(row.argument) },
                            set: { model.setClaudeNotification(argument: row.argument, enabled: $0) }
                        ))
                        .labelsHidden()
                    }
                    if row.argument != eventRows.last?.argument {
                        Divider()
                    }
                }
            }
            // 总开关关着时类型开关不可用,避免「勾了却没反应」的困惑。
            .disabled(!hookEnabled)
        }
    }

    private var categoryPanel: some View {
        SectionPanel(title: "客户端通知类别", hint: hookEnabled ? "通知类别在客户端过滤；关闭普通状态不会影响失败、完成或需要你操作的事件。" : "先启用上方的 Claude Code 通知总开关。") {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(categoryRows, id: \.category) { row in
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
                            set: { model.setClaudeNotificationCategory(row.category, enabled: $0) }
                        ))
                        .labelsHidden()
                    }
                    if row.category != categoryRows.last?.category {
                        Divider()
                    }
                }
            }
            .disabled(!hookEnabled)
        }
    }

    private var soundPanel: some View {
        SectionPanel(title: "声音与测试", hint: "试听声音会立即播放；发送测试通知用于验证横幅是否弹出。") {
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
