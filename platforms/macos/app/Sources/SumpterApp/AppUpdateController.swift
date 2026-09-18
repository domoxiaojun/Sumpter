import AppKit
import Combine
import Sparkle

/// Sparkle feed 是否已写入当前 bundle。开发包故意不写 `SUFeedURL`，
/// 避免本地 SwiftPM 运行时弹出配置错误；正式发布包由 package-app.sh 注入。
enum AppUpdateFeedAvailability: Equatable {
    case ready
    case missingFeed
    case incomplete

    var canCheck: Bool {
        self == .ready
    }

    var userMessage: String? {
        switch self {
        case .ready:
            nil
        case .missingFeed:
            "当前构建未配置更新源。开发包不会写入 Sparkle feed；请从 GitHub Releases 安装正式版后再检查更新。"
        case .incomplete:
            "更新源配置不完整，无法在应用内检查更新。"
        }
    }

    static func resolve(feedURL: String?, publicKey: String?) -> Self {
        let url = feedURL?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let key = publicKey?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if url.isEmpty && key.isEmpty {
            return .missingFeed
        }
        guard !url.isEmpty, URL(string: url) != nil, !key.isEmpty else {
            return .incomplete
        }
        return .ready
    }

    static func resolve(bundle: Bundle) -> Self {
        resolve(
            feedURL: bundle.object(forInfoDictionaryKey: "SUFeedURL") as? String,
            publicKey: bundle.object(forInfoDictionaryKey: "SUPublicEDKey") as? String
        )
    }
}

/// 单一 Sparkle 更新器。菜单栏、关于页和应用菜单必须共用这一份，
/// 不能各自 `SPUStandardUpdaterController`，否则自动检查会重复启动。
@MainActor
final class AppUpdateController: ObservableObject {
    static let shared = AppUpdateController()

    let availability: AppUpdateFeedAvailability
    var canCheckForUpdates: Bool { availability.canCheck }

    private var updaterController: SPUStandardUpdaterController?

    init(bundle: Bundle = .main) {
        availability = AppUpdateFeedAvailability.resolve(bundle: bundle)
    }

    /// 只在应用完成启动后调用。Sparkle 在 `applicationDidFinishLaunching`
    /// 之前 `startUpdater` 可能错过首次调度检查。
    func startIfNeeded() {
        guard canCheckForUpdates, updaterController == nil else { return }
        updaterController = SPUStandardUpdaterController(
            startingUpdater: true,
            updaterDelegate: nil,
            userDriverDelegate: nil
        )
    }

    func checkForUpdates() {
        startIfNeeded()
        guard updaterController != nil else { return }
        // LSUIElement 应用不激活的话，Sparkle 对话框会开在其它 app 后面。
        NSApp.activate(ignoringOtherApps: true)
        updaterController?.checkForUpdates(nil)
    }
}
