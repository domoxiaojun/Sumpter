import AppKit
import Foundation
import ServiceManagement
import SwiftUI
import UserNotifications
import SumpterCore

@MainActor
extension AppModel {
    func setClaudeNotifications(enabled: Bool) {
        do {
            try ClaudeNotificationHooks.setEnabled(enabled, port: config.listener.port)
            refreshNotificationHookState()
            if enabled {
                requestNotificationAuthorization()
                flash("已安装 Claude Code 通知")
            } else {
                flash("已移除 Claude Code 通知")
            }
        } catch {
            lastError = "\(error)"
            flash("Claude Code 通知设置失败")
        }
    }

    func setCodexNotifications(enabled: Bool) {
        do {
            try CodexNotificationHooks.setEnabled(enabled, port: config.listener.port)
            refreshNotificationHookState()
            if enabled {
                requestNotificationAuthorization()
                flash("已安装 Codex CLI 通知")
            } else {
                flash("已移除 Codex CLI 通知")
            }
        } catch {
            refreshNotificationHookState()
            lastError = "\(error)"
            flash("Codex 通知设置失败")
        }
    }

    func setGrokNotifications(enabled: Bool) {
        do {
            try GrokNotificationHooks.setEnabled(enabled, port: config.listener.port)
            GrokNotificationHooks.markRemovedByUser(!enabled)
            refreshNotificationHookState()
            if enabled {
                requestNotificationAuthorization()
                flash("已安装 Grok Build 通知")
            } else {
                flash("已移除 Grok Build 通知")
            }
        } catch {
            refreshNotificationHookState()
            lastError = "\(error)"
            flash("Grok Build 通知设置失败")
        }
    }

    func setNotificationClient(_ client: NotificationHookClient, enabled: Bool) {
        switch client {
        case .claude:
            setClaudeNotifications(enabled: enabled)
        case .codex:
            setCodexNotifications(enabled: enabled)
        case .grok:
            setGrokNotifications(enabled: enabled)
        }
    }

    func setNotifications(enabled: Bool) {
        var failed: [String] = []
        do {
            try ClaudeNotificationHooks.setEnabled(enabled, port: config.listener.port)
        } catch {
            failed.append("Claude Code")
            lastError = "\(error)"
        }
        do {
            try CodexNotificationHooks.setEnabled(enabled, port: config.listener.port)
        } catch {
            failed.append("Codex CLI")
            lastError = "\(error)"
        }
        do {
            try GrokNotificationHooks.setEnabled(enabled, port: config.listener.port)
            GrokNotificationHooks.markRemovedByUser(!enabled)
        } catch {
            failed.append("Grok Build")
            lastError = "\(error)"
        }
        refreshNotificationHookState()
        if enabled {
            requestNotificationAuthorization()
        }
        if failed.isEmpty {
            flash(enabled ? "已启用全部客户端通知" : "已移除全部客户端通知")
        } else {
            flash("部分通知配置失败：\(failed.joined(separator: "、"))")
        }
    }

    func setClaudeNotification(argument: String, enabled: Bool) {
        do {
            var arguments = claudeNotificationArguments
            if enabled {
                arguments.insert(argument)
            } else {
                arguments.remove(argument)
            }
            try ClaudeNotificationHooks.setSelectedArguments(arguments, port: config.listener.port)
            refreshNotificationHookState()
            if !arguments.isEmpty {
                requestNotificationAuthorization()
            }
        } catch {
            lastError = "\(error)"
            flash("通知类型设置失败")
        }
    }

    func setNotificationCategory(_ category: String, enabled: Bool) {
        switch category {
        case "action_required": actionNotificationsEnabled = enabled
        case "status": statusNotificationsEnabled = enabled
        case "turn_completed": turnCompletionNotificationsEnabled = enabled
        case "subtask_completed": subtaskNotificationsEnabled = enabled
        case "turn_failed": failureNotificationsEnabled = enabled
        default: return
        }
        UserDefaults.standard.set(enabled, forKey: notificationCategoryKey(category))
        if enabled { requestNotificationAuthorization() }
    }

    func refreshNotificationHookState() {
        claudeNotificationArguments = ClaudeNotificationHooks.enabledArguments()
        claudeNotificationsEnabled = !claudeNotificationArguments.isEmpty
        codexNotificationHookPath = CodexNotificationHooks.resolvedHooksJSONPath()
        codexNotificationArguments = CodexNotificationHooks.enabledArguments()
        codexNotificationHookStatus = CodexNotificationHooks.currentStatus()
        codexNotificationsEnabled = !codexNotificationArguments.isEmpty
        grokNotificationHookPath = GrokNotificationHooks.resolvedHooksJSONPath()
        grokNotificationsEnabled = GrokNotificationHooks.isEnabled()
    }

    func sendTestNotification() {
        Task {
            await deliverNotification(title: "Sumpter", message: "通知已连接")
        }
    }

    func previewNotificationSound() {
        NativeNotifier.shared.playSound(notificationSoundPreference)
    }

    func requestNotificationAuthorization() {
        Task {
            do {
                notificationAuthorizationStatus = try await NativeNotifier.shared.requestAuthorization()
                notificationError = nil
            } catch {
                notificationError = "\(error)"
                lastError = "\(error)"
            }
        }
    }

    func refreshNotificationAuthorizationStatus() {
        Task {
            NativeNotifier.shared.configure()
            notificationAuthorizationStatus = await NativeNotifier.shared.authorizationStatus()
        }
    }

    func openNotificationSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.notifications") {
            NSWorkspace.shared.open(url)
        } else {
            NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/System Settings.app"))
        }
    }

    /// 在访达里打开Sumpter的配置目录(config.json / runtime.sqlite3 / 旧 stats.json / proxy.log 所在处)。
    func openConfigDirectory() {
        do {
            let directory = try SumpterPaths.appSupportDirectory()
            NSWorkspace.shared.activateFileViewerSelecting([directory])
        } catch {
            flash("打开配置目录失败")
            lastError = "\(error)"
        }
    }

    func setNotificationSoundPreference(_ preference: NotificationSoundPreference) {
        notificationSoundPreference = preference
        preference.save()
    }

    func refreshLoginItemStatus() {
        loginItemEnabled = SMAppService.mainApp.status == .enabled
    }

    func setLoginItem(enabled: Bool) {
        do {
            if enabled {
                if SMAppService.mainApp.status != .enabled {
                    try SMAppService.mainApp.register()
                }
            } else if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            }
            refreshLoginItemStatus()
        } catch {
            lastError = "\(error)"
            flash("自启动设置失败")
            refreshLoginItemStatus()
        }
    }

    // MARK: - 入口

}
